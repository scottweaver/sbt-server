//! Answers to the terminal queries sbt sends an attached client. We are a
//! batch client with no terminal, so every answer describes a dumb 80x24
//! one. What matters is that each query gets an answer at all: sbt's
//! `NetworkTerminal` blocks until one arrives (five seconds for the
//! properties query it sends right after `sbt/attach`, and with no timeout
//! for the capability query).

use serde_json::{Value, json};

pub const WIDTH: u32 = 80;
pub const HEIGHT: u32 = 24;

/// The reply for one of sbt's terminal requests, or `None` for anything
/// that is not one (the connection then declines it).
#[must_use]
pub fn answer(method: &str, _params: &Value) -> Option<Value> {
    match method {
        "sbt/terminalPropertiesQuery" => Some(json!({
            "width": WIDTH,
            "height": HEIGHT,
            "isAnsiSupported": false,
            "isColorEnabled": false,
            "isSupershellEnabled": false,
            "isEchoEnabled": false,
        })),
        "sbt/terminalCapabilities" => Some(json!({
            "boolean": null,
            "numeric": null,
            "string": null,
        })),
        "sbt/terminalGetSize" => Some(json!({ "width": WIDTH, "height": HEIGHT })),
        "sbt/getTerminalAttributes" => Some(json!({
            "iflag": "",
            "oflag": "",
            "cflag": "",
            "lflag": "",
            "cchars": "",
        })),
        "sbt/terminalSetSize"
        | "sbt/setTerminalAttributes"
        | "sbt/terminalSetEcho"
        | "sbt/terminalSetRawMode" => Some(json!({})),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_terminal_query_sbt_can_send_gets_a_dumb_terminal_answer() {
        let properties = answer("sbt/terminalPropertiesQuery", &json!({})).unwrap();
        assert_eq!(properties["width"], 80);
        assert_eq!(properties["isAnsiSupported"], false);
        assert_eq!(properties["isEchoEnabled"], false);
        let capabilities = answer("sbt/terminalCapabilities", &json!({ "boolean": "am" })).unwrap();
        assert!(capabilities["boolean"].is_null());
        assert_eq!(
            answer("sbt/terminalGetSize", &json!({})).unwrap()["height"],
            24
        );
        assert_eq!(
            answer("sbt/terminalSetRawMode", &json!({ "toggle": true })),
            Some(json!({}))
        );
        assert_eq!(answer("sbt/readSystemIn", &json!("")), None);
        assert_eq!(answer("sbt/exec", &json!({})), None);
    }
}
