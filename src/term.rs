//! Terminal-emulator boundary.
//!
//! This is the single module that depends on the `alacritty_terminal` crate.
//! Everything past it works against [`crate::render`] types, so the emulator can
//! be swapped without touching the rest of the codebase.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AlacColor, CursorShape as VteCursorShape, NamedColor, Processor,
};

use crate::palette;
use crate::protocol::{Attrs, CursorShape};
use crate::render::{Cursor, RowCell, Screen, Snapshot};

/// Grid dimensions handed to the emulator. Scrollback is disabled: the visible
/// screen is the whole buffer, which is all the protocol streams.
#[derive(Clone, Copy)]
struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Side effects the emulator reports through its event listener, accumulated
/// between snapshots.
#[derive(Default)]
struct RecorderState {
    pending_title: Option<String>,
    bell: bool,
    pty_writes: Vec<u8>,
}

/// Event listener that records title/bell/PTY-write events. `alacritty_terminal`
/// hands events to `&self`, so the state is behind interior mutability; the
/// emulator lives on one thread, so `Rc<RefCell<…>>` is sufficient.
#[derive(Clone, Default)]
struct Recorder(Rc<RefCell<RecorderState>>);

impl EventListener for Recorder {
    fn send_event(&self, event: AlacEvent) {
        let mut state = self.0.borrow_mut();
        match event {
            AlacEvent::Title(title) => state.pending_title = Some(title),
            AlacEvent::ResetTitle => state.pending_title = Some(String::new()),
            AlacEvent::Bell => state.bell = true,
            AlacEvent::PtyWrite(text) => state.pty_writes.extend_from_slice(text.as_bytes()),
            _ => {}
        }
    }
}

/// A terminal emulator fed raw PTY bytes, exposing its screen as a [`Snapshot`].
pub struct Emulator {
    term: Term<Recorder>,
    parser: Processor,
    recorder: Recorder,
}

impl Emulator {
    /// Create an emulator with an initial grid size.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = GridSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let config = Config {
            scrolling_history: 0,
            ..Config::default()
        };
        let recorder = Recorder::default();
        Self {
            term: Term::new(config, &size, recorder.clone()),
            parser: Processor::new(),
            recorder,
        }
    }

    /// Feed a chunk of PTY output into the emulator.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Resize the grid; the child observes this via `SIGWINCH` separately.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(GridSize {
            cols: cols as usize,
            rows: rows as usize,
        });
    }

    /// Take any bytes the emulator wants written back to the PTY (e.g. responses
    /// to cursor-position queries).
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.recorder.0.borrow_mut().pty_writes)
    }

    /// Snapshot the current visible screen plus cursor and side state.
    pub fn snapshot(&mut self) -> Snapshot {
        let (cursor_point, cursor_shape, mode) = {
            let content = self.term.renderable_content();
            (content.cursor.point, content.cursor.shape, content.mode)
        };

        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();

        let mut lines = Vec::with_capacity(rows);
        for line in 0..rows {
            let row = &grid[Line(line as i32)];
            let mut cells = Vec::with_capacity(cols);
            for col in 0..cols {
                let cell = &row[Column(col)];
                // The trailing half of a wide char is regenerated from the wide
                // cell's width, so drop it. A leading wide-char spacer is a real
                // blank column (a wide char wrapped to the next line), so keep it.
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut text = String::new();
                text.push(cell.c);
                if let Some(zerowidth) = cell.zerowidth() {
                    text.extend(zerowidth.iter());
                }

                let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                };
                cells.push(RowCell {
                    text,
                    attrs: cell_attrs(cell),
                    width,
                });
            }
            lines.push(cells);
        }

        let cursor = Cursor {
            row: cursor_point.line.0.max(0) as u16,
            col: cursor_point.column.0 as u16,
            visible: cursor_shape != VteCursorShape::Hidden,
            shape: map_shape(cursor_shape),
        };

        let (title, bell) = {
            let mut state = self.recorder.0.borrow_mut();
            (state.pending_title.take(), std::mem::take(&mut state.bell))
        };

        Snapshot {
            screen: Screen {
                cols: cols as u16,
                rows: rows as u16,
                lines,
            },
            cursor,
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            title,
            bell,
        }
    }
}

/// Map an emulator color into the neutral palette color, so the palette never
/// sees an `alacritty_terminal` type. Named foreground/background (and cursor)
/// stay default; the 16 ANSI names and their dim variants map to palette indices.
fn palette_color(color: AlacColor) -> palette::Color {
    use NamedColor::*;
    match color {
        AlacColor::Spec(rgb) => palette::Color::Rgb(rgb.r, rgb.g, rgb.b),
        AlacColor::Indexed(index) => palette::Color::Indexed(index),
        AlacColor::Named(named) => {
            let index = match named {
                Black => 0,
                Red => 1,
                Green => 2,
                Yellow => 3,
                Blue => 4,
                Magenta => 5,
                Cyan => 6,
                White => 7,
                BrightBlack => 8,
                BrightRed => 9,
                BrightGreen => 10,
                BrightYellow => 11,
                BrightBlue => 12,
                BrightMagenta => 13,
                BrightCyan => 14,
                BrightWhite => 15,
                DimBlack => 0,
                DimRed => 1,
                DimGreen => 2,
                DimYellow => 3,
                DimBlue => 4,
                DimMagenta => 5,
                DimCyan => 6,
                DimWhite => 7,
                Foreground | Background | Cursor | BrightForeground | DimForeground => {
                    return palette::Color::Default;
                }
            };
            palette::Color::Indexed(index)
        }
    }
}

