//! Wire protocol types.
//!
//! `PROTOCOL.md` is the authoritative specification; these types are its Rust
//! encoding. Every message is an internally tagged JSON object keyed by `t`.

use serde::de::{self, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Protocol identifier carried in `hello.proto`.
pub const PROTO: &str = "ptybridge";

/// Protocol version carried in `hello.v`.
pub const VERSION: u32 = 1;

/// One grid cell on the wire: `[text, hl_id?, repeat?]`.
///
/// Semantics match Neovim `grid_line`:
/// - `hl_id` omitted (`None`) inherits the previous cell's highlight.
/// - `repeat` (`None` = 1) compresses a run of identical cells; when present the
///   encoder also emits `hl_id`.
///
/// The cell invariant: `text` holds exactly one grapheme; a double-width
/// character is followed by an empty-text cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub hl_id: Option<u32>,
    pub repeat: Option<u32>,
}

impl Cell {
    /// A single cell with an explicit highlight id and no repeat.
    pub fn new(text: impl Into<String>, hl_id: u32) -> Self {
        Self {
            text: text.into(),
            hl_id: Some(hl_id),
            repeat: None,
        }
    }

    /// A cell that inherits the previous cell's highlight id.
    pub fn inherit(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            hl_id: None,
            repeat: None,
        }
    }

    /// A run of `repeat` identical cells with an explicit highlight id.
    pub fn run(text: impl Into<String>, hl_id: u32, repeat: u32) -> Self {
        Self {
            text: text.into(),
            hl_id: Some(hl_id),
            repeat: Some(repeat),
        }
    }

    /// Number of columns this cell advances (`repeat`, defaulting to 1).
    pub fn span(&self) -> u32 {
        self.repeat.unwrap_or(1)
    }
}

impl Serialize for Cell {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Emit the shortest array the values allow, matching Neovim semantics.
        match (self.hl_id, self.repeat) {
            (Some(hl), Some(rep)) => {
                let mut seq = serializer.serialize_seq(Some(3))?;
                seq.serialize_element(&self.text)?;
                seq.serialize_element(&hl)?;
                seq.serialize_element(&rep)?;
                seq.end()
            }
            (Some(hl), None) => {
                let mut seq = serializer.serialize_seq(Some(2))?;
                seq.serialize_element(&self.text)?;
                seq.serialize_element(&hl)?;
                seq.end()
            }
            (None, Some(rep)) => {
                // A run that inherits the previous highlight: hold the position
                // with an explicit null so `repeat` stays at index 2.
                let mut seq = serializer.serialize_seq(Some(3))?;
                seq.serialize_element(&self.text)?;
                seq.serialize_element(&Option::<u32>::None)?;
                seq.serialize_element(&rep)?;
                seq.end()
            }
            (None, None) => {
                let mut seq = serializer.serialize_seq(Some(1))?;
                seq.serialize_element(&self.text)?;
                seq.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Cell {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CellVisitor;

        impl<'de> Visitor<'de> for CellVisitor {
            type Value = Cell;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a cell array [text, hl_id?, repeat?]")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Cell, A::Error> {
                let text: String = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let hl_id: Option<u32> = seq.next_element()?.flatten();
                let repeat: Option<u32> = seq.next_element()?.flatten();
                Ok(Cell {
                    text,
                    hl_id,
                    repeat,
                })
            }
        }

        deserializer.deserialize_seq(CellVisitor)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A highlight attribute set: the source of `hl_attr` and the key the highlight
/// cache interns. Colors are `0xRRGGBB` or `None` for the terminal default;
/// boolean attributes default to false and are omitted on the wire when false.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Attrs {
    #[serde(default)]
    pub fg: Option<u32>,
    #[serde(default)]
    pub bg: Option<u32>,
    #[serde(default)]
    pub sp: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub undercurl: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strikethrough: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub blink: bool,
}

/// Cursor shape reported to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

/// Messages emitted by the bridge (Bridge → Host).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Event {
    #[serde(rename = "hello")]
    Hello {
        proto: String,
        v: u32,
        cols: u16,
        rows: u16,
        features: Vec<String>,
    },
    #[serde(rename = "grid_resize")]
    GridResize { cols: u16, rows: u16 },
    #[serde(rename = "default_colors")]
    DefaultColors { fg: u32, bg: u32, sp: u32 },
    #[serde(rename = "hl_attr")]
    HlAttr {
        id: u32,
        #[serde(flatten)]
        attrs: Attrs,
    },
    #[serde(rename = "grid_clear")]
    GridClear,
    #[serde(rename = "grid_scroll")]
    GridScroll {
        top: u16,
        bot: u16,
        left: u16,
        right: u16,
        /// Lines scrolled; positive is up.
        rows: i32,
        cols: i32,
    },
    #[serde(rename = "grid_line")]
    GridLine {
        row: u16,
        col: u16,
        cells: Vec<Cell>,
    },
    #[serde(rename = "scrollback_push")]
    ScrollbackPush {
        /// Lines that scrolled off the top of the primary screen, oldest first.
        /// Each is a `cells` array with the same shape as `grid_line.cells`.
        lines: Vec<Vec<Cell>>,
    },
    #[serde(rename = "cursor")]
    Cursor {
        row: u16,
        col: u16,
        visible: bool,
        shape: CursorShape,
    },
    #[serde(rename = "mode")]
    Mode { alt_screen: bool },
    #[serde(rename = "title")]
    Title { text: String },
    #[serde(rename = "bell")]
    Bell,
    #[serde(rename = "flush")]
    Flush,
    #[serde(rename = "child_exit")]
    ChildExit {
        code: Option<i32>,
        signal: Option<String>,
    },
    #[serde(rename = "pong")]
    Pong { id: u64 },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}

impl Event {
    /// The initial handshake with the protocol's own identity and version.
    pub fn hello(cols: u16, rows: u16, features: Vec<String>) -> Self {
        Event::Hello {
            proto: PROTO.to_string(),
            v: VERSION,
            cols,
            rows,
            features,
        }
    }
}

/// Messages received from the host (Host → Bridge).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Control {
    #[serde(rename = "input")]
    Input {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enc: Option<String>,
        data: String,
    },
    #[serde(rename = "resize")]
    Resize { cols: u16, rows: u16 },
    #[serde(rename = "signal")]
    Signal { name: String },
    #[serde(rename = "ping")]
    Ping { id: u64 },
    #[serde(rename = "shutdown")]
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_json(cell: &Cell) -> String {
        serde_json::to_string(cell).unwrap()
    }

