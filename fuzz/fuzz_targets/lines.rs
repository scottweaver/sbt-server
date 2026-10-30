#![no_main]

use libfuzzer_sys::fuzz_target;
use sbt_server_core::instance::lines::LineAssembler;

fuzz_target!(|chunks: Vec<Vec<u8>>| {
    let newlines = chunks.iter().flatten().filter(|b| **b == b'\n').count();
    let mut assembler = LineAssembler::new();
    let mut lines = 0;
    for chunk in &chunks {
        for line in assembler.push(chunk) {
            assert!(!line.contains('\n'));
            lines += 1;
        }
    }
    assert_eq!(lines, newlines);
    if let Some(tail) = assembler.flush() {
        assert!(!tail.contains('\n'));
    }
    assert!(assembler.flush().is_none());
});
