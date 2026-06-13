# Bloom

Generic three-pass bloom (F-076): threshold the bright pixels, separable Gaussian blur, additively composite back over the original. Reusable inside the CRT chain or as a standalone glow effect.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `threshold` | float | 0.7 | 0…2 | Luminance cutoff (Rec.709) above which pixels contribute to the bloom. |
| `intensity` | float | 0.6 | 0…4 | Multiplier on the blurred contribution. |
| `radius` | float | 4.0 | 1…32 | Gaussian blur radius in pixels. |
| `soft_knee` | float | 0.3 | 0…1 | Falloff width around the threshold — softer = smoother glow. |

**Pass type:** GPU, three pipelines orchestrated per frame.
**Working space:** linear.

Single-level Gaussian for v1 — the proper multi-level downsample chain that buys very wide blooms cheaply is a perf follow-up.
