//! Session engine: drive a child on a PTY and exchange JSONL with the host.
//!
//! PTY output and host control messages are merged onto one channel so a single
//! loop can react to both while coalescing output bursts into one frame per
//! `max_fps` window. The engine reads control lines from a [`BufRead`] (stdin)
//! and writes events to a [`Write`] (stdout).

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use crate::control::{decode_input, signal_number};
use crate::protocol::{Control, Event};
use crate::pty::{Exit, Pty};
use crate::render::Renderer;
use crate::term::Emulator;
use crate::transport::codec::{Decoded, decode_control, write_event};

/// Bound on in-flight messages, applying backpressure to the PTY (and thus the
/// child) when the host cannot keep up, instead of buffering without limit.
const CHANNEL_CAPACITY: usize = 64;

/// Upper bounds on a requested grid size, so a host cannot request a multi-gigacell
/// grid that exhausts memory.
const MAX_COLS: u16 = 4096;
const MAX_ROWS: u16 = 1024;

/// A message arriving at the engine from either input source.
enum Incoming {
    Output(Vec<u8>),
    OutputEnd,
    Control(Control),
    ControlError(Event),
    ControlEnd,
}

/// Whether the session should keep running after handling a message.
enum Flow {
    Continue,
    Stop,
}

/// Per-session parameters.
pub struct SessionConfig<'a> {
    pub command: &'a [String],
    pub cols: u16,
    pub rows: u16,
    pub max_fps: u16,
}

/// Run one session: read control lines from `reader`, write events to `writer`.
///
/// `close` runs when the session ends, a hook to release the underlying
/// connection. For stdio it is a no-op — process exit closes the streams.
pub fn run_session(
    config: &SessionConfig,
    reader: Box<dyn BufRead + Send>,
    writer: Box<dyn Write + Send>,
    close: impl FnOnce(),
) -> anyhow::Result<()> {
    let outcome = session_loop(config, reader, writer);
    close();
    outcome
}

fn session_loop(
    config: &SessionConfig,
    reader: Box<dyn BufRead + Send>,
    mut writer: Box<dyn Write + Send>,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::sync_channel::<Incoming>(CHANNEL_CAPACITY);
    spawn_control_reader(reader, tx.clone());

    // Clamp the initial grid to the same bounds the resize path enforces, so an
    // oversized --cols/--rows cannot request a multi-gigacell grid.
    let cols = config.cols.clamp(1, MAX_COLS);
    let rows = config.rows.clamp(1, MAX_ROWS);
    let max_fps = config.max_fps;
    let (mut pty, pty_rx) = match Pty::spawn(config.command, cols, rows) {
        Ok(pty) => pty,
        Err(err) => {
            // Report the spawn failure as the protocol prescribes, then end.
            write_event(
                &mut writer,
                &Event::Error {
                    code: "spawn".to_string(),
                    message: err.to_string(),
                },
            )?;
            write_event(
                &mut writer,
                &Event::ChildExit {
                    code: None,
                    signal: None,
                },
            )?;
            writer.flush()?;
            return Ok(());
        }
    };

    spawn_output_forwarder(pty_rx, tx.clone());
    drop(tx); // the loop exits once both source threads drop their senders

    let outcome = drive(&rx, &mut pty, &mut writer, cols, rows, max_fps);

    // Reap the child unconditionally — even if the loop bailed on an I/O error
    // (e.g. the host disconnected) — so it cannot linger as a zombie. The
    // explicit kill also unblocks a wait on a still-interactive child.
    let _ = pty.kill();
    let exit = pty.wait().unwrap_or(Exit {
        code: Some(-1),
        signal: None,
    });
    let _ = write_event(
        &mut writer,
        &Event::ChildExit {
            code: exit.code,
            signal: exit.signal,
        },
    );
    let _ = writer.flush();
    outcome
}

