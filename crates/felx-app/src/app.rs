//! The eframe `App` impl. Owns the project, the compositor, and the egui
//! texture handle that mirrors the compositor's output for display.

use crate::manifests::ManifestRegistry;
use crate::panels::effects::{self, EffectsAction};
use crate::panels::layers::{self, LayerAction};
use crate::panels::transport::{self, TransportAction};
use crate::playback::Playhead;
use eframe::egui_wgpu::RenderState;
use eframe::{App, CreationContext, Frame};
use egui::{CentralPanel, Color32, Context, Sense, SidePanel, TextureId, TopBottomPanel, Vec2};
use felx_core::model::{CompId, Effect, LayerId, Project};
use felx_render::compositor::{Compositor, CompositorError};
use felx_render::{AdapterInfo, Renderer};
use tracing::{error, info};

pub struct FelxApp {
    project: Project,
    comp_id: CompId,
    playhead: Playhead,
    compositor: Compositor,
    selected_layer: Option<LayerId>,
    manifests: ManifestRegistry,
    /// Texture currently registered with egui's wgpu renderer. Replaced
    /// every time the compositor produces a new output texture.
    egui_texture: Option<TextureId>,
    /// Set any time the compositor needs to re-render (layer or parameter
    /// edit, scrub, playback advance). Cleared by [`ensure_frame_rendered`].
    render_dirty: bool,
}

#[derive(Debug)]
pub enum AppInitError {
    NoWgpuRenderState,
}

impl std::fmt::Display for AppInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppInitError::NoWgpuRenderState => write!(
                f,
                "eframe did not provide a wgpu render state — was the wgpu \
                 feature enabled and the wgpu renderer selected?"
            ),
        }
    }
}

impl std::error::Error for AppInitError {}

impl FelxApp {
    pub fn new(cc: &CreationContext<'_>) -> Result<Self, AppInitError> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or(AppInitError::NoWgpuRenderState)?;
        let renderer = build_renderer(render_state);
        let compositor = Compositor::new(renderer);
        let manifests = ManifestRegistry::load_builtins();
        let (project, comp_id) = default_project(&manifests);
        let comp = project.composition(comp_id).expect("comp exists");
        let playhead = Playhead::new(comp.framerate.as_fps(), comp.duration_frames);
        info!(
            comp = comp_id.0,
            manifests = manifests.len(),
            "felx-app initialized"
        );
        Ok(Self {
            project,
            comp_id,
            playhead,
            compositor,
            selected_layer: None,
            manifests,
            egui_texture: None,
            render_dirty: true,
        })
    }

    fn apply_transport_actions(&mut self, actions: Vec<TransportAction>) {
        if actions.is_empty() {
            return;
        }
        let mut moved = false;
        for action in actions {
            match action {
                TransportAction::Toggle => self.playhead.toggle(),
                TransportAction::StepForward => {
                    self.playhead.step_forward();
                    moved = true;
                }
                TransportAction::StepBackward => {
                    self.playhead.step_backward();
                    moved = true;
                }
                TransportAction::Seek(f) => {
                    self.playhead.seek(f);
                    moved = true;
                }
            }
        }
        if moved {
            self.render_dirty = true;
        }
    }

    fn apply_effects_actions(&mut self, actions: Vec<EffectsAction>) {
        if actions.is_empty() {
            return;
        }
        let Some(layer_id) = self.selected_layer else {
            return;
        };
        let Some(comp) = self.project.composition_mut(self.comp_id) else {
            return;
        };
        let Some(layer) = comp.layer_mut(layer_id) else {
            return;
        };
        for action in actions {
            match action {
                EffectsAction::SetValue {
                    effect_index,
                    id,
                    value,
                } => {
                    if let Some(eff) = layer.effects.get_mut(effect_index) {
                        eff.values.set(id, value);
                    }
                }
                EffectsAction::ToggleEnabled {
                    effect_index,
                    enabled,
                } => {
                    if let Some(eff) = layer.effects.get_mut(effect_index) {
                        eff.enabled = enabled;
                    }
                }
            }
        }
        self.render_dirty = true;
        self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
    }

    fn apply_layer_actions(&mut self, actions: Vec<LayerAction>) {
        if actions.is_empty() {
            return;
        }
        let dirty = !actions.is_empty();
        for action in actions {
            match action {
                LayerAction::Select(id) => self.selected_layer = id,
                LayerAction::AddSolid => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        let id = comp.add_solid("Solid", [0.5, 0.5, 0.5, 1.0]);
                        self.selected_layer = Some(id);
                    }
                }
                LayerAction::Delete(id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.remove_layer(id);
                    }
                    if self.selected_layer == Some(id) {
                        self.selected_layer = None;
                    }
                }
                LayerAction::MoveUp(id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.move_layer_up(id);
                    }
                }
                LayerAction::MoveDown(id) => {
                    if let Some(comp) = self.project.composition_mut(self.comp_id) {
                        comp.move_layer_down(id);
                    }
                }
            }
        }
        if dirty {
            self.render_dirty = true;
            self.compositor.cache_mut().invalidate_comp(self.comp_id.0);
        }
    }

    fn ensure_frame_rendered(&mut self, render_state: &RenderState) {
        if !self.render_dirty && self.egui_texture.is_some() {
            return;
        }
        let frame = self.playhead.current_frame();
        let texture = match self
            .compositor
            .render_cached(&self.project, self.comp_id, frame)
        {
            Ok(t) => t,
            Err(CompositorError::NoVisibleLayer) => {
                // Empty playhead; show a placeholder later. For now leave
                // texture unset.
                return;
            }
            Err(e) => {
                error!(error = %e, "compositor render failed");
                return;
            }
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut renderer = render_state.renderer.write();
        let id =
            renderer.register_native_texture(&render_state.device, &view, wgpu::FilterMode::Linear);
        if let Some(old) = self.egui_texture.replace(id) {
            renderer.free_texture(&old);
        }
        self.render_dirty = false;
    }

    fn comp_aspect(&self) -> f32 {
        let comp = self.project.composition(self.comp_id).expect("comp exists");
        comp.width as f32 / comp.height as f32
    }
}

