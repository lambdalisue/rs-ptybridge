//! JSONL line framing: one JSON object per line, separated by `\n`.

use std::io::{self, Write};

use crate::protocol::{Control, Event};

/// Serialize an event to a single JSONL line (without the trailing newline).
pub fn encode_event(event: &Event) -> String {
    // Events are derived from owned data, so serialization cannot fail.
    serde_json::to_string(event).expect("event serialization is infallible")
}

/// Write an event as one JSONL line, terminated by `\n`.
pub fn write_event<W: Write>(writer: &mut W, event: &Event) -> io::Result<()> {
    writer.write_all(encode_event(event).as_bytes())?;
    writer.write_all(b"\n")
}

/// Event message types — receiving one as a control is a direction violation.
const EVENT_TYPES: &[&str] = &[
    "hello",
    "grid_resize",
    "default_colors",
    "hl_attr",
    "grid_clear",
    "grid_scroll",
    "grid_line",
    "cursor",
    "mode",
    "title",
    "bell",
    "flush",
    "child_exit",
    "pong",
    "error",
];

/// Control message types this daemon understands.
const CONTROL_TYPES: &[&str] = &["input", "resize", "signal", "ping", "shutdown"];

/// Outcome of decoding one control line, mirroring the protocol's error rules.
#[derive(Debug)]
pub enum Decoded {
    /// A well-formed, understood control message.
    Control(Control),
    /// An error to send back to the host (`parse`, `direction`, `bad_message`).
    Error(Event),
    /// An unknown message type — log and ignore (forward compatibility).
    Ignore,
}

/// Decode one line, classifying it per the protocol's error handling:
/// malformed JSON → `parse`, an event type → `direction`, a known control with
/// bad fields → `bad_message`, and an unknown type → ignore.
pub fn decode_control(line: &str) -> Decoded {
    let value: serde_json::Value = match serde_json::from_str(line.trim_end_matches(['\r', '\n'])) {
        Ok(value) => value,
        Err(err) => return Decoded::Error(error("parse", err.to_string())),
    };

    let Some(t) = value.get("t").and_then(|t| t.as_str()) else {
        return Decoded::Error(error("parse", "missing string field \"t\"".to_string()));
    };

    if EVENT_TYPES.contains(&t) {
        return Decoded::Error(error(
            "direction",
            format!("event type sent as control: {t}"),
        ));
    }

    if CONTROL_TYPES.contains(&t) {
        return match serde_json::from_value::<Control>(value) {
            Ok(control) => Decoded::Control(control),
            Err(err) => Decoded::Error(error("bad_message", err.to_string())),
        };
    }

    Decoded::Ignore
}

fn error(code: &str, message: String) -> Event {
    Event::Error {
        code: code.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Cell;

    #[test]
    fn write_event_appends_newline() {
        let mut buf = Vec::new();
        write_event(&mut buf, &Event::Flush).unwrap();
        assert_eq!(buf, b"{\"t\":\"flush\"}\n");
    }

    #[test]
    fn encode_event_has_no_embedded_newline() {
        // A newline inside string data must be JSON-escaped, never raw.
        let event = Event::GridLine {
            row: 0,
            col: 0,
            cells: vec![Cell::inherit("\n")],
        };
        let line = encode_event(&event);
        assert!(!line.contains('\n'));
        assert!(line.contains("\\n"));
    }

    fn error_code(decoded: Decoded) -> String {
        match decoded {
            Decoded::Error(Event::Error { code, .. }) => code,
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn decode_control_parses_a_valid_line() {
        match decode_control("{\"t\":\"resize\",\"cols\":120,\"rows\":40}\n") {
            Decoded::Control(ctrl) => assert_eq!(
                ctrl,
                Control::Resize {
                    cols: 120,
                    rows: 40
                }
            ),
            other => panic!("expected a control, got {other:?}"),
        }
    }

    #[test]
    fn malformed_line_is_a_parse_error() {
        assert_eq!(error_code(decode_control("not json")), "parse");
    }

    #[test]
    fn missing_type_is_a_parse_error() {
        assert_eq!(error_code(decode_control(r#"{"cols":1}"#)), "parse");
    }

    #[test]
    fn event_type_sent_as_control_is_a_direction_error() {
        assert_eq!(
            error_code(decode_control(
                r#"{"t":"grid_line","row":0,"col":0,"cells":[]}"#
            )),
            "direction"
        );
    }

    #[test]
    fn known_control_with_bad_fields_is_a_bad_message_error() {
        assert_eq!(
            error_code(decode_control(r#"{"t":"resize","cols":"wide"}"#)),
            "bad_message"
        );
    }

    #[test]
    fn unknown_type_is_ignored() {
        // Forward compatibility: a future control type the daemon doesn't know.
        assert!(matches!(
            decode_control(r#"{"t":"frobnicate","x":1}"#),
            Decoded::Ignore
        ));
    }

    #[test]
    fn control_roundtrips_through_a_line() {
        let ctrl = Control::Input {
            enc: None,
            data: "ls\r".to_string(),
        };
        let line = serde_json::to_string(&ctrl).unwrap();
        match decode_control(&line) {
            Decoded::Control(decoded) => assert_eq!(decoded, ctrl),
            other => panic!("expected a control, got {other:?}"),
        }
    }
}
