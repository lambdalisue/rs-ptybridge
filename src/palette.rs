//! Resolve colors to `0xRRGGBB`, or `None` for the terminal default.
//!
//! This module is free of emulator-crate types: `term.rs` maps the emulator's
//! colors into the neutral [`Color`] here, keeping the vterm dependency behind
//! that single boundary.

/// A color to resolve, independent of any terminal-emulator crate.
pub enum Color {
    /// The themeable default foreground/background — the host supplies the
    /// concrete value via `default_colors`.
    Default,
    /// A 256-color palette index: 0..16 ANSI, 16..232 cube, 232..256 grayscale.
    Indexed(u8),
    /// A direct truecolor value.
    Rgb(u8, u8, u8),
}

/// The 16 ANSI colors as `0xRRGGBB` (xterm defaults).
const ANSI16: [u32; 16] = [
    0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0, 0x808080,
    0xff0000, 0x00ff00, 0xffff00, 0x0000ff, 0xff00ff, 0x00ffff, 0xffffff,
];

/// Resolve a color to packed RGB, or `None` for the terminal default.
pub fn resolve(color: Color) -> Option<u32> {
    match color {
        Color::Default => None,
        Color::Indexed(index) => Some(indexed(index)),
        Color::Rgb(r, g, b) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
    }
}

fn indexed(index: u8) -> u32 {
    match index {
        0..=15 => ANSI16[index as usize],
        16..=231 => {
            // 6×6×6 color cube; each channel steps 0, 95, 135, 175, 215, 255.
            let n = index - 16;
            let level = |v: u8| -> u32 { if v == 0 { 0 } else { 55 + 40 * v as u32 } };
            (level(n / 36) << 16) | (level((n / 6) % 6) << 8) | level(n % 6)
        }
        232..=255 => {
            // 24-step grayscale ramp from 8 to 238.
            let gray = 8 + 10 * (index as u32 - 232);
            (gray << 16) | (gray << 8) | gray
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_resolves_to_none() {
        assert_eq!(resolve(Color::Default), None);
    }

    #[test]
    fn truecolor_packs_to_rgb() {
        assert_eq!(resolve(Color::Rgb(0x12, 0x34, 0x56)), Some(0x123456));
    }

    #[test]
    fn indexed_low_matches_ansi() {
        assert_eq!(resolve(Color::Indexed(1)), Some(0x800000));
        assert_eq!(resolve(Color::Indexed(15)), Some(0xffffff));
    }

    #[test]
    fn indexed_cube_endpoints() {
        assert_eq!(resolve(Color::Indexed(16)), Some(0x000000));
        assert_eq!(resolve(Color::Indexed(231)), Some(0xffffff));
    }

    #[test]
    fn indexed_grayscale_ramp() {
        assert_eq!(resolve(Color::Indexed(232)), Some(0x080808));
        assert_eq!(resolve(Color::Indexed(255)), Some(0xeeeeee));
    }
}
