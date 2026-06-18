//! Turn an emulated screen into protocol events.
//!
//! The neutral types here ([`RowCell`], [`Screen`], [`Snapshot`]) are the
//! boundary that keeps the vterm crate out of the rest of the codebase:
//! `term.rs` fills them, and the [`Renderer`] diffs successive snapshots into
//! the event stream, interning highlights as it goes.

use crate::hlcache::HlCache;
use crate::protocol::{Attrs, Cell, CursorShape, Event};

/// Default foreground/background the host applies to cells with no explicit
/// color. The emulator carries no theme, so the daemon picks sensible defaults.
const DEFAULT_FG: u32 = 0xd0d0d0;
const DEFAULT_BG: u32 = 0x000000;

/// One logical cell as produced by the emulator: a single grapheme cluster, its
/// highlight attributes, and the number of terminal columns it occupies (1 or 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowCell {
    pub text: String,
    pub attrs: Attrs,
    pub width: u8,
}

impl RowCell {
    /// A single-width cell with default attributes (test/helper convenience).
    pub fn narrow(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: Attrs::default(),
            width: 1,
        }
    }

    /// A double-width cell with default attributes.
    pub fn wide(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: Attrs::default(),
            width: 2,
        }
    }

    /// Set the highlight attributes (builder style).
    pub fn with_attrs(mut self, attrs: Attrs) -> Self {
        self.attrs = attrs;
        self
    }
}

/// A full emulated screen: `rows` lines of logical cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<Vec<RowCell>>,
}

/// Cursor state reported alongside a screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

/// Everything observed from one emulation step: the screen plus side state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub screen: Screen,
    pub cursor: Cursor,
    pub alt_screen: bool,
    pub title: Option<String>,
    pub bell: bool,
}

/// Run-length compress columns `(text, hl_id)` into wire cells, emitting the
/// shortest cell shape (inherit when the highlight matches the previous cell).
fn compress(columns: &[(String, u32)]) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut prev_hl: Option<u32> = None;
    let mut i = 0;
    while i < columns.len() {
        let (text, hl) = &columns[i];
        let mut run = 1;
        while i + run < columns.len() && columns[i + run] == columns[i] {
            run += 1;
        }

        if run > 1 {
            cells.push(Cell::run(text.clone(), *hl, run as u32));
        } else if prev_hl == Some(*hl) {
            cells.push(Cell::inherit(text.clone()));
        } else {
            cells.push(Cell::new(text.clone(), *hl));
        }

        prev_hl = Some(*hl);
        i += run;
    }
    cells
}

/// Expand a row to one `(text, hl_id)` column per terminal column, interning
/// highlights and materializing the empty spacer that follows a wide cell.
fn expand(row: &[RowCell], hlcache: &mut HlCache, hl_attrs: &mut Vec<Event>) -> Vec<(String, u32)> {
    let mut columns = Vec::with_capacity(row.len());
    for cell in row {
        let id = hlcache.intern(&cell.attrs, hl_attrs);
        columns.push((cell.text.clone(), id));
        if cell.width == 2 {
            columns.push((String::new(), id));
        }
    }
    columns
}

/// Detect a full-screen upward scroll: the shift `k` for which the most current
/// rows equal the previous screen shifted up by `k` lines. The baseline diff
/// re-emits whatever does not match (a terminal writes the new line at the
/// second-to-last row and leaves the cursor's blank line at the bottom, so the
/// shift is rarely exact), so any `k` is correct — this just picks the best and
/// only commits when a scroll spares at least half the rows.
fn detect_scroll_up(prev: &Screen, cur: &Screen) -> Option<u16> {
    let n = cur.lines.len();
    if n < 2 || prev.lines.len() != n || cur.lines == prev.lines {
        return None;
    }

    let mut best_k = 0;
    let mut best_matches = 0;
    for k in 1..n {
        let matches = (0..n - k)
            .filter(|&r| cur.lines[r] == prev.lines[r + k])
            .count();
        if matches > best_matches {
            best_matches = matches;
            best_k = k;
        }
    }

    // The shift must explain the frame better than staying put — otherwise blank
    // rows (which match any shift) trigger a spurious scroll when, say, a line is
    // edited in place on an otherwise-empty screen.
    let identity = (0..n).filter(|&r| cur.lines[r] == prev.lines[r]).count();
    if best_k > 0 && best_matches > identity && best_matches * 2 >= n {
        Some(best_k as u16)
    } else {
        None
    }
}

