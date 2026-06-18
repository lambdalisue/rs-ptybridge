//! Command-line interface definition.

use clap::Parser;

/// Allocate a PTY, emulate the terminal, and stream screen state as JSONL on
/// stdout, reading control messages as JSONL on stdin.
#[derive(Debug, Parser)]
#[command(name = "ptybridge", version = env!("PTYBRIDGE_VERSION"), about, long_about = None)]
pub struct Cli {
    /// Initial grid width in columns.
    #[arg(long, default_value_t = 80)]
    pub cols: u16,

    /// Initial grid height in rows.
    #[arg(long, default_value_t = 24)]
    pub rows: u16,

    /// Upper bound on coalesced frames per second.
    #[arg(long, default_value_t = 60)]
    pub max_fps: u16,

    /// Write debug logs to this path instead of stderr.
    #[arg(long)]
    pub log: Option<String>,

    /// Command (and arguments) to run on the PTY, after `--`.
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}
