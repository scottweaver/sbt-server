#![no_main]

use libfuzzer_sys::fuzz_target;
use sbt_server_daemon::{LoopbackHost, parse_loopback_host};

fuzz_target!(|text: &str| {
    if let Ok(LoopbackHost::Ip(ip)) = parse_loopback_host(text) {
        assert!(
            ip.is_loopback(),
            "{text:?} was admitted as non-loopback {ip}"
        );
    }
});
