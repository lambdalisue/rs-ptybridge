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
title), pushes lines that scroll off the top as `scrollback_push`, and accepts
input, resize, signal, ping, and shutdown control. The wire format is versioned
at `hello.v = 1`.

## Usage

```console
$ ptybridge -- bash
```

`ptybridge` runs `bash` on a PTY and emits JSONL events on stdout, one object
per line:

```jsonl
{"t":"hello","proto":"ptybridge","v":1,"cols":80,"rows":24,"features":["scroll","scrollback","alt_screen","title"]}
{"t":"grid_line","row":0,"col":0,"cells":[["$",0],[" ",0,79]]}
{"t":"flush"}
```

The host sends control messages (input, resize, signal, ping, shutdown) on
stdin.

Pass `--format msgpack` to carry the same messages as MessagePack instead — more
compact and cheaper to parse for the high-frequency `grid_line` traffic.

### Scrollback

Lines that scroll off the top of the primary screen are emitted as
`scrollback_push` (oldest first, same cell shape as `grid_line`) so the host can
append them to its own buffer and scroll through history **locally** — ideal for
a consumer that renders into a normal editor buffer (e.g. Vim/Neovim), where
scrolling is native window movement with no round-trip to the bridge. The host
owns the durable scrollback; the bridge transmits each line once as it leaves
the grid, which also captures lines from bursts too fast to appear on screen.
`--scrollback N` sets the per-chunk capture window (default 10000; `0` disables
it and drops the `scrollback` feature). See `scrollback_push` in
[`PROTOCOL.md`](./PROTOCOL.md).

## Reference host

A consumer reconstructs the screen from the event stream. The examples show how:

All examples are Deno scripts (`deno run --allow-run --allow-env`):

- [`examples/jsonl_render.ts`](./examples/jsonl_render.ts) — a minimal consumer
  of the default JSONL stream that repaints the screen text (display only).
- [`examples/msgpack_render.ts`](./examples/msgpack_render.ts) — the same
  minimal consumer for the `--format msgpack` stream (display only).
- [`examples/scrollback_render.ts`](./examples/scrollback_render.ts) — appends
  `scrollback_push` lines above a live region and, on exit, dumps the full
  transcript, showing scrolled-off history preserved (the Vim/Neovim-buffer
  model).
- [`examples/passthrough.ts`](./examples/passthrough.ts) — a full interactive
  host that reconstructs the screen as ANSI on the real terminal and forwards
  input, so `passthrough.ts -- claude` behaves as if `claude` ran directly.

## License

Dual licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE))
- MIT license ([LICENSE-MIT](./LICENSE-MIT))

at your option.
