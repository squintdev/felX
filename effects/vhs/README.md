# VHS

Combined VHS physical-tape defects: sync wobble, transport wobble, tape damage, dropouts, and a user-driven vertical roll.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `sync_wobble` | float | 1.0 | 0…8 | Horizontal sync-line jitter intensity. |
| `dropouts_density` | float | 0.2 | 0…1 | Per-row hash-driven streak frequency. |
| `dropouts_polarity` | float | 1.0 | -1…1 | Bright (+1) vs dark (-1) dropouts. |
| `tape_damage` | float | 0.2 | 0…1 | Drifting masked degradation. |
| `transport_brightness` | float | 0.2 | 0…1 | Slow brightness wobble. |
| `transport_chroma_phase` | float | 0.1 | 0…1 | Slow chroma phase wobble. |
| `transport_freq` | float | 0.5 | 0…4 | Wobble rate. |
| `vertical_scroll` | float | 0.0 | -1…1 | Manual vertical roll position; keyframe to animate. |
| `seed` | int | 0 | 0…1000 | Determinism seed. |
| `time_seconds` | float | 0.0 | | Should be wired to the comp playhead for stable wobble timing. |

**Pass type:** GPU.
**Working space:** linear.

Stack downstream of `signal` for the canonical "VHS on a CRT" preset.
