//! The JSON-RPC client against the scripted fake sbt server.

use std::time::{Duration, Instant};

use fake_sbt::{Endpoint, ExecScript, FakeSbtServer, Options, Scenario, Transport};
use sbt_server_core::jsonrpc::{self, Connection, Notification, RequestError};
use serde_json::{Value, json};
use tokio::sync::mpsc;

async fn start(options: Options) -> (tempfile::TempDir, FakeSbtServer) {
    let root = tempfile::tempdir().unwrap();
    let server = FakeSbtServer::start(root.path(), options).await.unwrap();
    (root, server)
}

async fn connect(server: &FakeSbtServer) -> (Connection, mpsc::Receiver<Notification>) {
    let io = match server.endpoint() {
        Endpoint::Tcp(addr) => jsonrpc::connect_tcp(*addr).await.unwrap(),
        Endpoint::Local(path) => jsonrpc::connect_unix(path).await.unwrap(),
    };
    let (tx, rx) = mpsc::channel(64);
    (Connection::open(io, tx), rx)
}

fn initialize_params(token: Option<&str>) -> Value {
    match token {
        Some(token) => json!({ "initializationOptions": { "token": token } }),
        None => json!({ "initializationOptions": {} }),
    }
}

fn exec_params(command_line: &str) -> Value {
    json!({ "commandLine": command_line })
}

fn with_scenario(scenario: Scenario) -> Options {
    Options {
        scenario,
        ..Options::default()
    }
}

async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn initialize_then_exec_delivers_every_notification_before_the_response() {
    let (_root, server) = start(Options::default()).await;
    let (connection, mut notifications) = connect(&server).await;

    let capabilities = connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();
    assert_eq!(capabilities["capabilities"]["hoverProvider"], json!(false));

    let result = connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap();
    assert_eq!(result["status"], json!("Done"));
    assert_eq!(result["exitCode"], json!(0));

    let mut methods = Vec::new();
    while let Ok(notification) = notifications.try_recv() {
        methods.push(notification.method);
    }
    assert_eq!(
        methods,
        [
            "build/logMessage",
            "build/taskStart",
            "build/logMessage",
            "build/taskProgress",
            "sbt/systemOut",
            "sbt/systemOut",
            "sbt/systemOutFlush",
            "build/logMessage",
            "build/publishDiagnostics",
            "build/taskFinish",
            "build/logMessage",
        ]
    );
}

#[tokio::test]
async fn server_initiated_requests_are_declined_with_method_not_found() {
    let (_root, server) = start(Options::default()).await;
    let (connection, _notifications) = connect(&server).await;
    connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();
    connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap();

    let reply_to_server = || {
        server
            .requests()
            .into_iter()
            .find(|r| r.method.is_none() && r.id == Some(json!("srv-1")))
    };
    wait_until("the client's reply to srv-1", || {
        reply_to_server().is_some()
    })
    .await;
    let reply = reply_to_server().unwrap();
    assert_eq!(reply.error.unwrap()["code"], json!(-32601));
}

#[tokio::test]
async fn exec_before_initialize_is_refused_when_a_token_is_required() {
    let options = Options {
        token: Some("84046191245433876643612047032303751629".into()),
        ..Options::default()
    };
    let (_root, server) = start(options).await;
    let (connection, _notifications) = connect(&server).await;

    let error = connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap_err();
    let RequestError::Rpc(rpc) = error else {
        panic!("expected an RPC error, got {error}")
    };
    assert_eq!(rpc.code, -32600);
    assert!(
        rpc.message.contains("not allowed before authentication"),
        "{rpc}"
    );
}

#[tokio::test]
async fn wrong_token_is_rejected_and_the_right_one_opens_the_session() {
    let options = Options {
        token: Some("123".into()),
        ..Options::default()
    };
    let (_root, server) = start(options).await;
    let (connection, _notifications) = connect(&server).await;

    let error = connection
        .request("initialize", initialize_params(Some("999")))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, RequestError::Rpc(rpc) if rpc.message == "invalid token"),
        "{error}"
    );

    connection
        .request("initialize", initialize_params(Some("123")))
        .await
        .unwrap();
    let result = connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap();
    assert_eq!(result["exitCode"], json!(0));
}

