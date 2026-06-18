//! PTY allocation and child-process I/O via `portable-pty`.

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use anyhow::Context;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Bound on buffered PTY output chunks, so a stalled host applies backpressure
/// to the child rather than letting output accumulate without limit.
const OUTPUT_CAPACITY: usize = 64;

/// How a child process ended: a normal exit `code`, or the `signal` that
/// terminated it. Exactly one is `Some` (a signal death carries no exit code).
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

/// A child process running on a PTY. Output is delivered over the channel
/// returned by [`Pty::spawn`]; this handle owns the write side and controls.
pub struct Pty {
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
}

impl Pty {
    /// Spawn `command` (program followed by arguments) on a fresh PTY of the
    /// given size. Returns the handle and a receiver of PTY output chunks; the
    /// receiver closes when the child's output ends.
    pub fn spawn(
        command: &[String],
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<(Self, Receiver<Vec<u8>>)> {
        let (program, args) = command.split_first().context("command must not be empty")?;

        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut builder = CommandBuilder::new(program);
        builder.args(args);
        let child = pair.slave.spawn_command(builder)?;
        // Drop the slave so the master sees EOF once the child closes its end.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let (tx, rx) = mpsc::sync_channel(OUTPUT_CAPACITY);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let pty = Self {
            writer,
            child,
            master: pair.master,
        };
        Ok((pty, rx))
    }

    /// Write bytes to the PTY master (host input, or emulator query responses).
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resize the PTY; the child receives `SIGWINCH`.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Send a signal to the child process.
    #[cfg(unix)]
    pub fn signal(&mut self, signal: i32) {
        if let Some(pid) = self.child.process_id() {
            // Safety: kill is always safe to call; an invalid pid simply errors.
            unsafe {
                libc::kill(pid as libc::pid_t, signal);
            }
        }
    }

    /// Send a signal to the child process. Non-Unix platforms can only
    /// terminate it, so any signal maps to a kill.
    #[cfg(not(unix))]
    pub fn signal(&mut self, _signal: i32) {
        let _ = self.child.kill();
    }

    /// Forcibly terminate the child (used on shutdown so a blocked `wait`
    /// returns).
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    /// Wait for the child to exit and report how it ended. A signal-terminated
    /// child reports the signal name; otherwise it reports the exit code.
    pub fn wait(&mut self) -> anyhow::Result<Exit> {
        let status = self.child.wait()?;
        Ok(match status.signal() {
            Some(signal) => Exit {
                code: None,
                signal: Some(signal.to_string()),
            },
            None => Exit {
                code: Some(status.exit_code() as i32),
                signal: None,
            },
        })
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Backstop in case a caller drops the handle without an explicit
        // kill/wait: reap the child so it cannot linger as a zombie. Both calls
        // are no-ops if the child was already reaped.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
