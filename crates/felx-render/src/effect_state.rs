//! Per-effect-instance frame-to-frame state (F-070).
//!
//! Effects that need to remember the previous frame (CRT phosphor decay,
//! datamosh, optical-flow trails) acquire a ping-pong texture pair from
//! [`EffectStateRegistry`]. Each call returns the read texture (last
//! frame's output), the write texture (this frame's destination), and a
//! `was_reset` flag the effect can use to seed initial state.
//!
//! Reset semantics: any non-monotonic frame advance (the frame just
//! requested isn't `last_frame + 1`) clears both textures and flags the
//! reset. So scrubbing, looping, jumping, or rendering a single frame in
//! isolation all give the effect a clean slate.

use crate::Renderer;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateKey {
    pub layer_id: u32,
    pub effect_index: usize,
    pub effect_id: String,
}

impl StateKey {
    pub fn new(layer_id: u32, effect_index: usize, effect_id: impl Into<String>) -> Self {
        Self {
            layer_id,
            effect_index,
            effect_id: effect_id.into(),
        }
    }
}

struct EffectState {
    tex_a: wgpu::Texture,
    tex_b: wgpu::Texture,
    /// When true the next acquire returns (a, b); when false (b, a). Flips
    /// after every acquire.
    use_a_as_read: bool,
    last_frame: Option<u32>,
    width: u32,
    height: u32,
}

/// What an effect gets when it acquires its state for a frame.
pub struct StateAcquired<'a> {
    /// Last frame's output. Read-only — sample as input.
    pub read: &'a wgpu::Texture,
    /// Where this frame's output should land. Write target.
    pub write: &'a wgpu::Texture,
    /// True if the read texture was just cleared (first frame, or seek).
    pub was_reset: bool,
}

#[derive(Default)]
pub struct EffectStateRegistry {
    states: HashMap<StateKey, EffectState>,
}

impl EffectStateRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire ping-pong state for one effect, one frame.
    ///
    /// On a fresh key OR a non-monotonic frame transition (any jump that
    /// isn't `last + 1`), both textures are cleared to transparent black
    /// and `was_reset` is true. After the call, `last_frame` is bumped to
    /// `current_frame` and the read/write roles flip for the next call.
    pub fn acquire<'a>(
        &'a mut self,
        renderer: &Renderer,
        key: StateKey,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
        current_frame: u32,
    ) -> StateAcquired<'a> {
        let needs_realloc = match self.states.get(&key) {
            Some(s) => s.width != w || s.height != h,
            None => true,
        };
        if needs_realloc {
            let tex_a = make_state_texture(renderer, w, h, format, "effect_state.a");
            let tex_b = make_state_texture(renderer, w, h, format, "effect_state.b");
            self.states.insert(
                key.clone(),
                EffectState {
                    tex_a,
                    tex_b,
                    use_a_as_read: true,
                    last_frame: None,
                    width: w,
                    height: h,
                },
            );
        }

        let state = self.states.get_mut(&key).expect("just inserted if missing");
        let monotonic = matches!(state.last_frame, Some(prev) if current_frame == prev + 1);
        let was_reset = !monotonic;
        if was_reset {
            clear_state_texture(renderer, &state.tex_a);
            clear_state_texture(renderer, &state.tex_b);
            state.use_a_as_read = true;
        }
        state.last_frame = Some(current_frame);
        let (read, write) = if state.use_a_as_read {
            (&state.tex_a, &state.tex_b)
        } else {
            (&state.tex_b, &state.tex_a)
        };
        state.use_a_as_read = !state.use_a_as_read;
        StateAcquired {
            read,
            write,
            was_reset,
        }
    }

    pub fn reset(&mut self, key: &StateKey) {
        self.states.remove(key);
    }

    pub fn clear(&mut self) {
        self.states.clear();
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

fn make_state_texture(
    renderer: &Renderer,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::Texture {
    renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

fn clear_state_texture(renderer: &Renderer, tex: &wgpu::Texture) {
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = renderer
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("effect_state.clear"),
        });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("effect_state.clear.pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        occlusion_query_set: None,
        timestamp_writes: None,
    });
    renderer.queue().submit(Some(encoder.finish()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_io::COMPOSITOR_FORMAT;
    use crate::{Renderer, RendererOptions};

    fn try_renderer() -> Option<Renderer> {
        Renderer::new_headless(RendererOptions {
            allow_software_fallback: true,
            ..Default::default()
        })
        .ok()
    }

    #[test]
    fn first_acquire_reports_reset() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut reg = EffectStateRegistry::new();
        let key = StateKey::new(1, 0, "crt");
        let s = reg.acquire(&r, key, 16, 16, COMPOSITOR_FORMAT, 0);
        assert!(s.was_reset);
    }

    #[test]
    fn monotonic_acquire_does_not_reset() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut reg = EffectStateRegistry::new();
        let key = StateKey::new(1, 0, "crt");
        let _ = reg.acquire(&r, key.clone(), 16, 16, COMPOSITOR_FORMAT, 5);
        let s2 = reg.acquire(&r, key, 16, 16, COMPOSITOR_FORMAT, 6);
        assert!(!s2.was_reset);
    }

    #[test]
    fn frame_jump_triggers_reset() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut reg = EffectStateRegistry::new();
        let key = StateKey::new(1, 0, "crt");
        let _ = reg.acquire(&r, key.clone(), 16, 16, COMPOSITOR_FORMAT, 5);
        // Skip from 5 to 30 — a scrub.
        let s = reg.acquire(&r, key, 16, 16, COMPOSITOR_FORMAT, 30);
        assert!(s.was_reset);
    }

    #[test]
    fn dim_change_reallocs_and_resets() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut reg = EffectStateRegistry::new();
        let key = StateKey::new(1, 0, "crt");
        let _ = reg.acquire(&r, key.clone(), 16, 16, COMPOSITOR_FORMAT, 5);
        let s = reg.acquire(&r, key, 32, 32, COMPOSITOR_FORMAT, 6);
        assert!(
            s.was_reset,
            "dim change should reset (last_frame goes back to None)"
        );
    }

    #[test]
    fn read_write_roles_swap_each_call() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut reg = EffectStateRegistry::new();
        let key = StateKey::new(1, 0, "crt");
        let read_addr_1 = {
            let s = reg.acquire(&r, key.clone(), 16, 16, COMPOSITOR_FORMAT, 5);
            s.read as *const _
        };
        let read_addr_2 = {
            let s = reg.acquire(&r, key.clone(), 16, 16, COMPOSITOR_FORMAT, 6);
            s.read as *const _
        };
        let read_addr_3 = {
            let s = reg.acquire(&r, key, 16, 16, COMPOSITOR_FORMAT, 7);
            s.read as *const _
        };
        // After two flips we're back to the original read texture.
        assert_ne!(read_addr_1, read_addr_2, "roles must swap between frames");
        assert_eq!(read_addr_1, read_addr_3, "should swap back after two flips");
    }
}
