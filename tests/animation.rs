//! Multi-frame rendering: redraw-in-place animations and streaming scroll, fed
//! as real ANSI through the emulator. These pin the diff renderer's promise that
//! a frame re-emits only what changed — a spinner repaints one cell, and a
//! scrolling stream collapses into `grid_scroll` instead of a full repaint.

use ptybridge::protocol::Event;
use ptybridge::render::Renderer;
use ptybridge::term::Emulator;

fn grid_line_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::GridLine { .. }))
        .count()
}

fn has(events: &[Event], pred: impl Fn(&Event) -> bool) -> bool {
    events.iter().any(pred)
}

#[test]
fn a_spinner_repaints_only_its_one_cell_each_frame() {
    let mut emu = Emulator::new(12, 2);
    let mut renderer = Renderer::default();

    // Baseline frame: the spinner's first glyph.
    emu.feed(b"-");
    let _ = renderer.frame(&emu.snapshot());

    // Each subsequent step returns the cursor to column 0 and overwrites the
    // single spinner cell. Only that row should be re-sent — no scroll, no clear.
    for glyph in [b'\\', b'|', b'/', b'-'] {
        emu.feed(&[b'\r', glyph]);
        let events = renderer.frame(&emu.snapshot());
        assert_eq!(
            grid_line_count(&events),
            1,
            "a spinner step should repaint exactly one row: {events:?}"
        );
        assert!(
            !has(&events, |e| matches!(e, Event::GridScroll { .. })),
            "an in-place spinner must not scroll: {events:?}"
        );
        assert!(
            !has(&events, |e| matches!(e, Event::GridClear)),
            "an in-place spinner must not clear: {events:?}"
        );
    }
}

#[test]
fn a_streaming_log_scrolls_instead_of_repainting_every_row() {
    let rows = 4u16;
    let mut emu = Emulator::new(6, rows);
    let mut renderer = Renderer::default();

    // Fill the screen, then stream more lines so the content scrolls upward.
    emu.feed(b"l0\r\nl1\r\nl2\r\nl3");
    let _ = renderer.frame(&emu.snapshot());

    let steps = 10;
    let mut scrolled_frames = 0;
    let mut total_grid_lines = 0;
    for n in 4..4 + steps {
        emu.feed(format!("\r\nl{n}").as_bytes());
        let events = renderer.frame(&emu.snapshot());
        if has(&events, |e| matches!(e, Event::GridScroll { .. })) {
            scrolled_frames += 1;
        }
        total_grid_lines += grid_line_count(&events);
    }

    // Nearly every step is a one-line scroll, not a full repaint.
    assert!(
        scrolled_frames >= steps - 1,
        "expected ~{steps} scrolling frames, got {scrolled_frames}"
    );
    // A full repaint would be steps * rows grid_lines; scrolling keeps it small.
    assert!(
        total_grid_lines < (steps * rows as usize),
        "expected far fewer than {} grid_lines, got {total_grid_lines}",
        steps * rows as usize
    );
}
