# ADR 0001 — Project file format

**Status:** Accepted
**Date:** 2026-05-09

## Context

analog-felx project files describe a deeply nested, heterogeneous tree:
compositions → layers (several variants) → transforms with keyframed curves →
effects with their own parameter trees (some of which carry optional
sub-groups like the Signal effect's `head_switching` and `vhs.edge_wave`). The
format must satisfy three hard requirements:

1. **Diff-readable.** Project files live in git; commits should produce
   reviewable diffs.
2. **Hand-editable.** When something goes wrong, the author should be able to
   open the file in a text editor and fix it.
3. **Round-trips through serde** without bespoke (de)serializer code, so
   adding a new layer type or effect parameter is a one-liner.

A representative sample project (3 layers — Video, Adjustment, Solid; one
effect with two optional nested sub-groups; a 3-keyframe bezier curve) was
serialized with each candidate and the outputs were compared.

## Considered options

### RON (Rusty Object Notation)

Native Rust enum syntax (`Video(...)`, `Linear`, `Bezier(in_tan: (..), out_tan: (..))`),
parentheses-and-commas data form, hierarchical without section headers.

```ron
position: Animated([
    (
        t: (num: 0, den: 30),
        v: (0.0, 0.0),
        interp: Bezier(in_tan: (0.0, 0.0), out_tan: (0.5, 0.0)),
    ),
    ...
])
```

### TOML

Section-headered, ubiquitous, smallest on disk. Heterogeneous nested data is
clumsy — animated keyframe arrays become arrays-of-tables that either explode
into many `[[...]]` headers or collapse into hard-to-read multiline inline
tables.

```toml
[[compositions.layers.Video.transform.position]]
Animated = [
    { t = { num = 0, den = 30 }, v = [0.0, 0.0], interp = { Bezier = { in_tan = [0.0, 0.0], out_tan = [0.5, 0.0] } } },
    ...
]
```

### JSON

Verbose, no comments, ubiquitous. Tagged-enum representations require explicit
`{"Video": {...}}` wrappers that read worse than RON's `Video(...)`.

### Binary (bincode / postcard)

Fails the diff-readable and hand-editable requirements. Not considered
seriously.

## Numbers

Sizes for the representative sample project (smaller = denser, not better):

| Format | Bytes |
|---|---|
| TOML | 2,439 |
| RON  | 4,756 |
| JSON | 4,956 |

Round-trip parse time, 1,000 iterations of the same project tree:

| Format | Time | Per parse |
|---|---|---|
| JSON | 6.7 ms  | 6.7 µs |
| RON  | 36.0 ms | 36 µs |
| TOML | 48.6 ms | 49 µs |

Project-file parsing happens once at load. Even the slowest result is
sub-millisecond on a single-comp project. Parse speed is not a meaningful
discriminator at this scale.

## Decision

**Use RON.** Filename extension: `.felx` (RON content).

## Consequences

- The `ron` crate (currently `0.10`) becomes a load-bearing workspace
  dependency, locked in by `Cargo.lock`. License is Apache-2.0/MIT — fine.
- **Tag enums externally** (the serde default — `Variant(...)` form). Avoid
  `#[serde(tag = "kind")]` (internally-tagged) and `#[serde(untagged)]`:
  RON 0.10 doesn't round-trip those cleanly. This was reproduced in the
  spike: both forms serialized fine but failed to deserialize.
- **Use `#[serde(default)]` and `#[serde(skip_serializing_if = "...")]`**
  liberally so older project files load against newer schemas (added fields
  get defaults; absent optional sub-groups don't bloat the file).
- Editor support is weaker than TOML/JSON. Mitigated by RON being
  syntactically simple — most editors highlight it tolerably as a Rust-like
  language, and a `.felx` mapping for VS Code / Helix can ship later.
- Pretty-printed RON is the on-disk form. Compact RON exists but offers no
  meaningful advantage given the file sizes involved.

## Out of scope

Streaming reads, partial loads, or schema migration tooling. These are
deferred until they become real problems. A future ADR (`0001a-...` or a
successor decision) should revisit if/when those constraints surface.

## References

- [RON on GitHub](https://github.com/ron-rs/ron)
- Spike code lived in `/tmp/felx-format-spike/` during the decision; deleted.
