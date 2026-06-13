# Signal (Signal-Lite)

WGSL approximation of analog video signal artifacts: chroma blur, ringing, snow, composite noise, and a periodic head-switching band. The full ntsc-rs adapter is the deferred F-071a follow-up.

| Parameter | Type | Default | Range | Notes |
|---|---|---|---|---|
| `chroma_blur` | float | 0.4 | 0…1 | Horizontal smear of color information. |
| `ringing_intensity` | float | 0.5 | 0…1 | Edge ringing characteristic of a low-bandwidth signal. |
| `snow_intensity` | float | 0.0 | 0…1 | Random per-pixel noise. |
| `composite_noise` | float | 0.1 | 0…1 | Lower-frequency colored noise. |
| `head_switch_height` | float | 8.0 | 0…64 | Pixels of vertical extent for the switching band at the bottom. |
| `head_switch_shift` | float | 4.0 | -32…32 | Horizontal pixel shift inside the band. |
| `seed` | int | 0 | 0…1000 | Determinism seed for the noise streams. |

**Pass type:** GPU.
**Working space:** linear.

Pair with `vhs` and `crt` for the canonical "VHS on a CRT" preset.
