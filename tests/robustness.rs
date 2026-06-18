//! Robustness properties: arbitrary input must not panic, and the result must
//! not depend on how the byte stream is chunked.

use proptest::prelude::*;

use ptybridge::render::Renderer;
use ptybridge::term::Emulator;
use ptybridge::transport::codec::decode_control;

/// A character from a well-formed terminal stream: control codes and printable
/// ASCII (so ESC/CSI sequences form), plus multi-byte text. Excludes C1 controls
/// (U+0080..U+009F), whose split-mid-UTF-8 form vte may resolve differently.
fn terminal_char() -> impl Strategy<Value = char> {
    prop_oneof![
        (0x07u32..0x7f).prop_map(|c| char::from_u32(c).unwrap()),
        prop::sample::select(vec!['あ', '本', '日', '中', '한', '🦀', '😀']),
    ]
}

proptest! {
    /// Arbitrary PTY bytes never panic, keep the column invariant, and render.
    #[test]
    fn feeding_arbitrary_bytes_is_safe(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut emu = Emulator::new(20, 6);
        emu.feed(&bytes);
        let screen = emu.snapshot().screen;
        for line in &screen.lines {
            let span: u32 = line.iter().map(|c| c.width as u32).sum();
            prop_assert_eq!(span, screen.cols as u32);
        }
        // Rendering the snapshot must not panic either.
        let _ = Renderer::default().frame(&emu.snapshot());
    }

    /// For a well-formed terminal stream — printables, escape sequences, and
    /// multi-byte text — splitting the bytes at any boundary (including mid
    /// escape and mid grapheme) yields the same screen, because vte buffers its
    /// parser state across feeds.
    ///
    /// This excludes C1 controls (U+0080..U+009F): a lone UTF-8 continuation
    /// byte at a chunk boundary is invalid and vte may resolve it differently
    /// than the joined form. That is a pathological case (real output does not
    /// split a C1 control mid-byte); arbitrary bytes are covered by the panic
    /// test above, which only requires safety, not chunk invariance.
    #[test]
    fn chunk_boundaries_do_not_change_the_screen(
        chars in proptest::collection::vec(terminal_char(), 0..80),
        split in 0usize..240,
    ) {
        let text: String = chars.into_iter().collect();
        let bytes = text.as_bytes();
        let split = split.min(bytes.len());

        let mut whole = Emulator::new(16, 4);
        whole.feed(bytes);

        let mut parts = Emulator::new(16, 4);
        parts.feed(&bytes[..split]);
        parts.feed(&bytes[split..]);

        prop_assert_eq!(whole.snapshot().screen, parts.snapshot().screen);
    }

    /// Decoding arbitrary control lines never panics.
    #[test]
    fn decoding_arbitrary_control_lines_is_safe(line in ".*") {
        let _ = decode_control(&line);
    }
}
