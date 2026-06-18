//! Snapshot tests: known ANSI byte streams to the Event stream they emit.
//!
//! These pin the emulation result end-to-end (emulator + diff render). Update
//! snapshots with `cargo insta review` after an intentional change.

use ptybridge::protocol::Event;
use ptybridge::render::Renderer;
use ptybridge::term::Emulator;

/// Feed `bytes` into a fresh `cols`×`rows` emulator and render the first frame.
fn frame(cols: u16, rows: u16, bytes: &[u8]) -> Vec<Event> {
    let mut emu = Emulator::new(cols, rows);
    emu.feed(bytes);
    Renderer::default().frame(&emu.snapshot())
}

/// As [`frame`], but with a scrollback buffer so lines that leave the top of the
/// screen surface as `scrollback_push`.
fn frame_with_scrollback(cols: u16, rows: u16, bytes: &[u8]) -> Vec<Event> {
    let mut emu = Emulator::with_scrollback(cols, rows, 100);
    emu.feed(bytes);
    Renderer::default().frame(&emu.snapshot())
}

#[test]
fn plain_text() {
    insta::assert_json_snapshot!(frame(12, 2, b"hello world"));
}

#[test]
fn two_lines_via_crlf() {
    insta::assert_json_snapshot!(frame(6, 3, b"ab\r\ncd\r\nef"));
}

#[test]
fn scrollback_push_carries_lines_that_left_the_screen() {
    // A two-row screen sees three lines: "line1" scrolls off and is committed
    // as `scrollback_push` ahead of the live "line2" / "line3" body.
    insta::assert_json_snapshot!(frame_with_scrollback(6, 2, b"line1\r\nline2\r\nline3"));
}

#[test]
fn clear_then_reposition() {
    // Clear screen, home cursor, write, then move to row 2 col 3 and write.
    insta::assert_json_snapshot!(frame(8, 3, b"\x1b[2J\x1b[Htop\x1b[2;3Hmid"));
}

#[test]
fn cjk_mixed_with_ascii() {
    // Wide characters must encode as a cell plus an empty spacer cell.
    insta::assert_json_snapshot!(frame(10, 1, "ab日本x".as_bytes()));
}

#[test]
fn sgr_colors_define_highlights() {
    // Bold red, then default, then underlined blue background — exercises
    // hl_attr definitions and ordering before grid_line.
    insta::assert_json_snapshot!(frame(12, 1, b"\x1b[1;31mERR\x1b[0m \x1b[4;44mok\x1b[0m"));
}

#[test]
fn colors_256_and_truecolor() {
    // Indexed 256-color (fg 196, bg 21) then 24-bit truecolor fg — both must
    // resolve to concrete RGB highlights.
    insta::assert_json_snapshot!(frame(
        6,
        1,
        b"\x1b[38;5;196;48;5;21mA\x1b[0m\x1b[38;2;10;20;30mB\x1b[0m"
    ));
}

#[test]
fn cursor_shape_bar_is_reported() {
    // DECSCUSR 6 (steady bar) must surface as a bar cursor shape.
    insta::assert_json_snapshot!(frame(4, 1, b"\x1b[6 qX"));
}

#[test]
fn hidden_cursor_is_reported_invisible() {
    // DECTCEM reset (\x1b[?25l) hides the cursor.
    insta::assert_json_snapshot!(frame(4, 1, b"\x1b[?25lX"));
}

#[test]
fn alt_screen_mode_is_reported() {
    // Entering the alternate screen (DECSET 1049) flips the reported mode.
    insta::assert_json_snapshot!(frame(6, 2, b"\x1b[?1049hALT"));
}

#[test]
fn osc_title_is_reported() {
    // OSC 0 sets the window title alongside the screen state.
    insta::assert_json_snapshot!(frame(6, 1, b"\x1b]0;hi\x07X"));
}

#[test]
fn bell_is_reported() {
    insta::assert_json_snapshot!(frame(4, 1, b"\x07X"));
}

#[test]
fn erase_in_line_clears_to_end() {
    // Write six cells, move to column 3, erase to end of line.
    insta::assert_json_snapshot!(frame(6, 1, b"abcdef\x1b[3G\x1b[0K"));
}

#[test]
fn scroll_region_confines_scrolling() {
    // Restrict the scroll region to rows 2..3 (DECSTBM), then write enough
    // lines that scrolling happens only inside the margins; row 1 and row 4
    // stay put.
    insta::assert_json_snapshot!(frame(4, 4, b"top\x1b[2;3r\x1b[2H1\r\n2\r\n3\r\n4"));
}

#[test]
fn dec_line_drawing_charset_maps_to_box_glyphs() {
    // Shift into the DEC special graphics charset: l q k render as ┌ ─ ┐.
    insta::assert_json_snapshot!(frame(6, 1, b"\x1b(0lqk\x1b(B"));
}

// --- Grapheme-width characterization -------------------------------------
//
// These pin how the emulator encodes width-ambiguous and multi-codepoint
// graphemes. ptybridge relays the emulator's cell model verbatim — it applies
// no Unicode width logic of its own — so these snapshots document that contract
// and flag any change in the emulator's behavior. See the width contract in
// PROTOCOL.md.

#[test]
fn ambiguous_width_chars_are_narrow() {
    // East Asian Ambiguous characters render as a single column each.
    insta::assert_json_snapshot!(frame(8, 1, "❯±°→※".as_bytes()));
}

#[test]
fn variation_selector_emoji_stays_narrow() {
    // A base symbol plus VS16 keeps the emulator's narrow width (it is not
    // promoted to emoji presentation), with the selector carried as a
    // zero-width continuation of the cell.
    insta::assert_json_snapshot!(frame(8, 1, "\u{2713}\u{fe0f}\u{2764}\u{fe0f}".as_bytes()));
}

#[test]
fn zwj_sequence_is_not_recombined() {
    // A ZWJ emoji family is left as its separate wide components, each a cell
    // (plus spacer), rather than one combined glyph.
    insta::assert_json_snapshot!(frame(10, 1, "👨\u{200d}👩\u{200d}👧".as_bytes()));
}

#[test]
fn regional_indicator_flag_is_two_cells() {
    // A flag is two regional indicators; the emulator keeps them as two
    // narrow cells rather than one wide flag glyph.
    insta::assert_json_snapshot!(frame(8, 1, "🇯🇵".as_bytes()));
}

#[test]
fn skin_tone_modifier_is_a_separate_cell() {
    // The skin-tone modifier is its own wide cell, not folded into the
    // preceding emoji.
    insta::assert_json_snapshot!(frame(8, 1, "👍\u{1f3fd}".as_bytes()));
}

#[test]
fn combining_marks_join_their_base_cell() {
    // Combining marks attach to the preceding grapheme: が (か + dakuten) is one
    // wide cell, é (e + acute) one narrow cell.
    insta::assert_json_snapshot!(frame(8, 1, "か\u{3099}e\u{0301}".as_bytes()));
}
