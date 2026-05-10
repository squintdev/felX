# analog-felx — User Guide

## Getting started

Run `cargo run -p felx-app` to launch the GUI, or `cargo run -p felx-cli -- help` for the headless render runner.

The default project is a 1280×720 / 30fps composition with a single slate-blue solid layer and a Gain effect. Use the **Layers** panel to add more, the **Effects** panel to dial parameters, and the bottom transport bar to play / scrub / change preview resolution.

## The viewer

The central area shows the current frame at the chosen preview scale.

- **Shift-drag** — set a region of interest. The viewer zooms to that crop. **Esc** clears.
- **Pen tool** (Masks panel) — once active, click on the viewer to drop corner anchors; click within ~12 px of the first anchor to close the path and commit a new mask on the selected layer; **Esc** cancels.

## Effects

The right-hand **Effects** panel auto-generates controls from each effect's manifest. Float parameters have a **stopwatch toggle** (`○` ↔ `⏱`) that converts between static and animated:

- Static: a plain slider sets the value.
- Animated: dragging the slider sets/updates a keyframe at the playhead. A small inline curve preview shows the keyframes; **drag** dots to retime, **right-click** for the interp picker / delete, **double-click** empty space to add at the click position.

The full **Graph editor** (toggle from the transport bar with `▲ Graph`) shows every animated Float parameter on the selected layer as a 2-D row across the comp duration. **Shift-click** for multi-select, **Delete** / **Backspace** removes the selection, right-click on a selection for an interp picker that applies to all selected. Hotkeys: `1`=Hold, `2`=Linear, `3`=EaseIn, `4`=EaseOut, `5`=EaseInOut.

## Layers

- `+ Solid` adds a colored solid; `+ Adj` adds an Adjustment layer.
- An **Adjustment** layer applies its effect stack to the flattened result of the layers beneath it (standard AE semantics).
- **Time remap** (collapsing section) gives any Composition or Video layer a constant `offset (frames)` and `scale` (0.5 = half-speed, 2.0 = double, –1.0 = reverse).

## Masks

The Masks panel under the layers panel provides:

- `+ Rect` / `+ Ellipse` — shape primitives at the comp center.
- `Pen` — minimal click-to-add-corner pen tool (see Viewer).
- Per-mask: mode (Add / Subtract / Intersect / Difference), opacity, feather, expansion.

## Effect catalog

Built-in effects (read each effect's `effects/<id>/README.md` for parameter detail, or open the **Help** menu in the top bar):

- `gain`, `invert`
- `cc_toner` — multi-tone color mapping
- `signal` — analog video artifacts
- `squint_diffusion` — directional error-diffusion halftoning
- `crt` — combined CRT display sim
- `vhs` — combined VHS tape defects
- `crt_persistence` — phosphor decay (use on an Adjustment layer for cross-source trails)
- `bloom` — generic threshold + Gaussian + composite

Built-in presets (top of the GUI):

- `CRT Consumer Trinitron`, `CRT Arcade Monitor`, `CRT PC Monitor`
- `VHS on CRT` — flagship; signal → vhs → crt

## Project files (`.felx`)

Text-based RON. Asset paths under the project file's parent directory are stored relative; assets outside stay absolute. See [`docs/decisions/0001-project-file-format.md`](decisions/0001-project-file-format.md) for the format rationale.

## Rendering

Two paths to a final file:

1. **CLI:** `cargo run -p felx-cli -- render <project.felx> --out <path> --format <fmt> [opts]`. `felx help` prints the full reference.
2. **GUI:** the render queue panel (data + UI) is in place; the front-end "Add to queue" hookup is the next polish step.

Formats: `h264` / `h265` / `prores422` / `prores4444` / `gif` / `png` (sequence) / `exr` (sequence) / `wav`.

Common encoder flags: `--crf` / `--bitrate` / `--max-bitrate` / `--preset` / `--profile` / `--gop` / `--hw <auto|nvenc|vaapi|videotoolbox>`. GIF: `--gif-palette 8..256`, `--gif-dither none|bayer|floyd|sierra`. WAV: `--wav-depth 16|24|f32`.

## Keyboard shortcuts

| Key | Action |
|---|---|
| Space | Play / pause |
| ←/→ | Step back / forward |
| Shift+drag (viewer) | Set region of interest |
| Esc | Clear ROI / cancel pen |
| Delete / Backspace (graph editor) | Delete selected keyframes |
| 1…5 (graph editor) | Set interp on selection (Hold / Linear / EaseIn / EaseOut / EaseInOut) |

## Troubleshooting

- **No preview?** Make sure your wgpu adapter is functional. `FELX_SOFTWARE_GPU=1 cargo run -p felx-app` forces the lavapipe / WARP software adapter as a fallback for environments without GPU.
- **Crash on launch?** Check `~/.felx/diagnostics/crash-*.txt` — F-112's panic hook captures the message and source location.
- **Hot reload not picking up shader changes?** `effects/<id>/effect.wgsl` is what's watched. The reload is currently active for `gain` only; other GPU effects opt in by adding their own `try_with_shader` path.

## See also

- [PRD](../PRD.md) — vision, scope, decisions.
- [`effects.md`](../effects.md) — per-effect concept notes and algorithm references.
- [`docs/decisions/`](decisions/) — ADRs (project file format, UI framework).
- [`NOTICES`](../NOTICES) — third-party attribution.
