# CRT Persistence

Stateful effect (F-079) that mixes the current frame with a decayed copy of the previous frame's output, simulating CRT phosphor falloff. Uses the F-070 ping-pong texture machinery.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `decay` | float | 0.85 | 0…0.99 | How much of the previous frame survives into this one. Higher = longer trails. |
| `tint_r` | float | 1.0 | 0…2 | Trail tint, red channel. |
| `tint_g` | float | 1.0 | 0…2 | Trail tint, green channel. |
| `tint_b` | float | 1.0 | 0…2 | Trail tint, blue channel. |

**Pass type:** GPU, stateful.
**Working space:** linear.

Most useful on an Adjustment layer so the trails accumulate across the flattened comp accumulator (multiple sources, post-effect). Per-layer placement only sees that layer's input.

State resets on any non-monotonic frame transition (seek, scrub, jump-to-end-and-loop) so scrubbing doesn't smear into mush.
