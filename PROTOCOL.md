# ptybridge protocol v1

This document is the authoritative specification of the ptybridge wire
protocol. It is self-contained; nothing else defines the format.

`ptybridge` allocates a PTY, runs a child process on it, emulates the terminal,
and streams the resulting **screen state** as JSONL. The two parties are:

- **Host** — the consumer (e.g. an editor plugin). Sends control messages
  (input, resize, signal) and renders the received screen state.
- **Bridge** — `ptybridge`. Owns the PTY and the terminal grid; emits screen
  diffs.

> The screen grid state lives entirely on the Bridge side. JSONL carries the
> **emulation result**, never a transcript of ANSI operations. The Host holds
> no terminal state machine.

The vocabulary mirrors Neovim's external `ext_linegrid` UI protocol.

## 1. Framing and encoding

The same messages are carried in one of two encodings, selected at startup
(`--format`, default `jsonl`):

- **JSONL** (default): one JSON object per line, separated by `\n`. JSON
  escapes any newline inside string values, so `\n` never appears mid-object.
- **MessagePack** (`--format msgpack`): each message is one self-delimiting
  MessagePack value, written back-to-back with no separator. More compact and
  cheaper to parse for the high-frequency `grid_line` traffic.

Both encodings carry the identical message shapes (§4, §5) — a map keyed by `t`,
with the same cell-array form. Events are written to **stdout** and controls
are read from **stdin**, an ordered, reliable stream in each direction.
**Direction is determined by message kind** (§3). Text is UTF-8; binary input
uses a base64 field where noted (`input`).

## 2. Envelope and compatibility

Every message is an object with a mandatory string discriminator `t`. Keys
are short because rendering messages are high-frequency.

```json
{"t": "grid_line", "row": 3, "col": 0, "cells": [["h", 7], [" ", 0, 5]]}
```

- **Unknown fields are ignored** (forward compatible).
- **Unknown `t` is logged and ignored** — never fatal, so the protocol tolerates
  future additions.

## 3. Message directions

| Direction | Kinds |
| --- | --- |
| **Bridge → Host** (Event) | `hello` `grid_resize` `hl_attr` `default_colors` `grid_line` `grid_scroll` `scrollback_push` `grid_clear` `cursor` `mode` `title` `bell` `flush` `child_exit` `error` `pong` |
| **Host → Bridge** (Control) | `input` `resize` `signal` `ping` `shutdown` |

A message that flows the wrong way is a protocol violation answered with
`error{code:"direction"}`.

## 4. Events (Bridge → Host)

### `hello` — handshake
```json
{"t":"hello","proto":"ptybridge","v":1,"cols":80,"rows":24,"features":["scroll","scrollback","alt_screen","title"]}
```
Sent right after startup. `v` is the protocol version. `features` advertises
the optional capabilities the Bridge emits.

### `grid_resize` — grid size settled
```json
{"t":"grid_resize","cols":120,"rows":40}
```
Sent after applying a Host `resize` (PTY + emulator resized) or at init. The
Host adjusts its buffer line count to match.

### `hl_attr` — highlight attribute definition
```json
{"t":"hl_attr","id":7,"fg":65280,"bg":null,"sp":null,"bold":true}
```
- `id`: integer referenced by later `grid_line` cells.
- `fg`/`bg`/`sp`: `0xRRGGBB` integers; `null` means the default color. Always
  present.
- Attributes: `bold` `italic` `underline` `undercurl` `reverse` `strikethrough`
  `blink` `dim` — boolean, **omitted when false** (absent = false).
- **A defined id is valid for the lifetime of the connection** — the same
  attribute set is never re-sent. This is the basis of diffing.

### `default_colors` — default palette
```json
{"t":"default_colors","fg":13684944,"bg":0,"sp":13684944}
```

### `grid_line` — cell update for one row (the workhorse)
```json
{"t":"grid_line","row":3,"col":0,"cells":[["h",7],["e"],["l",7,3],[" ",0,5]]}
```
- `row`/`col`: 0-based. Cells are laid left-to-right starting at `col`.
- `cells`: array of `[text, hl_id?, repeat?]`, identical semantics to Neovim
  `grid_line`:
  - `hl_id` omitted ⇒ inherit the previous cell's id. When `repeat` is present,
    `hl_id` is present too (the encoder always emits the resolved id alongside a
    run).
  - `repeat` compresses **a run of identical cells** (`["l",7,3]` = the same
    `"l"` with `hl_id` 7 three times; also used for blank fill).

**CJK-safety rule (mandatory):** one cell = **one grapheme**. Never pack a
multi-character run into `text`. This lets the Host place cells with **zero
width computation**.

- A **double-width** character is encoded as two cells: the character's cell
  followed by an empty cell `[""]`. The Host advances two columns without
  inspecting content.
