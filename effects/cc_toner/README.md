# CC Toner

Multi-tone color mapping (clean-room reimplementation of After Effects' Cycore CC Toner). Computes per-pixel luminance, then maps that luminance through a 2–5 colour piecewise-linear gradient.

| Parameter | Type | Default | Notes |
|---|---|---|---|
| `tones` | enum (`solid` / `duotone` / `tritone` / `quadtone` / `pentone`) | `tritone` | Selects how many slots are active. |
| `highlights` | color | white | Used in Duo / Tri / Quad / Pentone. |
| `brights` | color | light grey | Used in Quad / Pentone. |
| `midtones` | color | mid grey | Used in Solid / Tri / Pentone. |
| `darktones` | color | dark grey | Used in Quad / Pentone. |
| `shadows` | color | black | Used in Duo / Tri / Quad / Pentone. |
| `blend` | float (0..1) | 0.0 | 0 = full effect, 1 = original through unchanged. |

**Pass type:** GPU.
**Working space:** sRGB-encoded — the compositor wraps the pass with the per-effect `srgb_wrap` transfers because CC Toner's character depends on the lerp happening on gamma-encoded values.

Slot activation per mode and the fixed luminance breakpoints are documented in `effects.md`.