    #[test]
    fn cell_with_hl_serializes_as_two_element_array() {
        assert_eq!(cell_json(&Cell::new("h", 7)), r#"["h",7]"#);
    }

    #[test]
    fn cell_inherit_serializes_as_single_element_array() {
        assert_eq!(cell_json(&Cell::inherit("e")), r#"["e"]"#);
    }

    #[test]
    fn cell_run_serializes_as_three_element_array() {
        assert_eq!(cell_json(&Cell::run(" ", 0, 5)), r#"[" ",0,5]"#);
    }

    #[test]
    fn cell_roundtrips_through_all_shapes() {
        for cell in [Cell::new("h", 7), Cell::inherit("e"), Cell::run(" ", 0, 5)] {
            let json = serde_json::to_string(&cell).unwrap();
            let back: Cell = serde_json::from_str(&json).unwrap();
            assert_eq!(cell, back);
        }
    }

    #[test]
    fn cell_deserializes_null_hl_as_inherit() {
        let cell: Cell = serde_json::from_str(r#"["l",null,3]"#).unwrap();
        assert_eq!(
            cell,
            Cell {
                text: "l".into(),
                hl_id: None,
                repeat: Some(3)
            }
        );
    }

    #[test]
    fn grid_line_matches_spec_example() {
        let event = Event::GridLine {
            row: 3,
            col: 0,
            cells: vec![
                Cell::new("h", 7),
                Cell::inherit("e"),
                Cell::run("l", 7, 3),
                Cell::run(" ", 0, 5),
            ],
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"t":"grid_line","row":3,"col":0,"cells":[["h",7],["e"],["l",7,3],[" ",0,5]]}"#
        );
    }

    #[test]
    fn scrollback_push_serializes_lines_as_cell_arrays() {
        let event = Event::ScrollbackPush {
            lines: vec![
                vec![Cell::new("o", 7), Cell::run("l", 7, 2), Cell::inherit("d")],
                vec![Cell::new("n", 0)],
            ],
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"t":"scrollback_push","lines":[[["o",7],["l",7,2],["d"]],[["n",0]]]}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn hello_carries_proto_and_version() {
        let json = serde_json::to_string(&Event::hello(80, 24, vec![])).unwrap();
        assert_eq!(
            json,
            r#"{"t":"hello","proto":"ptybridge","v":1,"cols":80,"rows":24,"features":[]}"#
        );
    }

    #[test]
    fn hl_attr_flattens_attrs_and_omits_false_flags() {
        let event = Event::HlAttr {
            id: 7,
            attrs: Attrs {
                fg: Some(0x00ff00),
                bg: None,
                sp: None,
                bold: true,
                ..Attrs::default()
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"t":"hl_attr","id":7,"fg":65280,"bg":null,"sp":null,"bold":true}"#
        );
        // Flatten must also roundtrip back (reference clients deserialize Events).
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }

    #[test]
    fn cursor_shape_serializes_lowercase() {
        let event = Event::Cursor {
            row: 1,
            col: 2,
            visible: true,
            shape: CursorShape::Block,
        };
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"t":"cursor","row":1,"col":2,"visible":true,"shape":"block"}"#
        );
    }

    #[test]
    fn flush_is_a_bare_tagged_object() {
        assert_eq!(
            serde_json::to_string(&Event::Flush).unwrap(),
            r#"{"t":"flush"}"#
        );
    }

    #[test]
    fn control_input_roundtrips_without_enc() {
        let json = r#"{"t":"input","data":"abc"}"#;
        let ctrl: Control = serde_json::from_str(json).unwrap();
        assert_eq!(
            ctrl,
            Control::Input {
                enc: None,
                data: "abc".into()
            }
        );
        assert_eq!(serde_json::to_string(&ctrl).unwrap(), json);
    }

    #[test]
    fn control_resize_deserializes_from_spec_example() {
        let ctrl: Control = serde_json::from_str(r#"{"t":"resize","cols":120,"rows":40}"#).unwrap();
        assert_eq!(
            ctrl,
            Control::Resize {
                cols: 120,
                rows: 40
            }
        );
    }
}