- Therefore **Host column advance = total cell element count** (`repeat`
  included). This single rule places CJK and emoji correctly and keeps all ANSI
  interpretation and width calculation inside the Bridge.

**Width contract — the Bridge mirrors its emulator.** Cell width (1 or 2) and
grapheme grouping are whatever the underlying terminal emulator computed when it
laid the cell into the grid; the Bridge performs no Unicode width logic of its
own. The guarantee to the Host is *internal consistency* — the emitted cells
tile the row exactly — not agreement with any particular font's rendering.
Consequences a Host should expect:

- **East Asian Ambiguous** characters follow the emulator's policy. The current
  emulator treats them as **narrow** (1 column); a Host that renders them wide
  will diverge. (A wide/CJK ambiguous mode depends on emulator support.)
- **Variation selectors** (e.g. VS16 emoji presentation) do **not** by
  themselves widen a cell — the selector rides along as a zero-width
  continuation of its base cell.
- **ZWJ sequences, regional-indicator flags, and skin-tone modifiers** are
  **not** recombined into a single glyph: each component keeps its own
  cell(s) per the emulator's model.
- **Combining marks** attach to the preceding grapheme's cell (zero width).

### `grid_scroll` — scroll a region
```json
{"t":"grid_scroll","top":0,"bot":40,"left":0,"right":120,"rows":3,"cols":0}
```
Scrolls the `top..bot` / `left..right` rectangle by `rows` lines (positive =
up), identical to Neovim `grid_scroll`, so the Host can shift buffer lines
instead of repainting. A Bridge emits this only when it advertised `scroll` in
`hello`; a Bridge that does not advertise `scroll` re-sends the affected rows as
`grid_line` instead. A Host connected to a `scroll`-advertising Bridge **must**
apply `grid_scroll` (§8).

### `scrollback_push` — commit lines that scrolled off the top

```json
{"t":"scrollback_push","lines":[[["o",7],["l",7,2],["d"]],[["n","ext"]]]}
```

Carries the lines that have just scrolled off the **top of the primary
screen**, oldest first, so the Host can append them to its own scrollback. Each
element of `lines` is a `cells` array with the exact same shape and semantics as
`grid_line.cells` (§`grid_line`): one cell = one grapheme, wide characters
followed by an empty spacer, runs compressed with `repeat`, highlight ids
referencing prior `hl_attr` definitions.

A Bridge emits this only when it advertised `scrollback` in `hello`. It is the
**sole** source of committed scrollback content — a Host **must not** derive
scrollback from `grid_scroll` (which only shifts the live region). This matters
for bursts: a program that prints hundreds of lines within one frame scrolls
them past the visible grid, so they never appear as `grid_line`; `scrollback_push`
delivers their content regardless.

The Host's model is two disjoint regions:

```
buffer = [committed scrollback] ++ [live region: `rows` lines]
```

- `scrollback_push` inserts its `lines` immediately **above** the live region
  (committed scrollback grows; never rewritten).
- `grid_line` / `grid_scroll` / `grid_clear` address only the live region, which
  always holds the current visible grid. With this split the Host scrolls
  through history **locally** (e.g. a normal editor buffer) with no round-trip.

Only the **primary** screen has scrollback. While `alt_screen` is active the
Bridge emits no `scrollback_push`; the live region is updated in place and the
committed scrollback is left untouched.

The committed content is a frozen transcript: lines are **not** reflowed when
the grid is later resized (only the live region reflows). Capture is
best-effort and bounded by the Bridge's `--scrollback` capacity; a single
pathological burst larger than that capacity may drop its oldest lines.

### `grid_clear` — clear the whole grid
```json
{"t":"grid_clear"}
```

### `cursor` — cursor position / visibility
```json
{"t":"cursor","row":3,"col":5,"visible":true,"shape":"block"}
```
`shape`: `block` | `bar` | `underline`. A display-only Host may render a pseudo
cursor or ignore this.

### `mode` — alt-screen state
```json
{"t":"mode","alt_screen":true}
```
Signals entering/leaving the alternate screen. While `alt_screen` is active the
live region is a scratch surface that overwrites in place and produces no
`scrollback_push` — the Host keeps its committed scrollback (§`scrollback_push`)
untouched until the primary screen returns. The Host stores the committed
scrollback; the Bridge only transmits each line once, as it scrolls off.

### `title` / `bell`
```json
{"t":"title","text":"claude"}
{"t":"bell"}
```
OSC title and terminal bell.

### `flush` — frame boundary (required)
```json
{"t":"flush"}
```
Marks everything since the previous `flush` as one frame to apply atomically.
The Host batches buffer updates per frame.