/// The previous-screen row the host holds at `row` when diffing.
///
/// After scrolling up by `k`, the host's top `n - k` rows are `prev[row + k]`
/// and its bottom `k` rows keep their stale `prev[row]` content (the protocol
/// does not clear the vacated region).
fn baseline_row(prev: &Screen, row: usize, scroll: Option<u16>) -> Option<&Vec<RowCell>> {
    match scroll {
        Some(k) if row + (k as usize) < prev.lines.len() => prev.lines.get(row + k as usize),
        _ => prev.lines.get(row),
    }
}

/// Stateful renderer: diffs successive snapshots and interns highlights for the
/// lifetime of a connection.
#[derive(Default)]
pub struct Renderer {
    prev: Option<Screen>,
    hlcache: HlCache,
    prev_cursor: Option<Cursor>,
    prev_alt: Option<bool>,
    defaults_sent: bool,
}

impl Renderer {
    /// Render one frame: only changed rows become `grid_line`, new highlights
    /// are defined first, and messages follow the protocol's frame order.
    pub fn frame(&mut self, snapshot: &Snapshot) -> Vec<Event> {
        let screen = &snapshot.screen;
        let resized = self
            .prev
            .as_ref()
            .is_some_and(|p| p.cols != screen.cols || p.rows != screen.rows);
        let full = self.prev.is_none() || resized;

        // Detect a full-screen scroll so its rows become one grid_scroll instead
        // of repainting every shifted line.
        let scroll = if full {
            None
        } else {
            self.prev
                .as_ref()
                .and_then(|prev| detect_scroll_up(prev, screen))
        };

        // Build changed rows first; interning pushes new hl_attr definitions.
        // After a grid_scroll the host's rows are prev shifted up by `k`, with
        // the bottom `k` rows left stale, so diff each row against that baseline.
        let mut hl_attrs = Vec::new();
        let mut lines = Vec::new();
        for (row, line) in screen.lines.iter().enumerate() {
            let unchanged = !full
                && self
                    .prev
                    .as_ref()
                    .and_then(|prev| baseline_row(prev, row, scroll))
                    .is_some_and(|prev_line| prev_line == line);
            if unchanged {
                continue;
            }
            let cells = compress(&expand(line, &mut self.hlcache, &mut hl_attrs));
            lines.push(Event::GridLine {
                row: row as u16,
                col: 0,
                cells,
            });
        }

        // Assemble in the order PROTOCOL.md frame ordering mandates.
        let mut events = Vec::new();
        if resized {
            events.push(Event::GridResize {
                cols: screen.cols,
                rows: screen.rows,
            });
        }
        if !self.defaults_sent {
            events.push(Event::DefaultColors {
                fg: DEFAULT_FG,
                bg: DEFAULT_BG,
                sp: DEFAULT_FG,
            });
            self.defaults_sent = true;
        }
        events.append(&mut hl_attrs);
        if full {
            events.push(Event::GridClear);
        }
        if let Some(k) = scroll {
            events.push(Event::GridScroll {
                top: 0,
                bot: screen.rows,
                left: 0,
                right: screen.cols,
                rows: k as i32,
                cols: 0,
            });
        }
        events.append(&mut lines);

        if self.prev_cursor != Some(snapshot.cursor) {
            let c = snapshot.cursor;
            events.push(Event::Cursor {
                row: c.row,
                col: c.col,
                visible: c.visible,
                shape: c.shape,
            });
            self.prev_cursor = Some(c);
        }
        if self.prev_alt != Some(snapshot.alt_screen) {
            events.push(Event::Mode {
                alt_screen: snapshot.alt_screen,
            });
            self.prev_alt = Some(snapshot.alt_screen);
        }
        if let Some(title) = &snapshot.title {
            events.push(Event::Title {
                text: title.clone(),
            });
        }
        if snapshot.bell {
            events.push(Event::Bell);
        }

        events.push(Event::Flush);
        self.prev = Some(screen.clone());
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> Cursor {
        Cursor {
            row: 0,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
        }
    }

    fn snapshot(lines: Vec<Vec<RowCell>>, cols: u16) -> Snapshot {
        let rows = lines.len() as u16;
        Snapshot {
            screen: Screen { cols, rows, lines },
            cursor: cursor(),
            alt_screen: false,
            title: None,
            bell: false,
        }
    }

    fn grid_lines(events: &[Event]) -> Vec<&Event> {
        events
            .iter()
            .filter(|e| matches!(e, Event::GridLine { .. }))
            .collect()
    }

    #[test]
    fn compress_collapses_blank_runs_and_inherits_hl() {
        let columns = vec![
            ("h".to_string(), 7),
            ("i".to_string(), 7),
            (" ".to_string(), 0),
            (" ".to_string(), 0),
        ];
        assert_eq!(
            compress(&columns),
            vec![Cell::new("h", 7), Cell::inherit("i"), Cell::run(" ", 0, 2)]
        );
    }

    #[test]
    fn expand_materializes_wide_spacer() {
        let mut cache = HlCache::default();
        let mut hl = Vec::new();
        let columns = expand(
            &[RowCell::wide("あ"), RowCell::narrow("x")],
            &mut cache,
            &mut hl,
        );
        assert_eq!(
            columns,
            vec![
                ("あ".to_string(), 0),
                (String::new(), 0),
                ("x".to_string(), 0)
            ]
        );
    }

    #[test]
    fn column_advance_equals_input_width() {
        let mut cache = HlCache::default();
        let mut hl = Vec::new();
        let row = vec![
            RowCell::narrow("a"),
            RowCell::wide("日"),
            RowCell::wide("本"),
            RowCell::narrow(" "),
        ];
        let cells = compress(&expand(&row, &mut cache, &mut hl));
        let span: u32 = cells.iter().map(Cell::span).sum();
        let width: u32 = row.iter().map(|c| c.width as u32).sum();
        assert_eq!(span, width);
        assert_eq!(span, 6);
    }

    #[test]
    fn first_frame_is_a_full_redraw_with_defaults() {
        let mut renderer = Renderer::default();
        let events = renderer.frame(&snapshot(
            vec![vec![RowCell::narrow("a")], vec![RowCell::narrow("b")]],
            1,
        ));
        assert!(matches!(events[0], Event::DefaultColors { .. }));
        assert!(events.iter().any(|e| matches!(e, Event::GridClear)));
        assert_eq!(grid_lines(&events).len(), 2);
        assert_eq!(*events.last().unwrap(), Event::Flush);
    }

    #[test]
    fn unchanged_screen_emits_no_grid_lines_on_resend() {
        let mut renderer = Renderer::default();
        let snap = snapshot(vec![vec![RowCell::narrow("a")]], 1);
        renderer.frame(&snap);
        let second = renderer.frame(&snap);
        assert!(grid_lines(&second).is_empty());
        assert!(!second.iter().any(|e| matches!(e, Event::GridClear)));
        assert_eq!(*second.last().unwrap(), Event::Flush);
    }

    #[test]
    fn only_changed_rows_are_resent() {
        let mut renderer = Renderer::default();
        let first = snapshot(
            vec![vec![RowCell::narrow("a")], vec![RowCell::narrow("b")]],
            1,
        );
        renderer.frame(&first);
        let second = snapshot(
            vec![vec![RowCell::narrow("a")], vec![RowCell::narrow("X")]],
            1,
        );
        let events = renderer.frame(&second);
        let lines = grid_lines(&events);
        assert_eq!(lines.len(), 1);
        assert!(matches!(lines[0], Event::GridLine { row: 1, .. }));
    }

    #[test]
    fn new_highlight_is_defined_before_the_grid_line_that_uses_it() {
        let mut renderer = Renderer::default();
        let red = Attrs {
            fg: Some(0xff0000),
            ..Attrs::default()
        };
        let events = renderer.frame(&snapshot(
            vec![vec![RowCell::narrow("a").with_attrs(red)]],
            1,
        ));
        let hl_pos = events
            .iter()
            .position(|e| matches!(e, Event::HlAttr { .. }))
            .expect("hl_attr present");
        let line_pos = events
            .iter()
            .position(|e| matches!(e, Event::GridLine { .. }))
            .expect("grid_line present");
        assert!(hl_pos < line_pos);
    }

    fn lines(texts: &[&str]) -> Vec<Vec<RowCell>> {
        texts
            .iter()
            .map(|t| t.chars().map(|c| RowCell::narrow(c.to_string())).collect())
            .collect()
    }

    #[test]
    fn scroll_up_by_one_emits_grid_scroll_and_only_the_new_row() {
        let mut renderer = Renderer::default();
        renderer.frame(&snapshot(lines(&["a", "b", "c"]), 1));
        // Content scrolled up by one: a is gone, d appears at the bottom.
        let events = renderer.frame(&snapshot(lines(&["b", "c", "d"]), 1));

        let scroll = events
            .iter()
            .find_map(|e| match e {
                Event::GridScroll { rows, top, bot, .. } => Some((*rows, *top, *bot)),
                _ => None,
            })
            .expect("grid_scroll emitted");
        assert_eq!(scroll, (1, 0, 3));

        // Only the newly exposed bottom row is repainted.
        let grid = grid_lines(&events);
        assert_eq!(grid.len(), 1);
        assert!(matches!(grid[0], Event::GridLine { row: 2, .. }));

        // grid_scroll must precede the grid_line (frame ordering).
        let scroll_pos = events
            .iter()
            .position(|e| matches!(e, Event::GridScroll { .. }))
            .unwrap();
        let line_pos = events
            .iter()
            .position(|e| matches!(e, Event::GridLine { .. }))
            .unwrap();
        assert!(scroll_pos < line_pos);
    }

    #[test]
    fn sustained_scrolling_sends_far_fewer_grid_lines_than_full_repaint() {
        // A 10-row screen scrolling one line at a time for many frames: with
        // grid_scroll each frame repaints ~one new row instead of all ten.
        let rows = 10usize;
        let frames = 30usize;
        let mut renderer = Renderer::default();
        let mut grid_line_total = 0;
        for f in 0..frames {
            let window: Vec<Vec<RowCell>> = (f..f + rows)
                .map(|n| {
                    format!("line{n}")
                        .chars()
                        .map(|c| RowCell::narrow(c.to_string()))
                        .collect()
                })
                .collect();
            let events = renderer.frame(&snapshot(window, 6));
            grid_line_total += grid_lines(&events).len();
        }
        // Full repaint would be frames * rows; scrolling keeps it near frames.
        assert!(
            grid_line_total < frames * 2,
            "expected far fewer than {} grid_lines, got {grid_line_total}",
            frames * rows
        );
    }

    #[test]
    fn unchanged_screen_does_not_emit_grid_scroll() {
        let mut renderer = Renderer::default();
        let snap = snapshot(lines(&["a", "b", "c"]), 1);
        renderer.frame(&snap);
        let events = renderer.frame(&snap);
        assert!(!events.iter().any(|e| matches!(e, Event::GridScroll { .. })));
        assert!(grid_lines(&events).is_empty());
    }

    #[test]
    fn in_place_edit_on_a_mostly_blank_screen_does_not_scroll() {
        // Content in the top row, blanks below (a shell prompt). Editing the top
        // row must not be mistaken for a scroll just because the blanks shift.
        let mut renderer = Renderer::default();
        renderer.frame(&snapshot(lines(&["$ ", "", "", "", "", ""]), 4));
        let events = renderer.frame(&snapshot(lines(&["$ x", "", "", "", "", ""]), 4));
        assert!(!events.iter().any(|e| matches!(e, Event::GridScroll { .. })));
        let grid = grid_lines(&events);
        assert_eq!(grid.len(), 1);
        assert!(matches!(grid[0], Event::GridLine { row: 0, .. }));
    }

    /// Apply one frame's events to a host-side row buffer, mirroring how a host
    /// renders: scroll shifts rows up and leaves the bottom stale, then each
    /// grid_line overwrites a row's text.
    fn apply(host: &mut [String], events: &[Event]) {
        for event in events {
            match event {
                Event::GridScroll { rows: k, .. } => {
                    let k = *k as usize;
                    for r in 0..host.len() {
                        if r + k < host.len() {
                            host[r] = host[r + k].clone();
                        }
                        // bottom k rows keep their (now stale) content
                    }
                }
                Event::GridLine { row, cells, .. } => {
                    let mut text = String::new();
                    for cell in cells {
                        for _ in 0..cell.span() {
                            text.push_str(&cell.text);
                        }
                    }
                    host[*row as usize] = text;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn emitted_events_reconstruct_the_screen_across_scrolls() {
        // Drive several frames including a clean scroll and the real-terminal
        // pattern (new line at row n-2, blank cursor line at the bottom), then
        // verify the host buffer rebuilt purely from events equals each screen.
        let frames = [
            lines(&["a", "b", "c", "d"]),
            lines(&["b", "c", "d", "e"]), // clean scroll: new line at bottom
            lines(&["c", "d", "f", ""]),  // new line at row n-2, blank below
            lines(&["c", "d", "f", "g"]), // fill the blank
        ];
        let mut renderer = Renderer::default();
        let mut host = vec![String::new(); 4];
        for frame in &frames {
            let events = renderer.frame(&snapshot(frame.clone(), 1));
            apply(&mut host, &events);
            let expected: Vec<String> = frame
                .iter()
                .map(|row| row.iter().map(|c| c.text.clone()).collect())
                .collect();
            assert_eq!(host, expected);
        }
    }

    #[test]
    fn cursor_move_without_cell_change_still_emits_cursor() {
        let mut renderer = Renderer::default();
        let mut snap = snapshot(vec![vec![RowCell::narrow("a")]], 1);
        renderer.frame(&snap);
        snap.cursor.col = 5;
        let events = renderer.frame(&snap);
        assert!(grid_lines(&events).is_empty());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Cursor { col: 5, .. }))
        );
    }
}
