# ptybridge

`ptybridge` is an agent-agnostic, editor-agnostic **headless terminal CLI**. It
allocates a PTY, runs a child process on it, emulates the terminal, and streams
the resulting **screen state** (line-grid diffs) as a JSONL protocol over
stdio — events on stdout, control on stdin — bidirectionally.

The consumer never re-implements a terminal emulator: the grid state lives
entirely inside `ptybridge`, and the protocol carries the emulation *result*
(cells, highlights, cursor, scroll) — not a transcript of ANSI escape
sequences. One cell is one grapheme, so consumers place cells with zero width
computation, which keeps CJK and emoji correct by construction.

See [`PROTOCOL.md`](./PROTOCOL.md) for the authoritative wire-format spec.

## Status

Early development. One session per process over stdio: it streams incremental
`grid_line` diffs (with scroll collapse, highlights, cursor, alt-screen, and
title) and accepts input, resize, signal, ping, and shutdown control. The wire
format is versioned at `hello.v = 1`.

## Usage

```console
$ ptybridge -- bash
```

`ptybridge` runs `bash` on a PTY and emits JSONL events on stdout, one object
per line:

```jsonl
{"t":"hello","proto":"ptybridge","v":1,"cols":80,"rows":24,"features":["scroll","alt_screen","title"]}
{"t":"grid_line","row":0,"col":0,"cells":[["$",0],[" ",0,79]]}
{"t":"flush"}
```

The host sends control messages (input, resize, signal, ping, shutdown) on
stdin.

Pass `--format msgpack` to carry the same messages as MessagePack instead — more
compact and cheaper to parse for the high-frequency `grid_line` traffic.

## Reference host

A consumer reconstructs the screen from the event stream. The examples show how:

- [`examples/passthrough.ts`](./examples/passthrough.ts) — a Deno host that
  reconstructs the screen as ANSI on the real terminal and forwards input, so
  `passthrough.ts -- claude` behaves as if `claude` ran directly.
- [`examples/reference_render.rs`](./examples/reference_render.rs) — a minimal
  Rust consumer that repaints the screen text (display only).
- [`examples/msgpack_render.ts`](./examples/msgpack_render.ts) — a minimal Deno
  consumer of the `--format msgpack` stream (display only).

## License

Dual licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
- MIT license ([LICENSE-MIT](./LICENSE-MIT))

at your option.
