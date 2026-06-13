# Invert

Per-pixel RGB inversion (`out.rgb = 1 - in.rgb`). Alpha is preserved unchanged.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| _none_ | | | | The effect has no tunable parameters. |

**Pass type:** CPU (Floyd–Steinberg-style sequential algorithms and reference CPU effects sit on this code path).
**Working space:** linear.

Used as the "minimal CPU effect" reference and the demo target for the adjustment-layer plumbing tests.
