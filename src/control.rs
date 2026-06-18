//! Pure helpers for interpreting control messages.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Decode an `input` message's payload into the bytes to write to the PTY.
///
/// `enc` is `None`/`"utf8"` for a UTF-8 string passthrough, or `"base64"` for
/// base64-encoded binary. An unknown encoding or malformed base64 is an error.
pub fn decode_input(enc: Option<&str>, data: &str) -> Result<Vec<u8>, String> {
    match enc {
        None | Some("utf8") => Ok(data.as_bytes().to_vec()),
        Some("base64") => STANDARD
            .decode(data)
            .map_err(|err| format!("invalid base64 input: {err}")),
        Some(other) => Err(format!("unknown input encoding: {other}")),
    }
}

/// Map a signal name to its number, for the signals a host may send.
pub fn signal_number(name: &str) -> Option<i32> {
    match name {
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "KILL" => Some(9),
        "TERM" => Some(15),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_passthrough_when_encoding_absent() {
        assert_eq!(decode_input(None, "ls\r").unwrap(), b"ls\r");
        assert_eq!(decode_input(Some("utf8"), "あ").unwrap(), "あ".as_bytes());
    }

    #[test]
    fn base64_is_decoded() {
        // "\x1b[A" (up arrow) base64-encoded.
        let encoded = STANDARD.encode(b"\x1b[A");
        assert_eq!(decode_input(Some("base64"), &encoded).unwrap(), b"\x1b[A");
    }

    #[test]
    fn malformed_base64_is_an_error() {
        assert!(decode_input(Some("base64"), "not base64!!!").is_err());
    }

    #[test]
    fn unknown_encoding_is_an_error() {
        assert!(decode_input(Some("rot13"), "abc").is_err());
    }

    #[test]
    fn known_signal_names_map_to_numbers() {
        assert_eq!(signal_number("INT"), Some(2));
        assert_eq!(signal_number("TERM"), Some(15));
        assert_eq!(signal_number("HUP"), Some(1));
        assert_eq!(signal_number("WINCH"), None);
    }
}
