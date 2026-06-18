//! ptybridge: a headless terminal that streams screen state as a JSONL protocol.
//!
//! See `PROTOCOL.md` for the authoritative wire-format specification.

pub mod cli;
pub mod control;
pub mod engine;
pub mod hlcache;
pub mod palette;
pub mod protocol;
pub mod pty;
pub mod render;
pub mod term;
pub mod transport;
