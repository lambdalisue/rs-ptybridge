//! Embed a git-derived version so `ptybridge --version` reflects the release
//! tag. Falls back to the Cargo.toml version when git is unavailable (e.g. a
//! source tarball without a `.git` directory).

use std::process::Command;

fn main() {
    let version = git_describe().unwrap_or_else(|| {
        std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is always set by cargo")
    });
    println!("cargo:rustc-env=PTYBRIDGE_VERSION={version}");

    // Rebuild when the checked-out commit or tags change, so the embedded
    // version stays current.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");
}

/// `git describe --tags --always --dirty`, or `None` when git is unavailable or
/// reports nothing (no repository, git not installed).
fn git_describe() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty=-dev"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let described = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!described.is_empty()).then_some(described)
}
