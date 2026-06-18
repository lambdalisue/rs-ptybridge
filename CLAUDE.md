# ptybridge

`ptybridge` is an agent-agnostic, editor-agnostic headless terminal CLI. It
allocates a PTY, emulates the terminal, and streams the resulting **screen
state** (line-grid diffs) as a JSONL protocol over stdio (events on stdout,
control on stdin), bidirectionally. One session per process.

## Spec-driven

`PROTOCOL.md` is the **single source of truth** for the wire protocol.

- Any change that affects the wire format updates `PROTOCOL.md` **first**, in
  the same change, then the code follows.
- Keep the snapshot tests (`tests/snapshot.rs`) consistent with the spec.
- Forward compatibility is mandatory: consumers ignore unknown `t` values and
  unknown fields. Additive changes (new message kinds, new fields) do **not**
  bump the protocol version.

## TDD (layered)

Apply Kent Beck's red-green-refactor to the **deterministic core**; the parts
that need a real terminal or a real PTY are covered by other layers. State the
layer a module belongs to rather than pretending unit coverage everywhere.

| Layer | What | How |
| --- | --- | --- |
| **Unit TDD** (red→green→refactor) | `codec` JSONL roundtrip, `protocol` serde tagging, `render::encode_cells` (grapheme/wide/repeat invariant), diff logic against a fake grid | `cargo test` |
| **Snapshot** | known ANSI bytes → Event stream (needs vterm wired) | `insta`, fixtures under `tests/` |
| **Integration + visual** | real PTY spawn, `examples/reference_render` | manual run; not unit-covered |

A "screen reproduced by eye" acceptance criterion is a visual check, not a unit
test — treat it as such.

## Invariants (never break)

1. The screen grid state lives on the **CLI side**. Consumers hold no state
   machine.
2. JSONL carries the **emulation result** (screen state), never a transcript of
   ANSI operations.
3. `grid_line` is **one cell = one grapheme**; a double-width character is its
   cell followed by an empty cell `[""]`. Consumers place cells with **zero
   width computation** — column advance equals the cell element count. This is
   the core of CJK safety.

## Architecture boundary

`src/term.rs` is the **only** boundary that isolates the vterm crate
(`alacritty_terminal`). Keep crate-specific types from leaking past it so the
emulator can be swapped.

## Conventions

- Comments, log messages, and error messages in **English**.
- Dual licensed **MIT OR Apache-2.0**.
- **Conventional Commits**; commit `Cargo.lock`.
- Carry context in the code itself: do not reference out-of-tree planning
  documents (e.g. milestone labels) in source, comments, or commit messages.