impl App for FelxApp {
    fn update(&mut self, ctx: &Context, frame: &mut Frame) {
        let Some(render_state) = frame.wgpu_render_state() else {
            return;
        };
        let render_state = render_state.clone();

        // Advance the playhead off real elapsed time before drawing the UI
        // so the transport bar shows the new frame.
        if self.playhead.tick() {
            self.render_dirty = true;
        }

        let transport_actions = TopBottomPanel::bottom("transport")
            .show(ctx, |ui| transport::show(ui, &self.playhead))
            .inner;
        self.apply_transport_actions(transport_actions);

        let layer_actions = SidePanel::left("layers")
            .resizable(true)
            .default_width(220.0)
            .min_width(180.0)
            .show(ctx, |ui| {
                let comp = self.project.composition(self.comp_id).expect("comp exists");
                layers::show(ui, comp, self.selected_layer)
            })
            .inner;
        self.apply_layer_actions(layer_actions);

        let effects_actions = SidePanel::right("effects")
            .resizable(true)
            .default_width(280.0)
            .min_width(220.0)
            .show(ctx, |ui| {
                let comp = self.project.composition(self.comp_id).expect("comp exists");
                let selected_layer = self
                    .selected_layer
                    .and_then(|id| comp.layers.iter().find(|l| l.id == id));
                effects::show(ui, &self.manifests, selected_layer)
            })
            .inner;
        self.apply_effects_actions(effects_actions);

        self.ensure_frame_rendered(&render_state);

        // Keep the loop running while playing so tick() fires regularly.
        if let Some(after) = self.playhead.repaint_after() {
            ctx.request_repaint_after(after);
        }

        CentralPanel::default()
            .frame(egui::Frame::default().fill(Color32::from_gray(15)))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let aspect = self.comp_aspect();
                let size = fit_aspect(avail, aspect);
                let (rect, _resp) = ui.allocate_exact_size(size, Sense::hover());

                if let Some(id) = self.egui_texture {
                    let painter = ui.painter_at(rect);
                    painter.image(
                        id,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            egui::RichText::new("(no frame)")
                                .color(Color32::GRAY)
                                .italics(),
                        );
                    });
                }
            });
    }
}

fn build_renderer(render_state: &RenderState) -> Renderer {
    let info = AdapterInfo::from(render_state.adapter.get_info());
    Renderer::from_borrowed(
        render_state.device.clone(),
        render_state.queue.clone(),
        info,
    )
}

/// Default placeholder project until file-open lands. A 1280x720 / 30fps
/// comp with a slate-blue solid layer and a Gain effect (defaulted from
/// the manifest if loaded, otherwise the bare `Effect::new` default).
fn default_project(manifests: &ManifestRegistry) -> (Project, CompId) {
    let mut project = Project::new();
    let comp_id = project.add_composition("preview", 1280, 720);
    let comp = project.composition_mut(comp_id).unwrap();
    comp.duration_frames = 600;
    comp.background = [0.0, 0.0, 0.0, 1.0];
    let layer = comp.add_solid("background", [0.18, 0.22, 0.32, 1.0]);
    let gain_effect = manifests
        .get("gain")
        .map(Effect::from_manifest)
        .unwrap_or_else(|| Effect::new("gain"));
    comp.push_effect(layer, gain_effect);
    (project, comp_id)
}

/// Largest box fitting `avail` while preserving `aspect` (= w/h).
fn fit_aspect(avail: Vec2, aspect: f32) -> Vec2 {
    if avail.x <= 0.0 || avail.y <= 0.0 || aspect <= 0.0 {
        return Vec2::ZERO;
    }
    if avail.x / avail.y > aspect {
        Vec2::new(avail.y * aspect, avail.y)
    } else {
        Vec2::new(avail.x, avail.x / aspect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_aspect_handles_wider_avail() {
        // avail wider than aspect → fit by height.
        let s = fit_aspect(Vec2::new(800.0, 400.0), 1.0);
        assert!((s.x - 400.0).abs() < 0.001);
        assert!((s.y - 400.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_handles_taller_avail() {
        // avail taller than aspect → fit by width.
        let s = fit_aspect(Vec2::new(400.0, 800.0), 1.0);
        assert!((s.x - 400.0).abs() < 0.001);
        assert!((s.y - 400.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_widescreen_in_square_box() {
        // 16:9 in a square: should be width-limited.
        let s = fit_aspect(Vec2::new(800.0, 800.0), 16.0 / 9.0);
        assert!((s.x - 800.0).abs() < 0.001);
        assert!((s.y - 450.0).abs() < 0.001);
    }

    #[test]
    fn fit_aspect_zero_inputs_return_zero() {
        assert_eq!(fit_aspect(Vec2::ZERO, 1.0), Vec2::ZERO);
        assert_eq!(fit_aspect(Vec2::new(100.0, 0.0), 1.0), Vec2::ZERO);
        assert_eq!(fit_aspect(Vec2::new(100.0, 100.0), 0.0), Vec2::ZERO);
    }
}
