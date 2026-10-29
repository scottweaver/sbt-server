//! Bytes to whole lines. Both the JVM's pipes and sbt's `sbt/systemOut`
//! notifications deliver arbitrary chunks that split mid-line and may not be
//! valid UTF-8; the assembler keeps the partial tail until a newline or a
//! flush arrives.

#[derive(Debug, Default)]
pub struct LineAssembler {
    pending: Vec<u8>,
}

impl LineAssembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; returns every line it completed, without newlines.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for byte in bytes {
            if *byte == b'\n' {
                lines.push(take_line(&mut self.pending));
            } else {
                self.pending.push(*byte);
            }
        }
        lines
    }

    /// Release the partial line, if any.
    pub fn flush(&mut self) -> Option<String> {
        (!self.pending.is_empty()).then(|| take_line(&mut self.pending))
    }
}

fn take_line(pending: &mut Vec<u8>) -> String {
    if pending.last() == Some(&b'\r') {
        pending.pop();
    }
    let line = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_across_chunks_come_out_whole() {
        let mut assembler = LineAssembler::new();
        assert_eq!(assembler.push(b"hello fr"), Vec::<String>::new());
        assert_eq!(
            assembler.push(b"om println\nsecond"),
            vec!["hello from println"]
        );
        assert_eq!(assembler.flush(), Some("second".to_owned()));
        assert_eq!(assembler.flush(), None);
    }

    #[test]
    fn crlf_is_stripped_and_invalid_utf8_is_replaced() {
        let mut assembler = LineAssembler::new();
        assert_eq!(assembler.push(b"a\r\nb\xff\n"), vec!["a", "b\u{fffd}"]);
    }

    #[test]
    fn empty_lines_are_preserved() {
        let mut assembler = LineAssembler::new();
        assert_eq!(assembler.push(b"\n\nx\n"), vec!["", "", "x"]);
    }
}
