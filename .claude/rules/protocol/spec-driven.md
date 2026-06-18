---
paths: "{PROTOCOL.md,src/protocol*,src/transport/**,tests/snapshot.rs,tests/snapshots/**}"
---

# Spec-driven protocol changes

`PROTOCOL.md` is the single source of truth for the wire protocol.

- A change that affects the wire format updates `PROTOCOL.md` **first**, in the
  same change, then the code follows it.
- Keep the snapshot tests (`tests/snapshot.rs` with `tests/snapshots/`)
  consistent with the spec: ANSI fixtures must map to the Event stream the spec
  describes.

## Forward compatibility (mandatory)

- Consumers **ignore unknown `t` values** (log and drop) and **unknown fields**.
- Additive changes — new message kinds, new optional fields — do **not** bump
  `hello.v`. Only a breaking change bumps the version, negotiated via
  `hello.features`.
- Every message is a JSON object carrying a string `t` discriminator. Encode
  enums with `#[serde(tag = "t")]` and short field keys (this is a high-frequency
  rendering protocol).

Keep `PROTOCOL.md` self-contained: do not reference out-of-tree design or
planning documents from it.
