//! Property tests for the CJK cell invariant.
//!
//! The screen-state contract is that a host advances exactly one column per cell
//! width: for any emulated row, the sum of cell widths equals the grid's column
//! count. This must hold for any mix of ASCII, CJK, emoji, and combining marks —
//! and it is the only width guarantee ptybridge makes. Width and grapheme
//! grouping mirror the emulator exactly; ptybridge applies no Unicode width logic
//! of its own (see the width contract in `PROTOCOL.md`).

use proptest::prelude::*;

use ptybridge::protocol::{Cell, Event};
use ptybridge::render::Renderer;
use ptybridge::term::Emulator;

/// A character drawn from the classes that stress width handling.
fn glyph() -> impl Strategy<Value = char> {
    prop_oneof![
        // ASCII printables and spaces.
        (0x20u32..0x7f).prop_map(|c| char::from_u32(c).unwrap()),
        // Line control to exercise wrapping and new lines.
        Just('\n'),
        Just('\r'),
        Just('\t'),
        // Double-width CJK ideographs and Hangul.
        prop::sample::select(vec!['あ', '日', '本', '語', '中', '한', '글']),
        // Emoji, including base emoji and the building blocks of grapheme
        // sequences (ZWJ, variation selector, skin tone, regional indicator)
        // the emulator does not recombine into a single cell.
        prop::sample::select(vec![
            '🦀',
            '😀',
            '🎉',
            '✅',
            '👨',
            '👍',
            '\u{200d}',  // zero-width joiner
            '\u{fe0f}',  // variation selector-16
            '\u{1f3fd}', // medium skin tone
            '\u{1f1ef}', // regional indicator J
            '\u{1f1f5}', // regional indicator P
        ]),
        // East Asian Ambiguous-width characters: the emulator renders these
        // narrow (width 1), so the invariant must still balance.
        prop::sample::select(vec!['❯', '±', '°', '→', '※', 'α', 'я', '─', '│']),
        // Combining marks (zero width, attach to the previous grapheme).
        prop::sample::select(vec!['\u{0301}', '\u{3099}', '\u{0308}']),
    ]
}

proptest! {
    /// Every emulated row's cell widths sum to exactly the column count.
    #[test]
    fn every_row_advances_exactly_its_columns(
        glyphs in proptest::collection::vec(glyph(), 0..60),
        cols in 2u16..32,
        rows in 1u16..12,
    ) {
        let text: String = glyphs.into_iter().collect();
        let mut emu = Emulator::new(cols, rows);
        emu.feed(text.as_bytes());
        let screen = emu.snapshot().screen;

        for (row, line) in screen.lines.iter().enumerate() {
            // Each cell occupies one or two columns — never zero or more.
            for cell in line {
                prop_assert!(
                    cell.width == 1 || cell.width == 2,
                    "row {} has a cell of width {}",
                    row,
                    cell.width
                );
            }
            let span: u32 = line.iter().map(|c| c.width as u32).sum();
            prop_assert_eq!(
                span,
                screen.cols as u32,
                "row {} spans {} columns, expected {}",
                row,
                span,
                screen.cols
            );
        }
    }

    /// The wire encoding conserves columns too: a rendered `grid_line` advances a
    /// host by exactly the grid width (runs and wide-cell spacers included), so a
    /// consumer that places cells by element count stays aligned with no width
    /// computation of its own.
    #[test]
    fn rendered_grid_lines_conserve_columns(
        glyphs in proptest::collection::vec(glyph(), 0..60),
        cols in 2u16..32,
        rows in 1u16..12,
    ) {
        let text: String = glyphs.into_iter().collect();
        let mut emu = Emulator::new(cols, rows);
        emu.feed(text.as_bytes());

        let events = Renderer::default().frame(&emu.snapshot());
        for event in &events {
            if let Event::GridLine { row, cells, .. } = event {
                let span: u32 = cells.iter().map(Cell::span).sum();
                prop_assert_eq!(
                    span,
                    cols as u32,
                    "grid_line for row {} spans {} columns, expected {}",
                    row,
                    span,
                    cols
                );
            }
        }
    }
}
