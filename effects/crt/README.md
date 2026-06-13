# CRT

Combined CRT display simulation: barrel curvature, scanlines, shadow mask, convergence offset, and vignette in one effect. The atomic split (one effect per sub-feature) is the deferred half of F-073…F-079.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `curvature_x` | float | 0.06 | 0…0.5 | Horizontal barrel curvature. |
| `curvature_y` | float | 0.08 | 0…0.5 | Vertical barrel curvature. |
| `scanline_intensity` | float | 0.4 | 0…1 | Strength of the scanline darkening. |
| `scanline_thickness` | float | 0.5 | 0.05…4 | Approximate pixel height of one scanline. |
| `mask_intensity` | float | 0.5 | 0…1 | Strength of the shadow-mask darkening. |
| `mask_size` | float | 3.0 | 1…12 | Pixel size of one mask cell. |
| `mask_type` | enum | `aperture_grille` | | `dot_trio` / `aperture_grille` / `slot_mask`. |
| `convergence_radial` | float | 1.5 | 0…16 | Per-channel radial offset (chromatic abberation). |
| `vignette_intensity` | float | 0.4 | 0…1 | Edge darkening. |
| `vignette_softness` | float | 0.4 | 0…1 | Falloff width. |

**Pass type:** GPU.
**Working space:** linear.

Phosphor persistence is the separate `crt_persistence` effect (F-079); use it on an Adjustment layer above the CRT for cross-source trails.
