//! LSP-style framing as sbt's server speaks it: `Content-Length: N`, an
//! optional `Content-Type`, a blank line, then N bytes of JSON. sbt may append
//! a bare newline after a body, so the decoder skips whitespace between frames.

use thiserror::Error;

const HEADER_END_CRLF: &[u8] = b"\r\n\r\n";
const HEADER_END_LF: &[u8] = b"\n\n";
const MAX_HEADER_BYTES: usize = 8 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    #[error("frame header is not `Name: value`: {0:?}")]
    BadHeader(String),
    #[error("frame headers ended without a Content-Length")]
    MissingContentLength,
    #[error("Content-Length {len} exceeds the {max}-byte limit")]
    BodyTooLarge { len: usize, max: usize },
}

/// Push bytes in as they arrive, pull whole bodies out.
#[derive(Debug)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    max_body: usize,
}

impl FrameDecoder {
    #[must_use]
    pub fn new(max_body: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_body,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete body, or `None` until more bytes arrive.
    ///
    /// # Errors
    ///
    /// A malformed header block, a missing `Content-Length`, or a body larger
    /// than the configured limit. The decoder is unusable after an error.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FramingError> {
        self.skip_whitespace();
        let Some((header_len, separator_len)) = find_header_end(&self.buf) else {
            if self.buf.len() > MAX_HEADER_BYTES {
                return Err(FramingError::BadHeader(lossy_prefix(&self.buf)));
            }
            return Ok(None);
        };
        let content_length = parse_content_length(&self.buf[..header_len])?;
        if content_length > self.max_body {
            return Err(FramingError::BodyTooLarge {
                len: content_length,
                max: self.max_body,
            });
        }
        let body_start = header_len + separator_len;
        let body_end = body_start + content_length;
        if self.buf.len() < body_end {
            return Ok(None);
        }
        let body = self.buf[body_start..body_end].to_vec();
        self.buf.drain(..body_end);
        Ok(Some(body))
    }

    fn skip_whitespace(&mut self) {
        let n = self
            .buf
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        self.buf.drain(..n);
    }
}

/// `Content-Length: N\r\n\r\n` followed by the body.
#[must_use]
pub fn encode_frame(body: &[u8]) -> Vec<u8> {
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    frame
}

/// Offset of the header block's end and the length of the separator found.
fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    let crlf = find(buf, HEADER_END_CRLF).map(|at| (at, HEADER_END_CRLF.len()));
    let lf = find(buf, HEADER_END_LF).map(|at| (at, HEADER_END_LF.len()));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn parse_content_length(headers: &[u8]) -> Result<usize, FramingError> {
    let text =
        std::str::from_utf8(headers).map_err(|_| FramingError::BadHeader(lossy_prefix(headers)))?;
    let mut content_length = None;
    for line in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| FramingError::BadHeader(line.to_owned()))?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            let len = value
                .trim()
                .parse()
                .map_err(|_| FramingError::BadHeader(line.to_owned()))?;
            content_length = Some(len);
        }
    }
    content_length.ok_or(FramingError::MissingContentLength)
}

fn lossy_prefix(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 1024;

    fn decode_all(chunks: &[&[u8]]) -> Vec<String> {
        let mut decoder = FrameDecoder::new(LIMIT);
        let mut out = Vec::new();
        for chunk in chunks {
            decoder.push(chunk);
            while let Some(body) = decoder.next_frame().unwrap() {
                out.push(String::from_utf8(body).unwrap());
            }
        }
        out
    }

    #[test]
    fn single_frame() {
        assert_eq!(
            decode_all(&[b"Content-Length: 2\r\n\r\n{}"]),
            vec!["{}".to_owned()]
        );
    }

    #[test]
    fn two_frames_in_one_push_with_sbt_style_trailing_newlines() {
        let bytes = b"Content-Length: 3\r\n\r\n[1]\nContent-Length: 3\r\n\r\n[2]\n";
        assert_eq!(
            decode_all(&[bytes]),
            vec!["[1]".to_owned(), "[2]".to_owned()]
        );
    }

    #[test]
    fn frame_split_across_pushes_inside_header_and_inside_body() {
        let chunks: &[&[u8]] = &[b"Content-Len", b"gth: 5\r\n", b"\r\n\"ab", b"c\""];
        assert_eq!(decode_all(chunks), vec!["\"abc\"".to_owned()]);
    }

    #[test]
    fn content_type_header_is_tolerated_and_name_is_case_insensitive() {
        let bytes =
            b"content-length: 4\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\nnull";
        assert_eq!(decode_all(&[bytes]), vec!["null".to_owned()]);
    }

    #[test]
    fn bare_lf_separators_are_accepted() {
        assert_eq!(
            decode_all(&[b"Content-Length: 2\n\n{}"]),
            vec!["{}".to_owned()]
        );
    }

    #[test]
    fn missing_content_length_is_an_error() {
        let mut decoder = FrameDecoder::new(LIMIT);
        decoder.push(b"Content-Type: x\r\n\r\n{}");
        assert_eq!(
            decoder.next_frame(),
            Err(FramingError::MissingContentLength)
        );
    }

    #[test]
    fn header_without_colon_is_an_error() {
        let mut decoder = FrameDecoder::new(LIMIT);
        decoder.push(b"garbage\r\n\r\n{}");
        assert!(matches!(
            decoder.next_frame(),
            Err(FramingError::BadHeader(_))
        ));
    }

    #[test]
    fn oversized_body_is_refused_before_buffering_it() {
        let mut decoder = FrameDecoder::new(LIMIT);
        decoder.push(b"Content-Length: 4096\r\n\r\n");
        assert_eq!(
            decoder.next_frame(),
            Err(FramingError::BodyTooLarge {
                len: 4096,
                max: LIMIT
            })
        );
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let frame = encode_frame(br#"{"jsonrpc":"2.0"}"#);
        assert!(frame.starts_with(b"Content-Length: 17\r\n\r\n"));
        assert_eq!(
            decode_all(&[&frame]),
            vec![r#"{"jsonrpc":"2.0"}"#.to_owned()]
        );
    }
}
