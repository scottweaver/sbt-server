#![no_main]

use libfuzzer_sys::fuzz_target;
use sbt_server_core::jsonrpc::message::{parse_incoming, redact_frame};

fuzz_target!(|body: &[u8]| {
    let _ = parse_incoming(body);
    let _ = redact_frame(body);
});
