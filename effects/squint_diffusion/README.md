# SquintDiffusion

Directional error-diffusion halftoning. Floyd–Steinberg distributes quantization error to neighboring pixels; a directional-scan twist (see NOTICES for prior art) gives the effect its recognizable look.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `num_colors` | int | 4 | 2…6 | Active palette size; only `color_1`…`color_<num_colors>` are used. |
| `color_1`…`color_6` | color | black, white, … | | Palette entries. |
| `error_weight` | float | 0.75 | 0…1 | How much error propagates forward. |
| `alpha` | float | 1.0 | 0…1 | Mix between original and diffused output. |

**Pass type:** CPU. Sequential dependency between pixels — not amenable to fragment-shader fanout.
**Working space:** linear.

Arbitrary scan angle (rotate-in / rotate-out), Steps quantization, Weight Falloff, and rayon per-row parallelism are deferred to F-072a.
