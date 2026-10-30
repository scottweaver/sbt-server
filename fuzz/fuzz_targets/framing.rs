#![no_main]

use libfuzzer_sys::fuzz_target;
use sbt_server_core::jsonrpc::framing::{FrameDecoder, encode_frame};

const MAX_BODY: usize = 1 << 16;

fuzz_target!(|chunks: Vec<Vec<u8>>| {
    let mut decoder = FrameDecoder::new(MAX_BODY);
    for chunk in &chunks {
        decoder.push(chunk);
        loop {
            match decoder.next_frame() {
                Ok(Some(body)) => {
                    assert!(body.len() <= MAX_BODY);
                    let mut again = FrameDecoder::new(MAX_BODY);
                    again.push(&encode_frame(&body));
                    assert_eq!(
                        again.next_frame().unwrap().as_deref(),
                        Some(body.as_slice())
                    );
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
});
