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

- `Image` / `Video` / `Audio` import a media file as a new layer. Importing a **video** sizes the comp duration to the clip's length, and a clip with sound automatically gets a parallel Audio layer — its sound plays during preview and is muxed into video exports.
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

## Rendering from the GUI

**File → Export…** opens the export dialog. Pick a format (H.264 / H.265 / ProRes 422 / ProRes 4444 / GIF / PNG sequence / EXR sequence / WAV), an output path, and quality knobs (CRF + preset for the lossy codecs, palette/dither for GIF). The export runs on a background thread with its own compositor, so the GUI stays responsive; a progress window tracks it.

Video formats mux the comp's mixed audio bus automatically when the project has Audio layers (AAC inside MP4, PCM inside MOV). Comps without audio layers export video-only.

**File → Settings…** lets you pin which GPU the viewer and exports each use (handy on dual-GPU machines); the `FELX_GPU` environment variable overrides both.

(The render queue panel for batching multiple comps exists as a module but isn't wired into the UI yet — single exports go through File → Export.)

## Rendering from the CLI

The `felx` binary renders a `.felx` project headlessly — no window, no GPU contention with the GUI. To get a `.felx`, set up your comp in the GUI and **File → Save project**.

```bash
cargo run -p felx-cli -- render <project.felx> --out <path> --format <fmt> [opts]
```

`--comp <name>` picks a composition by name (default: the project's first).

Examples for each format:

```bash
# H.264 MP4 (CRF quality mode), audio muxed if the comp has Audio layers
felx render proj.felx --out out.mp4 --format h264 --crf 18 --preset slow

# H.265 MP4 (note: x265 profiles are main/main10, not high)
felx render proj.felx --out out.mp4 --format h265 --crf 22

# Bitrate-targeted H.264 (CBR; add --max-bitrate for VBR)
felx render proj.felx --out out.mp4 --format h264 --bitrate 8000000

# ProRes 422 / 4444 in MOV (profile: proxy / lt / standard / hq / 4444)
felx render proj.felx --out out.mov --format prores422 --profile hq
felx render proj.felx --out out.mov --format prores4444

# Animated GIF (two-pass palettegen/paletteuse)
felx render proj.felx --out out.gif --format gif --gif-palette 64 --gif-dither floyd

# PNG / EXR image sequences (--out is a directory)
felx render proj.felx --out frames/ --format png --png-pattern 'frame_{frame:05}.png'
felx render proj.felx --out frames/ --format exr

# Audio-only WAV of the comp's mixed bus
felx render proj.felx --out master.wav --format wav --wav-depth 24
```

Encoder flags (h264 / h265 / prores):

| Flag | Meaning |
|---|---|
| `--crf <0..51>` | CRF quality target (h264/h265; lower = better, 18 ≈ visually lossless) |
| `--bitrate <bps>` | Target bitrate — switches to CBR (or VBR with `--max-bitrate`) |
| `--max-bitrate <bps>` | Max bitrate for VBR / VBV |
| `--preset <name>` | Encoder speed/quality preset (`ultrafast`…`veryslow`) |
| `--profile <name>` | `baseline`/`main`/`high` (h264), `main`/`main10` (h265), `proxy`/`lt`/`standard`/`hq`/`4444` (prores) |
| `--gop <frames>` | Keyframe interval |
| `--hw <auto\|nvenc\|vaapi\|videotoolbox>` | Hardware encoder; falls back to software if unavailable |

`felx help` prints the same reference. Progress is reported as structured tracing events (target `felx::progress`), so scripts wrapping the CLI can scrape completion percentage.

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

- [`docs/decisions/`](decisions/) — ADRs (project file format, UI framework).
- [`NOTICES`](../NOTICES) — third-party attribution.
- `effects/<id>/README.md` — per-effect parameter reference (also available from the GUI's Help menu).