#[tokio::test]
async fn failing_exec_surfaces_sbts_exit_code_as_the_error_code() {
    let (_root, server) = start(with_scenario(Scenario {
        exec: ExecScript::FailsWith(1),
        ..Scenario::default()
    }))
    .await;
    let (connection, mut notifications) = connect(&server).await;
    connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();

    let error = connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap_err();
    let RequestError::Rpc(rpc) = error else {
        panic!("expected an RPC error, got {error}")
    };
    assert_eq!(rpc.code, 1);
    assert!(rpc.message.contains("Compilation failed"), "{rpc}");

    let diagnostics = std::iter::from_fn(|| notifications.try_recv().ok())
        .find(|n| n.method == "build/publishDiagnostics")
        .unwrap();
    assert_eq!(diagnostics.params["diagnostics"][0]["severity"], json!(1));
}

#[tokio::test]
async fn pending_requests_fail_when_the_server_goes_away() {
    let (_root, server) = start(with_scenario(Scenario {
        exec: ExecScript::Mute,
        ..Scenario::default()
    }))
    .await;
    let (connection, _notifications) = connect(&server).await;
    connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();

    let pending = {
        let connection = connection.clone();
        tokio::spawn(async move { connection.request("sbt/exec", exec_params("compile")).await })
    };
    wait_until("the exec to reach the server", || {
        !server.exec_requests().is_empty()
    })
    .await;
    server.stop().await;

    let error = pending.await.unwrap().unwrap_err();
    assert!(matches!(error, RequestError::ConnectionClosed), "{error}");
    wait_until("the actor to exit", || connection.is_closed()).await;
    let error = connection
        .request("sbt/exec", exec_params("compile"))
        .await
        .unwrap_err();
    assert!(matches!(error, RequestError::ConnectionClosed), "{error}");
}

#[tokio::test]
async fn shutdown_is_an_exec_that_removes_the_descriptor() {
    let (_root, server) = start(Options::default()).await;
    let (connection, _notifications) = connect(&server).await;
    connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();
    assert!(server.active_json().is_file());

    let error = connection
        .request("sbt/exec", exec_params("shutdown"))
        .await
        .unwrap_err();
    assert!(matches!(error, RequestError::ConnectionClosed), "{error}");
    tokio::time::timeout(Duration::from_secs(5), server.finished())
        .await
        .unwrap();
    assert!(!server.active_json().exists());
    wait_until("the connection to close", || connection.is_closed()).await;
}

#[tokio::test]
async fn unix_socket_transport_is_supported() {
    let socket = std::env::temp_dir().join(format!("fake-sbt-{}.sock", std::process::id()));
    let options = Options {
        transport: Transport::Local(socket.clone()),
        ..Options::default()
    };
    let (_root, server) = start(options).await;
    assert_eq!(
        server.endpoint().uri(),
        format!("local://{}", socket.display())
    );
    let (connection, _notifications) = connect(&server).await;

    let capabilities = connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();
    assert!(capabilities["capabilities"].is_object());
    server.stop().await;
    assert!(!socket.exists());
}

#[tokio::test]
async fn concurrent_requests_are_correlated_by_id() {
    let (_root, server) = start(Options::default()).await;
    let (connection, _notifications) = connect(&server).await;
    connection
        .request("initialize", initialize_params(None))
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        connection.request("sbt/exec", exec_params("compile")),
        connection.request("sbt/exec", exec_params("test")),
    );
    assert_eq!(first.unwrap()["status"], json!("Done"));
    assert_eq!(second.unwrap()["status"], json!("Done"));
    let ids: Vec<_> = server.exec_requests().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, [Some(json!("2")), Some(json!("3"))]);
}
