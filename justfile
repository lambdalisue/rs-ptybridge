[private]
default:
  @just --list

# Format Rust and the Deno reference hosts.
fmt:
  cargo fmt --all
  deno fmt

# Lint, format-check, and type-check everything (mirrors CI).
check:
  cargo fmt --all --check
  cargo clippy --all-targets --all-features
  deno fmt --check
  deno lint
  deno check examples/passthrough.ts examples/msgpack_render.ts

# Run the Rust test suite.
test:
  cargo test --all-features

# Full local verification pass.
verify: check test

# Build the release binary.
build:
  cargo build --release --locked
