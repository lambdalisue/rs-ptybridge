//! End-to-end check that `--format msgpack` frames the event stream as
//! MessagePack — including that the self-delimiting stream decodes cleanly to
//! the end and that a flattened `hl_attr` round-trips.
#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

#[test]
fn msgpack_transport_emits_a_clean_msgpack_event_stream() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptybridge"))
        .args([
            "--format",
            "msgpack",
            "--cols",
            "20",
            "--rows",
            "5",
            "--", //
            "bash",
            "--norc",
            "-c",
            "printf '\\033[1;31mRED\\033[0m'",
        ])
        // Leave stdin piped and open so the control reader blocks rather than
        // hitting EOF; the session ends when the child exits, after the colored
        // frame has been emitted.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ptybridge");

    let mut out = Vec::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut out)
        .expect("read stdout");
    child.wait().expect("wait");

    // Binary MessagePack, not a JSON line.
    assert!(!out.is_empty());
    assert_ne!(out[0], b'{', "stream looks like JSON, not msgpack");

    // Decode the whole self-delimiting stream.
    let total = out.len() as u64;
    let mut cursor = std::io::Cursor::new(out);
    let mut kinds = Vec::new();
    let mut hl_attr_has_fg = false;
    while let Ok(value) = rmp_serde::from_read::<_, serde_json::Value>(&mut cursor) {
        if let Some(t) = value.get("t").and_then(|t| t.as_str()) {
            if t == "hl_attr" && value.get("fg").and_then(|f| f.as_u64()).is_some() {
                hl_attr_has_fg = true;
            }
            kinds.push(t.to_string());
        }
    }

    // Every byte parsed: the framing is intact to the end, with no trailing junk.
    assert_eq!(
        cursor.position(),
        total,
        "stream did not decode cleanly to EOF; kinds={kinds:?}"
    );
    assert_eq!(kinds.first().map(String::as_str), Some("hello"));
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("child_exit"),
        "child_exit must be the final message; kinds={kinds:?}"
    );
    // A flattened hl_attr round-tripped through MessagePack (the colored "RED").
    assert!(
        hl_attr_has_fg,
        "expected an hl_attr carrying fg; kinds={kinds:?}"
    );
}
