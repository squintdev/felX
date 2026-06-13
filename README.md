# analog-felx

A GPU-accelerated, node-and-timeline video compositor written in Rust + wgpu, aimed at replacing the After Effects workflows the author uses for video glitch art.

## Prerequisites

- **Rust** stable (≥ 1.85; uses 2024 edition)
- **FFmpeg dev libraries** (linked by `felx-media`):
  - Arch: `sudo pacman -S ffmpeg pkgconf clang`
  - Debian/Ubuntu: `sudo apt install ffmpeg libavcodec-dev libavformat-dev libavutil-dev libavfilter-dev libavdevice-dev libswscale-dev libswresample-dev libva-dev libdrm-dev pkg-config clang`
  - macOS: `brew install ffmpeg pkg-config`
- A **wgpu-compatible adapter** (Vulkan / Metal / DX12). For headless / CI environments without a GPU, the test suite falls back to lavapipe / WARP via `FELX_SOFTWARE_GPU=1`.
- For audio playback (F-052), a working **system audio device** (cpal opens the default output).

## Run the GUI

```bash
cargo run -p felx-app
```

This opens a 1280×720 / 30fps default project with a slate-blue solid layer and a Gain effect on top. Things to try:

- **Click the layer** in the Layers panel (left), then dial the Gain slider in the Effects panel (right).
- **Click the `⏱` stopwatch** next to a Float slider to convert it to an animated curve. Drag the slider at different playhead positions to add keyframes; an inline mini-curve appears below the slider.
- **Toggle `▲ Graph`** in the bottom transport bar for the full graph editor across the comp duration. Shift-click for multi-select, `1`/`2`/`3`/`4`/`5` to set interp on the selection.
- **Click a preset** in the top bar (`CRT Consumer Trinitron`, `VHS on CRT`, etc.) — it adds the canonical effect chain to the selected layer.
- **`+ Adj`** in Layers to add an Adjustment layer; effects on it apply to everything beneath.
- **`+ Rect` / `+ Ellipse`** in the Masks section to add a mask. Click `Pen` then click on the viewer to draw a custom mask (click near the first anchor to close).
- **Shift-drag on the viewer** to set a region of interest. **Esc** clears.
- **Help menu** (top-right) → pick an effect to see its README in a popup.
- **Space / ←/→** for play / step. **Hot reload**: edit `effects/gain/effect.wgsl` while the app is running and the change applies on save.

If you don't have a GPU at all, prefix with `FELX_SOFTWARE_GPU=1`:

```bash
FELX_SOFTWARE_GPU=1 cargo run -p felx-app
```

## Run the CLI

```bash
cargo run -p felx-cli -- help
```

Render a `.felx` project to a video / image-sequence / audio file:

```bash
cargo run -p felx-cli -- render path/to/project.felx --out out.mp4 --format h264
cargo run -p felx-cli -- render path/to/project.felx --out out.gif --format gif --gif-palette 64 --gif-dither floyd
cargo run -p felx-cli -- render path/to/project.felx --out frames/ --format png --png-pattern 'frame_{frame:05}.png'
cargo run -p felx-cli -- render path/to/project.felx --out master.wav --format wav --wav-depth 24
```

`felx help` lists every flag.

The repo doesn't currently ship example `.felx` files (the GUI's default project is in-memory only). To produce one for the CLI, run the GUI, set up a comp, and use `Project::save` from a quick test or `cargo run -p felx-cli` after wiring a save command — for now the easiest path to a `.felx` is to write one by hand or via a small `cargo run --example` snippet. (A `felx new` subcommand is on the polish list.)

## Test

The full suite (unit + integration + visual-regression) runs against your local GPU:

```bash
cargo test --workspace --all-targets --no-fail-fast
```

Force the software adapter (matches what CI does — guarantees identical results regardless of which GPU you have):

```bash
FELX_SOFTWARE_GPU=1 cargo test --workspace --all-targets --no-fail-fast
```

Run a single test:

```bash
cargo test -p felx-render --test masks rectangle_mask_gates_layer_alpha
cargo test -p felx-core --lib media::mixer
```

Update visual-regression goldens after an intentional render change (review the new PNGs in `crates/felx-render/tests/golden/` before committing):

```bash
FELX_UPDATE_GOLDEN=1 cargo test -p felx-render --test cc_toner
```

Lint / format (CI gates on these):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Verbose tracing while running anything:

```bash
RUST_LOG=felx=debug cargo run -p felx-app
```

## What's in the workspace

```
felx-core    Domain model (Project / Composition / Layer / Curve / params /
             mask / mixer / waveform / av_sync). Pure data + serde, no wgpu.
felx-render  wgpu compositor. Owns the renderer, every per-effect pipeline,
             the frame cache, the masks rasterizer, the export paths
             (PNG / EXR / GIF / WAV / video).
felx-media   ffmpeg wrappers: video decode + encode (H.264 / H.265 / ProRes
             with full controls + hw paths), audio decode, WAV writer.
felx-test    Visual regression harness: `golden!` macro, diffing, writes
             target/visual-diffs on failure.
felx-cli     `felx` binary — headless renders, dispatched to the right
             export pipeline per --format.
felx-app     `felx-app` GUI — eframe host, shares its wgpu device with the
             compositor, owns the playback loop, panels, masks UI, render
             queue, crash reporter, autosave.
```

## See also

- [`docs/USER_GUIDE.md`](docs/USER_GUIDE.md) — keyboard shortcuts, effect catalog, render formats, troubleshooting.
- [`CLAUDE.md`](CLAUDE.md) — codebase tour for contributors.
- [`docs/decisions/`](docs/decisions/) — ADRs (project file format → RON, UI framework → egui).
- [`NOTICES`](NOTICES) — third-party attribution.

## License

Dual-licensed under MIT and Apache-2.0; see `LICENSE-MIT`, `LICENSE-APACHE`.
