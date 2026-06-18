//! ptybridge binary entry point.

use std::io::BufReader;

use clap::Parser;
use ptybridge::cli::Cli;
use ptybridge::engine::{self, SessionConfig};
use ptybridge::transport;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log.as_deref())?;
    tracing::debug!(?cli, "parsed arguments");

    let config = SessionConfig {
        command: &cli.command,
        cols: cli.cols,
        rows: cli.rows,
        max_fps: cli.max_fps,
    };

    engine::run_session(
        &config,
        Box::new(BufReader::new(std::io::stdin())),
        Box::new(transport::stdio::writer()),
        // The process exits when the session ends, so nothing to close.
        || {},
    )
}

/// Initialize tracing to stderr, or to a file when `--log` is given.
fn init_tracing(log_path: Option<&str>) -> anyhow::Result<()> {
    use tracing_subscriber::{EnvFilter, fmt};

    let filter =
        EnvFilter::try_from_env("PTYBRIDGE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = fmt().with_env_filter(filter);

    match log_path {
        Some(path) => {
            let file = std::fs::File::create(path)?;
            builder.with_writer(std::sync::Mutex::new(file)).init();
        }
        None => builder.with_writer(std::io::stderr).init(),
    }
    Ok(())
}
