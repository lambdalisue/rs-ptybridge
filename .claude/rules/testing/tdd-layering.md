---
paths: "{src/**/*.rs,tests/**/*.rs}"
---

# TDD layering

Apply Kent Beck's red-green-refactor to the **deterministic core**. Parts that
need a real terminal or a real PTY are covered by other layers — do not pretend
unit coverage where it does not fit.

| Module | Layer | Notes |
| --- | --- | --- |
| `protocol` | Unit TDD | serde tag roundtrip; cell encodes to `[text, hl?, repeat?]` |
| `transport::codec` | Unit TDD | JSONL line framing roundtrip; bad line → `error{code:"parse"}` |
| `render` cell encoding (`expand`/`compress`) | Unit TDD | grapheme/wide/repeat invariant against a fake grid line |
| `render` (diff / scroll) | Unit TDD | fake grid in, Event stream out |
| `hlcache` | Unit TDD | attr-set → id interning |
| `palette` | Unit TDD | neutral color → RGB; default → none |
| `term` | Snapshot (`insta`) | ANSI fixtures → Event stream; vterm wired |
| `pty`, `engine`, `session` | Integration | real PTY spawn |
| `examples/jsonl_render` | Visual | screen reproduced by eye; not a unit test |

## The cell invariant (test it hardest)

`render`'s cell encoding (`expand` + `compress`) must hold: **one cell = one
grapheme**, a double-width character is its cell followed by an empty cell
`[""]`, runs of identical cells are compressed via `repeat`. The property to
assert: **host column advance = total cell element count** for any mix of ASCII
/ CJK / emoji. Width comes from the vterm cell's wide flag; a grapheme is the
cell's base char plus its zero-width combining chars.