/// Resolve a cell's colors and flags into protocol highlight attributes.
fn cell_attrs(cell: &alacritty_terminal::term::cell::Cell) -> Attrs {
    let f = cell.flags;
    Attrs {
        fg: palette::resolve(palette_color(cell.fg)),
        bg: palette::resolve(palette_color(cell.bg)),
        sp: None,
        bold: f.contains(Flags::BOLD),
        italic: f.contains(Flags::ITALIC),
        underline: f.intersects(
            Flags::UNDERLINE
                | Flags::DOUBLE_UNDERLINE
                | Flags::DOTTED_UNDERLINE
                | Flags::DASHED_UNDERLINE,
        ),
        undercurl: f.contains(Flags::UNDERCURL),
        reverse: f.contains(Flags::INVERSE),
        strikethrough: f.contains(Flags::STRIKEOUT),
        dim: f.contains(Flags::DIM),
        blink: false,
    }
}

fn map_shape(shape: VteCursorShape) -> CursorShape {
    match shape {
        VteCursorShape::Underline => CursorShape::Underline,
        VteCursorShape::Beam => CursorShape::Bar,
        VteCursorShape::Block | VteCursorShape::HollowBlock | VteCursorShape::Hidden => {
            CursorShape::Block
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_texts(screen: &Screen) -> Vec<String> {
        screen
            .lines
            .iter()
            .map(|line| line.iter().map(|c| c.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn plain_text_lands_on_the_first_row() {
        let mut emu = Emulator::new(10, 2);
        emu.feed(b"hello");
        let screen = emu.snapshot().screen;
        assert_eq!(screen.cols, 10);
        assert_eq!(screen.rows, 2);
        assert_eq!(row_texts(&screen)[0], "hello     ");
    }

    #[test]
    fn carriage_return_and_newline_move_the_cursor() {
        let mut emu = Emulator::new(5, 2);
        emu.feed(b"ab\r\ncd");
        let texts = row_texts(&emu.snapshot().screen);
        assert_eq!(texts[0], "ab   ");
        assert_eq!(texts[1], "cd   ");
    }

    #[test]
    fn wide_char_occupies_two_columns_with_a_spacer() {
        let mut emu = Emulator::new(6, 1);
        emu.feed("あa".as_bytes());
        let line = &emu.snapshot().screen.lines[0];
        assert_eq!(line[0].text, "あ");
        assert_eq!(line[0].width, 2);
        assert_eq!(line[1].text, "a");
        let span: u32 = line.iter().map(|c| c.width as u32).sum();
        assert_eq!(span, 6);
    }

    #[test]
    fn wide_char_wrapping_at_right_edge_keeps_columns_aligned() {
        // Three columns filled, then a wide char that cannot fit the last column
        // wraps to the next line, leaving a blank in the final column. Every row
        // must still account for exactly `cols` columns.
        let mut emu = Emulator::new(4, 2);
        emu.feed("aaaあ".as_bytes());
        let screen = emu.snapshot().screen;
        for (row, line) in screen.lines.iter().enumerate() {
            let span: u32 = line.iter().map(|c| c.width as u32).sum();
            assert_eq!(span, screen.cols as u32, "row {row} spans {span}, not cols");
        }
    }

    #[test]
    fn sgr_colors_and_bold_become_attributes() {
        let mut emu = Emulator::new(4, 1);
        // Bold + red foreground, one char, then reset.
        emu.feed(b"\x1b[1;31mX\x1b[0m");
        let line = &emu.snapshot().screen.lines[0];
        assert_eq!(line[0].text, "X");
        assert!(line[0].attrs.bold);
        assert_eq!(line[0].attrs.fg, Some(0x800000));
    }

    #[test]
    fn default_cells_carry_no_explicit_colors() {
        // A plain cell uses the named default fg/bg, which map to None so the
        // host applies its default_colors.
        let mut emu = Emulator::new(4, 1);
        emu.feed(b"a");
        let cell = &emu.snapshot().screen.lines[0][0];
        assert_eq!(cell.attrs.fg, None);
        assert_eq!(cell.attrs.bg, None);
    }

    #[test]
    fn osc_title_is_recorded_once() {
        let mut emu = Emulator::new(4, 1);
        emu.feed(b"\x1b]2;hello\x07");
        let first = emu.snapshot();
        assert_eq!(first.title.as_deref(), Some("hello"));
        // Drained: a later snapshot without a new title reports none.
        let second = emu.snapshot();
        assert_eq!(second.title, None);
    }

    #[test]
    fn widening_reflows_a_wrapped_line() {
        // A line wrapped at the old width rejoins onto one row when the grid
        // widens enough to hold it — the emulator reflows, it does not leave the
        // break frozen — and every row still accounts for exactly `cols` columns.
        let mut emu = Emulator::new(4, 3);
        emu.feed(b"abcdefgh");
        let before = row_texts(&emu.snapshot().screen);
        assert_eq!(before[0], "abcd");
        assert_eq!(before[1], "efgh");

        emu.resize(8, 3);
        let after = emu.snapshot().screen;
        assert_eq!(row_texts(&after)[0], "abcdefgh");
        for (row, line) in after.lines.iter().enumerate() {
            let span: u32 = line.iter().map(|c| c.width as u32).sum();
            assert_eq!(span, after.cols as u32, "row {row} spans {span}, not cols");
        }
    }
}