### `child_exit` — child process ended
```json
{"t":"child_exit","code":0,"signal":null}
```
A child that exited normally reports its `code` with `signal` null; a child
terminated by a signal reports the signal name in `signal` with `code` null. If
the child failed to spawn, both are null and an `error{code:"spawn"}` precedes
it; `code: -1` is a sentinel for a failed `wait`.

### `error` / `pong`
```json
{"t":"error","code":"bad_message","message":"unknown control type: foo"}
{"t":"pong","id":42}
```

## 5. Controls (Host → Bridge)

### `input` — terminal input
```json
{"t":"input","data":"[A"}
{"t":"input","enc":"base64","data":"..."}
```
Bytes written to the PTY master. Default is a UTF-8 string (the Host has already
encoded keys into terminal escape sequences). Use `enc:"base64"` for binary.

### `resize` — size change
```json
{"t":"resize","cols":120,"rows":40}
```
Notifies the Bridge of a window size change. The Bridge resizes the PTY
(`TIOCSWINSZ`), resizes the emulator, and replies with `grid_resize`. The child
receives `SIGWINCH`. **The Host is the authority on size.**

### `signal`
```json
{"t":"signal","name":"INT"}
```
Sends a signal to the child. Accepted names: `HUP` `INT` `QUIT` `KILL` `TERM`
(any other name is answered with `error{code:"bad_message"}`).

### `ping` / `shutdown`
```json
{"t":"ping","id":42}
{"t":"shutdown"}
```
Keepalive and explicit disconnect.

## 6. Frame ordering

Within one frame (after the previous `flush`, up to the next `flush`), messages
are emitted in this order:

1. `grid_resize` — only on a size change
2. `default_colors` — once, on the first frame
3. `hl_attr*` — define any **new** ids referenced below (by both
   `scrollback_push` and the live-region body)
4. `scrollback_push` — lines committed since the previous frame, applied before
   the live region so its rows resolve against the advanced buffer
5. `grid_clear` — on a full repaint
6. `grid_scroll*` / `grid_line*` — the live-region body
7. `cursor`
8. `mode` / `title` / `bell` — whichever occurred
9. `flush`

`hl_attr` defines only ids not previously sent; established ids are never
re-sent.

## 7. Connection lifecycle

```
(start: stdin/stdout connected)
  Bridge → hello{cols,rows,features}
  Host   → resize{cols,rows}            (real window size)
  Bridge spawns child on the PTY (TIOCSWINSZ)
  loop:
    child → PTY output (ANSI bytes)
    Bridge → hl_attr* / grid_line* / cursor / flush
    Host   → input{data}                (user input)
    Host   → resize{cols,rows}          (on window change)
    Bridge → grid_resize + repaint frame
  child exits
  Bridge → child_exit{code}
  (process disconnects and exits)
```

### resize sequence

```
Host   → resize{cols:120, rows:40}
Bridge → PTY.resize(120,40)  → child SIGWINCH
Bridge → emulator.resize(120,40); mark full repaint
Bridge → grid_resize{120,40}
child  → repaint output → ANSI bytes
Bridge → hl_attr* / grid_line* / cursor / flush
```

## 8. Versioning and negotiation

- `hello.proto = "ptybridge"`, `hello.v = <int>` (independent of the crate
  version). A Host that cannot speak the version sends `shutdown`.
- `hello.features` lists the optional capabilities the Bridge **emits**
  (`scroll`, `scrollback`, `alt_screen`, `title`, future `sixel`, …). A Host
  **must** handle every advertised feature — e.g. apply `grid_scroll` when
  `scroll` is present, and append `scrollback_push` lines when `scrollback` is
  present. The `scroll` feature (emits `grid_scroll`) and the `scrollback`
  feature (emits `scrollback_push`) are independent: `scroll` optimizes
  live-region repaints, `scrollback` preserves lines that leave the grid. v1 has
  no Host→Bridge capability exchange, so a Host cannot opt out of a feature per
  connection; per-Host suppression is a future extension.
- Adding `scrollback_push` and the `scrollback` feature is **additive** (a new
  message kind, a new feature flag); it does not bump `hello.v`.

## 9. Error handling

| Event | Behavior |
| --- | --- |
| Malformed JSON line | reply `error{code:"parse"}`, drop the line |
| Control line exceeding the Bridge's length cap | reply `error{code:"parse"}`, drop the line, resynchronize at the next newline |
| Malformed MessagePack value, or one exceeding the Bridge's per-message byte cap | reply `error{code:"parse"}`, then end the stream — binary framing cannot resynchronize |
| Unknown `t` | log and ignore (forward compatible) |
| Wrong-direction message | `error{code:"direction"}` |
| Known control with bad fields, encoding, or signal name | `error{code:"bad_message"}` |
| PTY / spawn failure | `error{code:"spawn"}` then `child_exit` |
