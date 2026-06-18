//! Terminal-emulator boundary.
//!
//! This is the single module that depends on the `alacritty_terminal` crate.
//! Everything past it works against [`crate::render`] types, so the emulator can
//! be swapped without touching the rest of the codebase.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Cell as VtCell;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{
    Color as AlacColor, CursorShape as VteCursorShape, NamedColor, Processor,
};

use crate::palette;
use crate::protocol::{Attrs, CursorShape};
use crate::render::{Cursor, RowCell, Screen, Snapshot};

/// Grid dimensions handed to the emulator: the visible screen only. The
/// scrollback capacity is set separately via [`Config::scrolling_history`]; the
/// emulator's history is drained into the committed buffer after each `feed`.
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
    /// Lines that scrolled off the top of the primary screen since the last
    /// snapshot, oldest first. Drained into [`Snapshot::committed`].
    committed: Vec<Vec<RowCell>>,
}

impl Emulator {
    /// Create an emulator with an initial grid size and no scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_scrollback(cols, rows, 0)
    }

    /// Create an emulator that captures up to `scrollback` lines per `feed` as
    /// they scroll off the top of the primary screen (0 disables capture).
    pub fn with_scrollback(cols: u16, rows: u16, scrollback: usize) -> Self {
        let size = GridSize {
            cols: cols as usize,
            rows: rows as usize,
        };
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let recorder = Recorder::default();
        Self {
            term: Term::new(config, &size, recorder.clone()),
            parser: Processor::new(),
            recorder,
            committed: Vec::new(),
        }
    }

    /// Feed a chunk of PTY output into the emulator.
    ///
    /// Any lines that scroll off the top of the primary screen during this
    /// chunk are captured into the committed buffer, then the emulator's history
    /// is cleared. Draining per chunk keeps the committed count exact: the
    /// emulator's `history_size` caps at the configured capacity, so a per-frame
    /// delta would plateau and silently stop growing once full — the Host, not
    /// the Bridge, is the durable scrollback store.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);

        let history = self.term.grid().history_size();
        if history == 0 {
            return;
        }
        let grid = self.term.grid();
        let cols = grid.columns();
        // History holds the most recently committed line at `Line(-1)`; emit
        // oldest first so the Host appends in chronological order.
        let mut newly = Vec::with_capacity(history);
        for offset in (1..=history).rev() {
            let row = &grid[Line(-(offset as i32))];
            newly.push(row_to_cells(row, cols));
        }
        self.committed.append(&mut newly);
        self.term.grid_mut().clear_history();
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
            lines.push(row_to_cells(&grid[Line(line as i32)], cols));
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
            committed: std::mem::take(&mut self.committed),
        }
    }
}

/// Convert one emulator grid row into neutral [`RowCell`]s, dropping the
/// trailing spacer of a wide character (regenerated from the wide cell's width)
/// while keeping a leading wide-char spacer (a real blank column left when a
/// wide char wrapped to the next line).
fn row_to_cells(row: &Row<VtCell>, cols: usize) -> Vec<RowCell> {
    let mut cells = Vec::with_capacity(cols);
    for col in 0..cols {
        let cell = &row[Column(col)];
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
    cells
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

    fn committed_texts(snapshot: &Snapshot) -> Vec<String> {
        snapshot
            .committed
            .iter()
            .map(|line| {
                line.iter()
                    .map(|c| c.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn lines_scrolled_off_the_top_are_committed_oldest_first() {
        let mut emu = Emulator::with_scrollback(4, 2, 100);
        emu.feed(b"1\r\n2\r\n3\r\n4");
        let snap = emu.snapshot();
        // The two-row screen keeps the last rows; the earlier lines committed.
        assert_eq!(committed_texts(&snap), vec!["1", "2"]);
        assert_eq!(row_texts(&snap.screen), vec!["3   ", "4   "]);
    }

    #[test]
    fn without_scrollback_no_lines_are_committed() {
        let mut emu = Emulator::new(4, 2);
        emu.feed(b"1\r\n2\r\n3\r\n4");
        assert!(emu.snapshot().committed.is_empty());
    }

    #[test]
    fn committed_lines_drain_on_snapshot() {
        let mut emu = Emulator::with_scrollback(4, 2, 100);
        emu.feed(b"1\r\n2\r\n3\r\n4");
        assert_eq!(emu.snapshot().committed.len(), 2);
        // Draining is one-shot: a later snapshot without new scrolling is empty.
        assert!(emu.snapshot().committed.is_empty());
    }

    #[test]
    fn scrolling_continues_to_commit_past_the_history_capacity() {
        // Capacity is the per-chunk capture window, not a session cap: history is
        // drained and cleared after every feed, so commits never plateau.
        let mut emu = Emulator::with_scrollback(4, 2, 3);
        let mut all = Vec::new();
        for n in 0..10 {
            emu.feed(format!("{n}\r\n").as_bytes());
            all.extend(committed_texts(&emu.snapshot()));
        }
        // Every line that scrolled out of the 2-row screen was committed once.
        assert_eq!(all, vec!["0", "1", "2", "3", "4", "5", "6", "7", "8"]);
    }

    #[test]
    fn alt_screen_scrolling_commits_nothing() {
        // The alternate screen has no scrollback, so scrolling there never
        // commits lines — committed stays empty while `alt_screen` is active.
        let mut emu = Emulator::with_scrollback(4, 2, 100);
        emu.feed(b"\x1b[?1049h"); // enter the alternate screen
        emu.feed(b"1\r\n2\r\n3\r\n4");
        let snap = emu.snapshot();
        assert!(snap.alt_screen);
        assert!(snap.committed.is_empty());
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
