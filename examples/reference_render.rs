//! Minimal reference consumer: run a command under `ptybridge` over stdio and
//! repaint its screen to this terminal. For eyeballing that the emulation and
//! the JSONL stream reproduce a real TUI.
//!
//! Usage:
//!   cargo run --example reference_render -- top
//!   cargo run --example reference_render -- bash -c 'printf "hi\r\n"; sleep 1'

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use ptybridge::protocol::Event;

fn main() -> anyhow::Result<()> {
    let command: Vec<String> = env::args().skip(1).collect();
    if command.is_empty() {
        eprintln!("usage: reference_render -- <command> [args...]");
        std::process::exit(2);
    }

    let bin = ptybridge_binary();
    let mut child = Command::new(&bin)
        .arg("--")
        .args(&command)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");

    // Enter the alternate screen so the host terminal is restored on exit.
    print!("\x1b[?1049h\x1b[2J");
    std::io::stdout().flush()?;

    let mut grid: Vec<String> = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        let Ok(event) = serde_json::from_str::<Event>(&line) else {
            continue; // ignore unknown / non-event lines
        };
        match event {
            Event::Hello { rows, .. } => {
                grid = vec![String::new(); rows as usize];
            }
            Event::GridResize { rows, .. } => {
                grid.resize(rows as usize, String::new());
            }
            Event::GridLine { row, cells, .. } => {
                if let Some(slot) = grid.get_mut(row as usize) {
                    *slot = render_row(&cells);
                }
            }
            Event::Flush => repaint(&grid)?,
            Event::ChildExit { .. } => break,
            _ => {}
        }
    }

    // Leave the alternate screen.
    print!("\x1b[?1049l");
    std::io::stdout().flush()?;
    child.wait()?;
    Ok(())
}

/// Expand a row's cells back into a plain string (one grapheme per cell, runs
/// expanded). Empty spacer cells contribute nothing — the wide char already
/// occupies its two columns visually.
fn render_row(cells: &[ptybridge::protocol::Cell]) -> String {
    let mut text = String::new();
    for cell in cells {
        for _ in 0..cell.span() {
            text.push_str(&cell.text);
        }
    }
    text
}

/// Repaint the whole grid from the home position.
fn repaint(grid: &[String]) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(out, "\x1b[H")?;
    for row in grid {
        // Clear to end of line, then the row content.
        write!(out, "\x1b[K{row}\r\n")?;
    }
    out.flush()?;
    Ok(())
}

/// Locate the sibling `ptybridge` binary next to this example, falling back to
/// `ptybridge` on `PATH`.
fn ptybridge_binary() -> std::path::PathBuf {
    if let Ok(exe) = env::current_exe() {
        // target/<profile>/examples/reference_render -> target/<profile>/ptybridge
        if let Some(dir) = exe.parent().and_then(|p| p.parent()) {
            let candidate = dir.join(if cfg!(windows) {
                "ptybridge.exe"
            } else {
                "ptybridge"
            });
            if candidate.exists() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from("ptybridge")
}
