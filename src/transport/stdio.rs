//! stdio transport: emit JSONL events on a buffered stdout.
//!
//! One session per process over stdin/stdout, with the child's command given on
//! argv.

use std::io::{self, BufWriter, Stdout};

/// A buffered writer over stdout. Callers flush at each frame boundary.
pub fn writer() -> BufWriter<Stdout> {
    BufWriter::new(io::stdout())
}
