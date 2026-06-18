//! Command-line interface definition.

use clap::Parser;

use crate::transport::codec::Format;

/// Allocate a PTY, emulate the terminal, and stream screen state on stdout,
/// reading control messages on stdin.
#[derive(Debug, Parser)]
#[command(name = "ptybridge", version = env!("PTYBRIDGE_VERSION"), about, long_about = None)]
pub struct Cli {
    /// Wire encoding: `jsonl` (one JSON object per line) or `msgpack`.
    #[arg(long, value_enum, default_value_t = Format::Jsonl)]
    pub format: Format,

    /// Initial grid width in columns.
    #[arg(long, default_value_t = 80)]
    pub cols: u16,

    /// Initial grid height in rows.
    #[arg(long, default_value_t = 24)]
    pub rows: u16,

    /// Upper bound on coalesced frames per second.
    #[arg(long, default_value_t = 60)]
    pub max_fps: u16,

    /// Lines of scrollback captured as they leave the top of the primary screen
    /// (emitted as `scrollback_push`). `0` disables scrollback entirely.
    ///
    /// This bounds the lines captured per PTY read (8 KiB, so at most ~8192
    /// newline-only lines); the default comfortably exceeds one read, so a
    /// single read never overflows it. The Host is the durable store — this is
    /// only the transient capture window.
    #[arg(long, default_value_t = 10000)]
    pub scrollback: usize,

    /// Write debug logs to this path instead of stderr.
    #[arg(long)]
    pub log: Option<String>,

    /// Command (and arguments) to run on the PTY, after `--`.
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}
