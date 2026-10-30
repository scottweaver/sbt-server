//! JSON-RPC 2.0 message shapes as sbt uses them. Request ids go out as JSON
//! strings because sbt stores and echoes them as strings; parsing accepts a
//! number too so a captured fixture from another client still decodes.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Value, json};
use thiserror::Error;

/// Id of a request this client issued. Never `__`-prefixed: sbt treats such
/// ids as internal and skips the completion response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

impl RequestId {
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawId {
    Number(u64),
    Text(String),
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match RawId::deserialize(deserializer)? {
            RawId::Number(n) => Ok(Self(n)),
            RawId::Text(s) => s
                .parse()
                .map(Self)
                .map_err(|_| de::Error::custom(format!("request id {s:?} is not an integer"))),
        }
    }
}

/// The `error` member of a response. For `sbt/exec`, `code` is sbt's exit
/// code.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error {}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

/// Anything the server can send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Response {
        id: RequestId,
        outcome: Result<Value, RpcError>,
    },
    Notification(Notification),
    /// sbt asking the client something (terminal queries); answered with
    /// "method not found" so sbt never waits on us.
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
    },
}

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("body is not valid JSON-RPC: {0}")]
    Json(#[from] serde_json::Error),
    #[error("message has neither `method` nor `id`")]
    Shapeless,
    #[error("response id {0} is not one this client issued")]
    ForeignId(Value),
    #[error("response {id} carries both `result` and `error`")]
    Ambiguous { id: RequestId },
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// Classify one decoded frame body.
///
/// # Errors
///
/// Invalid JSON, a message with neither `method` nor `id`, a response whose
/// id is not an integer, or a response with both `result` and `error`.
pub fn parse_incoming(body: &[u8]) -> Result<Incoming, MessageError> {
    let raw: Raw = serde_json::from_slice(body)?;
    let params = raw.params.unwrap_or(Value::Null);
    match (raw.method, raw.id) {
        (Some(method), None) => Ok(Incoming::Notification(Notification { method, params })),
        (Some(method), Some(id)) => Ok(Incoming::ServerRequest { id, method, params }),
        (None, Some(id)) => {
            let request_id: RequestId =
                serde_json::from_value(id.clone()).map_err(|_| MessageError::ForeignId(id))?;
            let outcome = match (raw.result, raw.error) {
                (Some(_), Some(_)) => return Err(MessageError::Ambiguous { id: request_id }),
                (_, Some(error)) => Err(error),
                (result, None) => Ok(result.unwrap_or(Value::Null)),
            };
            Ok(Incoming::Response {
                id: request_id,
                outcome,
            })
        }
        (None, None) => Err(MessageError::Shapeless),
    }
}

#[must_use]
pub fn encode_request(id: RequestId, method: &str, params: &Value) -> Vec<u8> {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
    .into_bytes()
}

const TOKEN_POINTER: &str = "/params/initializationOptions/token";

/// The frame as text for a trace line, with the `initialize` request's sbt
/// token replaced. Inbound frames need no redaction: sbt never echoes it.
#[must_use]
pub fn redact_frame(body: &[u8]) -> String {
    let Ok(mut frame) = serde_json::from_slice::<Value>(body) else {
        return String::from_utf8_lossy(body).into_owned();
    };
    match frame.pointer_mut(TOKEN_POINTER) {
        Some(token) => {
            *token = json!("<redacted>");
            frame.to_string()
        }
        None => String::from_utf8_lossy(body).into_owned(),
    }
}

/// A success response to a request the server sent us.
#[must_use]
pub fn encode_response(id: &Value, result: &Value) -> Vec<u8> {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
        .to_string()
        .into_bytes()
}

/// JSON-RPC "method not found" for a request the server sent us.
#[must_use]
pub fn encode_method_not_found(id: &Value, method: &str) -> Vec<u8> {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32601, "message": format!("method not found: {method}") },
    })
    .to_string()
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_string_ids() {
        let bytes = encode_request(
            RequestId::new(7),
            "sbt/exec",
            &json!({"commandLine": "compile"}),
        );
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["id"], json!("7"));
        assert_eq!(value["jsonrpc"], json!("2.0"));
        assert_eq!(value["params"]["commandLine"], json!("compile"));
    }

    #[test]
    fn redact_frame_hides_the_initialize_token() {
        let secret = "84046191245433876643612047032303751629";
        let body = encode_request(
            RequestId::new(1),
            "initialize",
            &json!({"initializationOptions": {"token": secret}}),
        );
        let text = redact_frame(&body);
        assert!(!text.contains(secret), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(text.contains("initialize"), "{text}");
    }

    #[test]
    fn redact_frame_leaves_other_frames_as_they_are() {
        let body = encode_request(
            RequestId::new(2),
            "sbt/exec",
            &json!({"commandLine": "compile"}),
        );
        assert_eq!(redact_frame(&body).as_bytes(), body.as_slice());
    }

    #[test]
    fn response_ids_parse_from_string_or_number() {
        for body in [
            br#"{"jsonrpc":"2.0","id":"3","result":{"status":"Done"}}"#.as_slice(),
            br#"{"jsonrpc":"2.0","id":3,"result":{"status":"Done"}}"#.as_slice(),
        ] {
            let Incoming::Response { id, outcome } = parse_incoming(body).unwrap() else {
                panic!("expected a response")
            };
            assert_eq!(id, RequestId::new(3));
            assert_eq!(outcome.unwrap()["status"], json!("Done"));
        }
    }

    #[test]
    fn error_responses_surface_the_code() {
        let body = br#"{"jsonrpc":"2.0","id":"4","error":{"code":1,"message":""}}"#;
        let Incoming::Response { outcome, .. } = parse_incoming(body).unwrap() else {
            panic!("expected a response")
        };
        let error = outcome.unwrap_err();
        assert_eq!(error.code, 1);
        assert_eq!(error.message, "");
        assert_eq!(error.data, None);
    }

    #[test]
    fn notifications_have_a_method_and_no_id() {
        let body =
            br#"{"jsonrpc":"2.0","method":"build/logMessage","params":{"type":3,"message":"hi"}}"#;
        let Incoming::Notification(notification) = parse_incoming(body).unwrap() else {
            panic!("expected a notification")
        };
        assert_eq!(notification.method, "build/logMessage");
        assert_eq!(notification.params["type"], json!(3));
    }

    #[test]
    fn server_requests_keep_their_foreign_id_verbatim() {
        let body =
            br#"{"jsonrpc":"2.0","id":"srv-1","method":"sbt/terminalPropertiesQuery","params":{}}"#;
        let Incoming::ServerRequest { id, method, .. } = parse_incoming(body).unwrap() else {
            panic!("expected a server request")
        };
        assert_eq!(id, json!("srv-1"));
        assert_eq!(method, "sbt/terminalPropertiesQuery");
        let reply: Value = serde_json::from_slice(&encode_method_not_found(&id, &method)).unwrap();
        assert_eq!(reply["id"], json!("srv-1"));
        assert_eq!(reply["error"]["code"], json!(-32601));
    }

    #[test]
    fn malformed_messages_are_classified() {
        assert!(matches!(
            parse_incoming(br#"{"jsonrpc":"2.0"}"#),
            Err(MessageError::Shapeless)
        ));
        assert!(matches!(
            parse_incoming(br#"{"jsonrpc":"2.0","id":"abc","result":null}"#),
            Err(MessageError::ForeignId(_))
        ));
        assert!(matches!(
            parse_incoming(br#"{"id":"1","result":1,"error":{"code":1,"message":""}}"#),
            Err(MessageError::Ambiguous { .. })
        ));
        assert!(matches!(
            parse_incoming(b"not json"),
            Err(MessageError::Json(_))
        ));
    }
}