/// Drive the session: send the handshake, then react to PTY output and host
/// control until the session ends. Returns `Err` on an I/O failure (the caller
/// still reaps the child).
fn drive<W: Write>(
    rx: &Receiver<Incoming>,
    pty: &mut Pty,
    writer: &mut W,
    cols: u16,
    rows: u16,
    max_fps: u16,
) -> anyhow::Result<()> {
    let mut emulator = Emulator::new(cols, rows);
    let mut renderer = Renderer::default();
    let frame_interval = Duration::from_secs_f64(1.0 / max_fps.max(1) as f64);

    let features = ["scroll", "alt_screen", "title"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    write_event(writer, &Event::hello(cols, rows, features))?;
    // Paint an initial baseline so the host has a synchronized starting grid and
    // a later resize diffs against it (emitting grid_resize rather than a silent
    // first redraw).
    for event in renderer.frame(&emulator.snapshot()) {
        write_event(writer, &event)?;
    }
    writer.flush()?;

    while let Ok(msg) = rx.recv() {
        let mut dirty = false;
        let mut stop = matches!(
            handle(msg, &mut emulator, pty, writer, &mut dirty)?,
            Flow::Stop
        );

        // Coalesce: keep draining (output and control alike) until the window
        // closes, so a burst collapses into one frame.
        if !stop {
            let deadline = Instant::now() + frame_interval;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match rx.recv_timeout(remaining) {
                    Ok(more) => {
                        if matches!(
                            handle(more, &mut emulator, pty, writer, &mut dirty)?,
                            Flow::Stop
                        ) {
                            stop = true;
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                }
            }
        }

        let responses = emulator.take_pty_writes();
        if !responses.is_empty() {
            pty.write(&responses)?;
        }
        if dirty {
            for event in renderer.frame(&emulator.snapshot()) {
                write_event(writer, &event)?;
            }
            writer.flush()?;
        }
        if stop {
            break;
        }
    }
    Ok(())
}

/// Apply one incoming message, returning whether the session should stop.
fn handle<W: Write>(
    msg: Incoming,
    emulator: &mut Emulator,
    pty: &mut Pty,
    out: &mut W,
    dirty: &mut bool,
) -> anyhow::Result<Flow> {
    match msg {
        Incoming::Output(chunk) => {
            emulator.feed(&chunk);
            *dirty = true;
        }
        // Child output ended or the host hung up: end the session.
        Incoming::OutputEnd | Incoming::ControlEnd => return Ok(Flow::Stop),
        Incoming::ControlError(event) => {
            write_event(out, &event)?;
            out.flush()?;
        }
        Incoming::Control(Control::Input { enc, data }) => {
            match decode_input(enc.as_deref(), &data) {
                Ok(bytes) => pty.write(&bytes)?,
                Err(message) => {
                    write_event(
                        out,
                        &Event::Error {
                            code: "bad_message".to_string(),
                            message,
                        },
                    )?;
                    out.flush()?;
                }
            }
        }
        Incoming::Control(Control::Resize { cols, rows }) => {
            let cols = cols.clamp(1, MAX_COLS);
            let rows = rows.clamp(1, MAX_ROWS);
            pty.resize(cols, rows)?;
            emulator.resize(cols, rows);
            *dirty = true;
        }
        Incoming::Control(Control::Signal { name }) => match signal_number(&name) {
            Some(signal) => pty.signal(signal),
            None => {
                write_event(
                    out,
                    &Event::Error {
                        code: "bad_message".to_string(),
                        message: format!("unknown signal: {name}"),
                    },
                )?;
                out.flush()?;
            }
        },
        Incoming::Control(Control::Ping { id }) => {
            // Answer keepalive immediately; pong is not part of a frame.
            write_event(out, &Event::Pong { id })?;
            out.flush()?;
        }
        Incoming::Control(Control::Shutdown) => return Ok(Flow::Stop),
    }
    Ok(Flow::Continue)
}

/// Forward PTY output chunks onto the engine channel, signaling end on EOF.
fn spawn_output_forwarder(pty_rx: Receiver<Vec<u8>>, tx: SyncSender<Incoming>) {
    thread::spawn(move || {
        while let Ok(chunk) = pty_rx.recv() {
            if tx.send(Incoming::Output(chunk)).is_err() {
                return;
            }
        }
        let _ = tx.send(Incoming::OutputEnd);
    });
}

/// Cap on a single control line. A hostile or broken peer could otherwise send
/// bytes that never include a newline and grow the read buffer without bound;
/// channel capacity bounds the message *count*, not the size of one line. An
/// over-long line is rejected with a `parse` error and the reader resynchronizes
/// to the next newline rather than ending the session.
const MAX_CONTROL_LINE: usize = 1 << 20; // 1 MiB

/// Read control JSONL lines from `reader` onto the engine channel. Detached and
/// never joined: it blocks on the reader and exits when the engine drops the
/// channel.
fn spawn_control_reader(mut reader: Box<dyn BufRead + Send>, tx: SyncSender<Incoming>) {
    thread::spawn(move || {
        loop {
            let mut buf = Vec::new();
            match read_capped_line(&mut reader, &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }

            // The cap was hit without reaching a newline: the line is over-long.
            // Report it and discard the remainder up to the next newline so the
            // following lines still parse.
            if buf.len() > MAX_CONTROL_LINE && buf.last() != Some(&b'\n') {
                let event = Event::Error {
                    code: "parse".to_string(),
                    message: "control line exceeds maximum length".to_string(),
                };
                if tx.send(Incoming::ControlError(event)).is_err() {
                    return;
                }
                if skip_to_newline(&mut reader).unwrap_or(false) {
                    continue;
                }
                break; // EOF or error while resynchronizing
            }

            let line = String::from_utf8_lossy(&buf);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let msg = match decode_control(line) {
                Decoded::Control(control) => Incoming::Control(control),
                Decoded::Error(error) => Incoming::ControlError(error),
                // Unknown type: log and drop (forward compatibility).
                Decoded::Ignore => {
                    tracing::debug!(line = %line, "ignoring unknown control message");
                    continue;
                }
            };
            if tx.send(msg).is_err() {
                return;
            }
        }
        let _ = tx.send(Incoming::ControlEnd);
    });
}

/// Read one `\n`-terminated line (newline included) into `buf`, buffering at
/// most one byte past [`MAX_CONTROL_LINE`] so an unterminated flood cannot grow
/// memory without bound. Returns the bytes read (0 at EOF). A buffer that ends
/// without a newline and exceeds the cap marks an over-long line.
fn read_capped_line(reader: &mut dyn BufRead, buf: &mut Vec<u8>) -> io::Result<usize> {
    while buf.len() <= MAX_CONTROL_LINE {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break; // EOF
        }
        if let Some(i) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..=i]);
            reader.consume(i + 1);
            break;
        }
        // No newline yet: take only up to the cap so the remainder of an
        // over-long line stays in the reader for resynchronization.
        let take = available.len().min(MAX_CONTROL_LINE + 1 - buf.len());
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
    }
    Ok(buf.len())
}

/// Discard bytes up to and including the next newline without buffering them,
/// so resynchronizing after an over-long line stays within bounded memory.
/// Returns `Ok(true)` once a newline is consumed, `Ok(false)` at EOF.
fn skip_to_newline(reader: &mut dyn BufRead) -> io::Result<bool> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(false);
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(i) => {
                reader.consume(i + 1);
                return Ok(true);
            }
            None => {
                let len = available.len();
                reader.consume(len);
            }
        }
    }
}
