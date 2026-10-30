#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use sbt_server_core::instance::descriptor::{
    SbtUserDir, ServerUri, parse_active_json, parse_token_file,
};

fuzz_target!(|text: &str| {
    let sbt_dir = SbtUserDir::resolve(Some("/home/fuzz".into())).unwrap();
    let path = Path::new("/build/project/target/active.json");
    if let Ok(active) = parse_active_json(path, text, &sbt_dir) {
        if let ServerUri::Tcp(socket) = active.uri {
            assert!(socket.socket_addr().ip().is_loopback());
        }
        if let Some(token_file) = active.token_file {
            assert!(token_file.path().starts_with(sbt_dir.path()));
        }
    }
    let _ = ServerUri::parse(text);
    let _ = parse_token_file(path, text);
});
