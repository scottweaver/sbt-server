//! What each sbt notification means to the instance. Method names are a
//! closed set matched exhaustively; anything else is ignored, which is what
//! sbt's own thin client does too.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::jsonrpc::Notification;
use crate::protocol::{Diagnostic, LogLevel, Severity};

/// Every notification method this client acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownMethod {
    LogMessage,
    SystemOut,
    SystemErr,
    SystemOutFlush,
    SystemErrFlush,
    PublishDiagnostics,
    /// sbt 1.x's own BSP-shaped form, the one it actually sends.
    BuildPublishDiagnostics,
    Shutdown,
}

impl KnownMethod {
    #[must_use]
    pub fn parse(method: &str) -> Option<Self> {
        match method {
            "build/logMessage" => Some(Self::LogMessage),
            "sbt/systemOut" => Some(Self::SystemOut),
            "sbt/systemErr" => Some(Self::SystemErr),
            "sbt/systemOutFlush" => Some(Self::SystemOutFlush),
            "sbt/systemErrFlush" => Some(Self::SystemErrFlush),
            "textDocument/publishDiagnostics" => Some(Self::PublishDiagnostics),
            "build/publishDiagnostics" => Some(Self::BuildPublishDiagnostics),
            "shutdown" => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// sbt's two forwarded process streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbtStream {
    Out,
    Err,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translated {
    Log {
        level: LogLevel,
        text: String,
    },
    /// Raw bytes for a line assembler; sbt chunks these arbitrarily.
    Bytes {
        stream: SbtStream,
        bytes: Vec<u8>,
    },
    Flush {
        stream: SbtStream,
    },
    Diagnostics {
        file: PathBuf,
        items: Vec<Diagnostic>,
    },
    /// The server announced it is going down.
    ServerShutdown,
    /// Unknown method, or a known one with an unusable payload.
    Ignored,
}

#[derive(Deserialize)]
struct LogMessageParams {
    #[serde(rename = "type")]
    kind: u8,
    message: String,
}

#[derive(Deserialize)]
struct PublishDiagnosticsParams {
    uri: String,
    diagnostics: Vec<LspDiagnostic>,
}

#[derive(Deserialize)]
struct BuildPublishDiagnosticsParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
    diagnostics: Vec<LspDiagnostic>,
}

#[derive(Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

#[derive(Deserialize)]
struct LspDiagnostic {
    range: LspRange,
    #[serde(default)]
    severity: Option<u8>,
    message: String,
}

#[derive(Deserialize)]
struct LspRange {
    start: LspPosition,
}

#[derive(Deserialize)]
struct LspPosition {
    line: u32,
    character: u32,
}

#[must_use]
pub fn translate(notification: &Notification) -> Translated {
    let Some(method) = KnownMethod::parse(&notification.method) else {
        return Translated::Ignored;
    };
    let params = &notification.params;
    match method {
        KnownMethod::LogMessage => log_message(params),
        KnownMethod::SystemOut => bytes(SbtStream::Out, params),
        KnownMethod::SystemErr => bytes(SbtStream::Err, params),
        KnownMethod::SystemOutFlush => Translated::Flush {
            stream: SbtStream::Out,
        },
        KnownMethod::SystemErrFlush => Translated::Flush {
            stream: SbtStream::Err,
        },
        KnownMethod::PublishDiagnostics => diagnostics(params),
        KnownMethod::BuildPublishDiagnostics => build_diagnostics(params),
        KnownMethod::Shutdown => Translated::ServerShutdown,
    }
}

fn log_message(params: &Value) -> Translated {
    match serde_json::from_value::<LogMessageParams>(params.clone()) {
        Ok(LogMessageParams { kind, message }) => Translated::Log {
            level: log_level(kind),
            text: message,
        },
        Err(_) => Translated::Ignored,
    }
}

/// LSP `MessageType`: 1 error, 2 warning, 3 info, 4 log, 5 debug.
fn log_level(kind: u8) -> LogLevel {
    match kind {
        1 => LogLevel::Error,
        2 => LogLevel::Warning,
        4 | 5 => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

/// sbt serializes `Seq[Byte]`, so values are signed: -1 is 0xFF.
fn bytes(stream: SbtStream, params: &Value) -> Translated {
    let Some(values) = params.as_array() else {
        return Translated::Ignored;
    };
    let bytes = values
        .iter()
        .filter_map(Value::as_i64)
        .filter_map(|v| u8::try_from(v & 0xFF).ok())
        .collect();
    Translated::Bytes { stream, bytes }
}

fn diagnostics(params: &Value) -> Translated {
    match serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
        Ok(parsed) => diagnostics_for(&parsed.uri, parsed.diagnostics),
        Err(_) => Translated::Ignored,
    }
}

fn build_diagnostics(params: &Value) -> Translated {
    match serde_json::from_value::<BuildPublishDiagnosticsParams>(params.clone()) {
        Ok(parsed) => diagnostics_for(&parsed.text_document.uri, parsed.diagnostics),
        Err(_) => Translated::Ignored,
    }
}

fn diagnostics_for(uri: &str, diagnostics: Vec<LspDiagnostic>) -> Translated {
    Translated::Diagnostics {
        file: file_path(uri),
        items: diagnostics.into_iter().map(diagnostic).collect(),
    }
}

fn diagnostic(lsp: LspDiagnostic) -> Diagnostic {
    Diagnostic {
        line: lsp.range.start.line + 1,
        column: lsp.range.start.character + 1,
        severity: match lsp.severity {
            Some(1) => Severity::Error,
            Some(2) => Severity::Warning,
            Some(4) => Severity::Hint,
            _ => Severity::Information,
        },
        text: lsp.message,
    }
}

/// Split a terminal line sbt's console appender formatted, such as
/// `[info] compiling 1 Scala source`, into its level and text. Lines with
/// any other prefix (`[success]`, a test framework's own) are not log lines.
#[must_use]
pub fn log_line(text: &str) -> Option<(LogLevel, &str)> {
    let rest = text.strip_prefix('[')?;
    let (tag, after) = rest.split_once("] ")?;
    let level = match tag {
        "debug" => LogLevel::Debug,
        "info" => LogLevel::Info,
        "warn" => LogLevel::Warning,
        "error" => LogLevel::Error,
        _ => return None,
    };
    Some((level, after))
}

/// `file:///a/b%20c.scala` to `/a/b c.scala`; a non-file uri is kept verbatim.
fn file_path(uri: &str) -> PathBuf {
    match uri.strip_prefix("file://") {
        Some(rest) => PathBuf::from(percent_decode(rest)),
        None => PathBuf::from(uri),
    }
}

fn percent_decode(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = decoded {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn notification(method: &str, params: Value) -> Notification {
        Notification {
            method: method.to_owned(),
            params,
        }
    }

    #[test]
    fn log_messages_map_lsp_message_types_to_levels() {
        for (kind, level) in [
            (1, LogLevel::Error),
            (2, LogLevel::Warning),
            (3, LogLevel::Info),
            (4, LogLevel::Debug),
        ] {
            let translated = translate(&notification(
                "build/logMessage",
                json!({ "type": kind, "message": "m" }),
            ));
            assert_eq!(
                translated,
                Translated::Log {
                    level,
                    text: "m".into()
                }
            );
        }
    }

    #[test]
    fn system_out_bytes_are_signed_scala_bytes() {
        let translated = translate(&notification("sbt/systemOut", json!([104, 105, -1, 10])));
        assert_eq!(
            translated,
            Translated::Bytes {
                stream: SbtStream::Out,
                bytes: vec![104, 105, 0xFF, 10]
            }
        );
        assert_eq!(
            translate(&notification("sbt/systemErrFlush", json!({}))),
            Translated::Flush {
                stream: SbtStream::Err
            }
        );
    }

    #[test]
    fn diagnostics_become_one_based_with_decoded_paths() {
        let translated = translate(&notification(
            "textDocument/publishDiagnostics",
            json!({
                "uri": "file:///tmp/my%20proj/src/A.scala",
                "diagnostics": [{
                    "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 2, "character": 7 } },
                    "severity": 1,
                    "source": "sbt",
                    "message": "not found: value foo"
                }]
            }),
        ));
        assert_eq!(
            translated,
            Translated::Diagnostics {
                file: PathBuf::from("/tmp/my proj/src/A.scala"),
                items: vec![Diagnostic {
                    line: 3,
                    column: 5,
                    severity: Severity::Error,
                    text: "not found: value foo".into()
                }]
            }
        );
    }

    #[test]
    fn sbts_own_build_publish_diagnostics_shape_is_understood() {
        let translated = translate(&notification(
            "build/publishDiagnostics",
            json!({
                "textDocument": { "uri": "file:///tmp/hello/src/main/scala/Hello.scala" },
                "buildTarget": { "uri": "file:/tmp/hello/#root/Compile" },
                "diagnostics": [{
                    "range": { "start": { "line": 1, "character": 15 }, "end": { "line": 1, "character": 27 } },
                    "severity": 1,
                    "source": "sbt",
                    "message": "type mismatch"
                }],
                "reset": true
            }),
        ));
        let Translated::Diagnostics { file, items } = translated else {
            panic!("expected diagnostics")
        };
        assert_eq!(file, PathBuf::from("/tmp/hello/src/main/scala/Hello.scala"));
        assert_eq!((items[0].line, items[0].column), (2, 16));
    }

    #[test]
    fn terminal_lines_with_sbts_level_prefix_are_log_lines() {
        assert_eq!(
            log_line("[info] compiling 1 Scala source"),
            Some((LogLevel::Info, "compiling 1 Scala source"))
        );
        assert_eq!(log_line("[warn] x"), Some((LogLevel::Warning, "x")));
        assert_eq!(log_line("[error] "), Some((LogLevel::Error, "")));
        assert_eq!(log_line("[success] Total time: 1 s"), None);
        assert_eq!(log_line("  + example test that succeeds 0.005s"), None);
        assert_eq!(log_line("MySuite:"), None);
        assert_eq!(log_line("[info]"), None);
    }

    #[test]
    fn empty_diagnostics_are_kept_and_unknown_methods_ignored() {
        let translated = translate(&notification(
            "textDocument/publishDiagnostics",
            json!({ "uri": "file:///x.scala", "diagnostics": [] }),
        ));
        assert!(matches!(translated, Translated::Diagnostics { items, .. } if items.is_empty()));
        assert_eq!(
            translate(&notification("sbt/exec", json!({ "status": "Processing" }))),
            Translated::Ignored
        );
        assert_eq!(
            translate(&notification("shutdown", json!({}))),
            Translated::ServerShutdown
        );
        assert_eq!(
            translate(&notification("build/logMessage", json!("not an object"))),
            Translated::Ignored
        );
    }
}
