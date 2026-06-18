//! Wire framing for the two encodings.
//!
//! - **JSONL** (default): one JSON object per line, separated by `\n`.
//! - **MessagePack**: each message a self-delimiting binary value, no separator.
//!
//! Both carry the same serde types; MessagePack is more compact and cheaper to
//! parse for the high-frequency `grid_line` traffic.

use std::io::{self, BufRead, Write};

use serde::Serialize;

use crate::protocol::{Control, Event};

/// On-the-wire encoding selected for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// One JSON object per line, separated by `\n`.
    #[default]
    Jsonl,
    /// MessagePack: each message a self-delimiting binary value.
    Msgpack,
}

/// Serialize an event to a single JSONL line (without the trailing newline).
pub fn encode_event(event: &Event) -> String {
    // Events are derived from owned data, so serialization cannot fail.
    serde_json::to_string(event).expect("event serialization is infallible")
}

/// Write one event in `format`: a JSONL line, or a MessagePack value.
pub fn write_event<W: Write>(writer: &mut W, event: &Event, format: Format) -> io::Result<()> {
    match format {
        Format::Jsonl => {
            writer.write_all(encode_event(event).as_bytes())?;
            writer.write_all(b"\n")
        }
        Format::Msgpack => {
            // `with_struct_map` keeps field names, so the internally-tagged
            // enums and flattened attributes round-trip.
            let mut ser = rmp_serde::Serializer::new(&mut *writer).with_struct_map();
            event.serialize(&mut ser).map_err(io::Error::other)
        }
    }
}

/// Outcome of reading one MessagePack value from a stream.
pub enum MsgRead {
    /// A decoded message object, ready for [`classify`].
    Value(serde_json::Value),
    /// A clean end of stream at a value boundary.
    Eof,
    /// The stream desynchronized (a value could not be parsed); the binary
    /// framing cannot be resynchronized, so the caller should stop.
    Fatal(String),
}

/// Cap on a single MessagePack message, mirroring the JSONL line cap: a hostile
/// peer could otherwise declare a huge string/array/map length and exhaust
/// memory in one value. A message past the cap is `Fatal` (binary framing
/// cannot resynchronize).
const MAX_MSGPACK_MESSAGE: usize = 1 << 20; // 1 MiB

/// A reader that errors once it has yielded more than `remaining` bytes, so one
/// MessagePack value cannot grow the decode buffer without bound.
struct Capped<'a> {
    inner: &'a mut dyn BufRead,
    remaining: usize,
}

impl io::Read for Capped<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Err(io::Error::other("message exceeds maximum length"));
        }
        let cap = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..cap])?;
        self.remaining -= n;
        Ok(n)
    }
}

/// Read one MessagePack value from `reader` as a generic object, bounded to
/// [`MAX_MSGPACK_MESSAGE`] bytes.
pub fn read_msgpack(reader: &mut dyn BufRead) -> MsgRead {
    let mut capped = Capped {
        inner: reader,
        remaining: MAX_MSGPACK_MESSAGE,
    };
    match rmp_serde::from_read::<_, serde_json::Value>(&mut capped) {
        Ok(value) => MsgRead::Value(value),
        Err(rmp_serde::decode::Error::InvalidMarkerRead(e))
            if e.kind() == io::ErrorKind::UnexpectedEof =>
        {
            MsgRead::Eof
        }
        Err(err) => MsgRead::Fatal(err.to_string()),
    }
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

/// Control message types this bridge understands.
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

/// Classify a decoded message object per the protocol's error handling: an
/// event type → `direction`, a known control with bad fields → `bad_message`,
/// an unknown type → ignore, otherwise the control. Shared by both encodings.
pub fn classify(value: serde_json::Value) -> Decoded {
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

/// Decode one JSONL line: malformed JSON → `parse`, otherwise [`classify`].
pub fn decode_control(line: &str) -> Decoded {
    match serde_json::from_str(line.trim_end_matches(['\r', '\n'])) {
        Ok(value) => classify(value),
        Err(err) => Decoded::Error(error("parse", err.to_string())),
    }
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
    use serde::Serialize;

    #[test]
    fn write_event_appends_newline() {
        let mut buf = Vec::new();
        write_event(&mut buf, &Event::Flush, Format::Jsonl).unwrap();
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
        // Forward compatibility: a future control type the bridge doesn't know.
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

    #[test]
    fn msgpack_event_is_compact_and_unframed() {
        // No newline framing, and smaller than the JSONL line.
        let event = Event::GridLine {
            row: 3,
            col: 0,
            cells: vec![Cell::new("h", 7), Cell::inherit("e"), Cell::run(" ", 0, 5)],
        };
        let mut mp = Vec::new();
        write_event(&mut mp, &event, Format::Msgpack).unwrap();
        assert!(!mp.is_empty());
        assert!(!mp.contains(&b'\n'));
        assert!(mp.len() < encode_event(&event).len());
    }

    #[test]
    fn msgpack_control_roundtrips_through_read_and_classify() {
        let ctrl = Control::Resize {
            cols: 120,
            rows: 40,
        };
        let mut buf = Vec::new();
        ctrl.serialize(&mut rmp_serde::Serializer::new(&mut buf).with_struct_map())
            .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        match read_msgpack(&mut cursor) {
            MsgRead::Value(value) => match classify(value) {
                Decoded::Control(decoded) => assert_eq!(decoded, ctrl),
                other => panic!("expected a control, got {other:?}"),
            },
            _ => panic!("expected a value"),
        }
        // The stream is fully consumed — the next read is a clean EOF.
        assert!(matches!(read_msgpack(&mut cursor), MsgRead::Eof));
    }

    #[test]
    fn msgpack_event_sent_as_control_is_a_direction_error() {
        let mut buf = Vec::new();
        Event::Flush
            .serialize(&mut rmp_serde::Serializer::new(&mut buf).with_struct_map())
            .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let MsgRead::Value(value) = read_msgpack(&mut cursor) else {
            panic!("expected a value");
        };
        assert_eq!(error_code(classify(value)), "direction");
    }
}
