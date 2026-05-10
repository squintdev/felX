# ADR 0002 — UI framework

**Status:** Accepted
**Date:** 2026-05-09

## Context

analog-felx is a single-window desktop tool that needs to host:

- a **viewer**: a wgpu-rendered surface showing the active composition
- a **timeline** with layered tracks, scrubbable, with inline keyframe editors
- an **effects panel** that auto-generates controls from each effect's
  parameter manifest (sliders, color pickers, dropdowns, nested optional
  sub-groups), all live-editable
- a **curve editor** with draggable anchor points and bezier tangents
- a **render queue panel**
- keyboard-driven workflows where possible

It runs on Linux, macOS, and Windows. Rust-native. No mobile or web target.

The render path is wgpu (decided implicitly by the PRD — to be re-confirmed
in F-016). Whatever UI framework we pick must let the wgpu compositor's
output texture appear inside the viewer area without an awkward inter-process
hop or readback.

## Considered options

### egui (via eframe)

Immediate-mode. Every frame, the application code rebuilds the UI; egui
diffs against the previous frame and repaints only what changed.

- **wgpu integration**: first-class via `eframe::Renderer::Wgpu`. The same
  `wgpu::Device` and `Queue` are shared between egui's draw path and the
  application's compositor. A wgpu texture can be embedded directly in an
  egui widget via `egui_wgpu::Renderer::register_native_texture`.
- **Custom drawing**: `egui::Painter` exposes lines, rects, polygons, text.
  Sufficient for a curve editor and a timeline. The `egui_plot` crate is
  available if we want a head start on plotting.
- **Live-editable parameters**: ergonomic. `ui.add(Slider::new(&mut x, range).text("foo"))`
  is one line per parameter. Auto-generating panels from a parameter
  manifest is a small visitor pattern.
- **Keyboard**: `ctx.input(|i| i.key_pressed(Key::Space))`. Vim-style
  modal bindings would be a thin layer on top.
- **Cross-platform**: eframe handles winit + wayland/x11/cocoa/win32. Just
  works.
- **Spike result**: tiny prototype with a side panel of sliders, a custom-
  painted central viewport, and keyboard handling compiled and ran on Linux
  with eframe 0.32 / egui 0.32 / transitively wgpu 25. About 70 lines of
  application code.

### iced

Retained-mode, Elm-style message-passing. Application is a state +
`update(state, message) -> state` + `view(state) -> Element`.

- **wgpu integration**: `iced_wgpu` exists; embedding an external wgpu
  texture into an iced widget is doable but more bespoke than eframe.
- **Custom widgets**: implementing the `Widget` trait per custom control.
  More boilerplate than egui's immediate-mode painting.
- **Live-editable parameters**: each parameter slider becomes a separate
  `Message` variant in the app's update loop. For tens to hundreds of effect
  parameters this gets verbose.
- **Keyboard**: supported, slightly more verbose to wire than egui.
- **Cross-platform**: yes.
- **Pro**: prettier defaults, good for "polished consumer app" aesthetic.
- **Con for us**: the polish-cost ratio is wrong for a workflow tool with
  many small live controls.

### Custom wgpu UI layer

Hand-roll layout, focus, scrolling, text input, accessibility. Maximum
control, maximum work.

- For a solo developer building a workflow tool, this is a months-long
  detour with no payoff that egui doesn't already provide.
- Would only make sense if egui and iced both had specific blockers we
  couldn't work around. Neither does.

## Decision

**Use egui via eframe.** All UI in the `felx-app` binary.

- `felx-app` depends on `eframe` with the `wgpu`, `wayland`, `x11`
  features enabled (default). On macOS / Windows, eframe selects the
  platform-appropriate winit backend automatically.
- The compositor's output texture is registered with the `egui_wgpu` renderer
  and drawn as a widget in the viewer panel.
- Parameter panels are generated procedurally from the effect manifest
  (F-019 / F-026) using small visitor functions.
- Custom controls (curve editor, timeline track, mask pen tool) extend
  `egui::Painter` and consume `egui::Response`.

## Consequences

- **wgpu version pin.** eframe transitively pins a specific wgpu version
  (`25.x` for eframe 0.32). The render path's `felx-render` crate must use
  the same wgpu major to share devices and queues without ABI grief. When
  bumping eframe, expect to bump wgpu in lockstep.
- **Single device, shared queue.** eframe owns the wgpu device. The
  compositor borrows it via the `eframe::CreationContext::wgpu_render_state`
  hook at app init.
- **Headless rendering** (CLI, F-109; CI tests) does not use eframe — it
  builds its own wgpu device standalone. Keep the compositor's device
  acquisition behind a thin trait so both paths work.
- **Theme**: ship a custom dark theme tuned for video work (deep
  near-black, low-saturation accents). Off-the-shelf egui dark is a
  reasonable starting point.
- **Accessibility**: egui+AccessKit is available behind the `accesskit`
  feature; defer until later if needed.
- **Testing UI**: keep app logic in pure-data update functions where
  possible so tests don't need a windowing stack. `egui::__run_test_ctx`
  exists for headless egui rendering when needed.

## Out of scope

- Vim-style modal bindings: nice-to-have, layer on top once the basic
  bindings settle.
- Multi-window: deferred. egui supports it via eframe `viewports` if needed
  later.
- Web build: deferred. egui supports wasm; would require dropping the
  wgpu compositor or running it through a different path.

## References

- [egui](https://github.com/emilk/egui)
- [eframe wgpu renderer](https://docs.rs/eframe/latest/eframe/struct.Renderer.html)
- [iced](https://github.com/iced-rs/iced)
- Spike code lived in `/tmp/felx-ui-spike-egui/` during the decision; deleted.
