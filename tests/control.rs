//! Integration tests for the bidirectional control round-trip.
//!
//! These drive the real binary over stdio with a child PTY process. Every wait
//! is bounded by a deadline so a regression fails the test instead of hanging.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(5);

/// A running `ptybridge` with a line-buffered view of its event stream.
struct Daemon {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Daemon {
    /// Spawn `ptybridge --cols <cols> --rows <rows> -- <command...>`.
    fn start(cols: u16, rows: u16, command: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ptybridge"))
            .args(["--cols", &cols.to_string(), "--rows", &rows.to_string()])
            .arg("--")
            .args(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ptybridge");

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        Daemon {
            child,
            stdin,
            lines,
        }
    }

    /// Send one control message as a JSONL line.
    fn send(&mut self, json: &str) {
        writeln!(self.stdin, "{json}").expect("write control");
        self.stdin.flush().expect("flush control");
    }

    /// Read event lines until one satisfies `pred`, or fail at the deadline.
    fn wait_for(&self, pred: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let remaining = DEADLINE
                .checked_sub(start.elapsed())
                .expect("timed out waiting for an event");
            match self.lines.recv_timeout(remaining) {
                Ok(line) if pred(&line) => return line,
                Ok(_) => continue,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    panic!("timed out waiting for an event")
                }
            }
        }
    }

    /// Assert the process exits before the deadline; kill it otherwise.
    fn expect_exit(&mut self) {
        let start = Instant::now();
        loop {
            if let Some(_status) = self.child.try_wait().expect("try_wait") {
                return;
            }
            if start.elapsed() > DEADLINE {
                let _ = self.child.kill();
                panic!("process did not exit after shutdown");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn resize_is_acknowledged_with_grid_resize() {
    let mut daemon = Daemon::start(20, 5, &["cat"]);
    // The handshake reports the initial size.
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    daemon.send(r#"{"t":"resize","cols":30,"rows":10}"#);
    let line = daemon.wait_for(|line| line.contains(r#""t":"grid_resize""#));
    assert!(line.contains(r#""cols":30"#), "got: {line}");
    assert!(line.contains(r#""rows":10"#), "got: {line}");
    daemon.send(r#"{"t":"shutdown"}"#);
}

#[test]
fn input_reaches_the_child_and_is_echoed() {
    let mut daemon = Daemon::start(20, 3, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    // The PTY echoes input and `cat` repeats the line; either way "hi" appears.
    daemon.send(r#"{"t":"input","data":"hi\r"}"#);
    let line =
        daemon.wait_for(|line| line.contains(r#""t":"grid_line""#) && line.contains(r#""h""#));
    assert!(line.contains(r#""i""#), "got: {line}");
    daemon.send(r#"{"t":"shutdown"}"#);
}

#[test]
fn ping_is_answered_with_pong() {
    let mut daemon = Daemon::start(10, 2, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    daemon.send(r#"{"t":"ping","id":42}"#);
    let line = daemon.wait_for(|line| line.contains(r#""t":"pong""#));
    assert!(line.contains(r#""id":42"#), "got: {line}");
    daemon.send(r#"{"t":"shutdown"}"#);
}

#[test]
fn shutdown_exits_cleanly_and_reports_child_exit() {
    let mut daemon = Daemon::start(10, 2, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    daemon.send(r#"{"t":"shutdown"}"#);
    daemon.wait_for(|line| line.contains(r#""t":"child_exit""#));
    daemon.expect_exit();
}

#[test]
fn an_over_long_control_line_is_rejected_and_the_session_resynchronizes() {
    let mut daemon = Daemon::start(10, 2, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));

    // A line far past the reader's cap, never including a newline until the end.
    let mut huge = vec![b'x'; 2 * 1024 * 1024];
    huge.push(b'\n');
    daemon.stdin.write_all(&huge).expect("write huge line");
    daemon.stdin.flush().expect("flush huge line");

    // It must be answered with a parse error, not buffered without bound.
    let line = daemon.wait_for(|line| line.contains(r#""t":"error""#));
    assert!(line.contains(r#""code":"parse""#), "got: {line}");

    // The reader resynchronizes to the next newline, so a following ping works.
    daemon.send(r#"{"t":"ping","id":7}"#);
    let pong = daemon.wait_for(|line| line.contains(r#""t":"pong""#));
    assert!(pong.contains(r#""id":7"#), "got: {pong}");
    daemon.send(r#"{"t":"shutdown"}"#);
}

#[test]
fn signal_terminated_child_reports_a_signal_in_child_exit() {
    // `cat` blocks reading the PTY; a TERM signal terminates it, so child_exit
    // must carry the signal (with a null exit code) rather than a fabricated one.
    let mut daemon = Daemon::start(10, 2, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    daemon.send(r#"{"t":"signal","name":"TERM"}"#);
    let line = daemon.wait_for(|line| line.contains(r#""t":"child_exit""#));
    assert!(
        !line.contains(r#""signal":null"#),
        "expected a non-null signal, got: {line}"
    );
    assert!(
        line.contains(r#""code":null"#),
        "expected a null code, got: {line}"
    );
    daemon.expect_exit();
}

#[test]
fn spawn_failure_reports_a_spawn_error() {
    let daemon = Daemon::start(10, 2, &["/nonexistent/ptybridge-cmd-xyz"]);
    let line = daemon.wait_for(|line| line.contains(r#""t":"error""#));
    assert!(line.contains(r#""code":"spawn""#), "got: {line}");
}

#[test]
fn stdin_eof_ends_the_session() {
    let mut daemon = Daemon::start(10, 2, &["cat"]);
    daemon.wait_for(|line| line.contains(r#""t":"hello""#));
    // Closing stdin (dropping the writer) must end the session like shutdown.
    drop(std::mem::replace(&mut daemon.stdin, dummy_stdin()));
    daemon.expect_exit();
}

/// A throwaway stdin handle so `Daemon` keeps a valid field after we drop the
/// real one to signal EOF.
fn dummy_stdin() -> ChildStdin {
    Command::new("true")
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn true")
        .stdin
        .take()
        .expect("piped stdin")
}
