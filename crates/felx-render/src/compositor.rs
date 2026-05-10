//! Single-layer compositor (M0). M3's F-040 generalizes this to arbitrary
//! layer stacks with blending modes and track mattes; here we render the
//! first visible layer's source through its effect stack.

use crate::cpu_pass::run_cpu_pass;
use crate::effects::gain::{Gain, GainParams};
use crate::effects::invert::invert_in_place;
use crate::frame_cache::{CacheKey, FrameCache, hash_effect_stack};
use crate::texture_io::{COMPOSITOR_FORMAT, upload_image};
use crate::{Renderer, RendererError};
use felx_core::model::{CompId, Effect, Layer, LayerKind, Project};
use image::{ImageBuffer, Rgba, RgbaImage};
use tracing::{debug, debug_span, info_span, warn};

#[derive(Debug)]
pub enum CompositorError {
    UnknownComposition,
    NoVisibleLayer,
    UnknownAsset,
    UnsupportedLayerKind(&'static str),
    AssetIo(std::io::Error),
    AssetDecode(image::ImageError),
    RendererInit(RendererError),
}

impl std::fmt::Display for CompositorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompositorError::UnknownComposition => write!(f, "unknown composition"),
            CompositorError::NoVisibleLayer => write!(f, "no layer visible at this frame"),
            CompositorError::UnknownAsset => write!(f, "layer references unknown asset"),
            CompositorError::UnsupportedLayerKind(k) => write!(f, "unsupported layer kind: {k}"),
            CompositorError::AssetIo(e) => write!(f, "asset io: {e}"),
            CompositorError::AssetDecode(e) => write!(f, "asset decode: {e}"),
            CompositorError::RendererInit(e) => write!(f, "renderer init: {e}"),
        }
    }
}

impl std::error::Error for CompositorError {}

/// Tiny dimension/format-keyed texture pool. Acquire on demand, release on
/// return; acquire returns a free texture if one matches, otherwise creates
/// a new one. Single-threaded.
#[derive(Default)]
pub struct TexturePool {
    free: Vec<(u32, u32, wgpu::TextureFormat, wgpu::Texture)>,
    label: &'static str,
}

impl TexturePool {
    pub fn new(label: &'static str) -> Self {
        Self {
            free: Vec::new(),
            label,
        }
    }

    pub fn acquire(
        &mut self,
        renderer: &Renderer,
        w: u32,
        h: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::Texture {
        if let Some(idx) = self
            .free
            .iter()
            .position(|(pw, ph, pf, _)| *pw == w && *ph == h && *pf == format)
        {
            let (_, _, _, t) = self.free.swap_remove(idx);
            return t;
        }
        debug!(target: "felx::pool", w, h, ?format, "allocating new pool texture");
        renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(self.label),
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

    pub fn release(&mut self, texture: wgpu::Texture) {
        let w = texture.width();
        let h = texture.height();
        let f = texture.format();
        self.free.push((w, h, f, texture));
    }

    pub fn len(&self) -> usize {
        self.free.len()
    }

    pub fn is_empty(&self) -> bool {
        self.free.is_empty()
    }
}

pub struct Compositor {
    renderer: Renderer,
    gain: Gain,
    pool: TexturePool,
    cache: FrameCache,
}

impl Compositor {
    pub fn new(renderer: Renderer) -> Self {
        Self::with_cache_capacity(renderer, 64)
    }

    pub fn with_cache_capacity(renderer: Renderer, cache_entries: usize) -> Self {
        let gain = Gain::new(&renderer, COMPOSITOR_FORMAT);
        Self {
            renderer,
            gain,
            pool: TexturePool::new("compositor-pool"),
            cache: FrameCache::new(cache_entries),
        }
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn pool(&self) -> &TexturePool {
        &self.pool
    }

    pub fn cache(&self) -> &FrameCache {
        &self.cache
    }

    pub fn cache_mut(&mut self) -> &mut FrameCache {
        &mut self.cache
    }

    /// Hot-swap the Gain pipeline. Used by the WGSL hot-reload path; the
    /// caller is responsible for invalidating the cache afterward.
    pub fn replace_gain(&mut self, gain: Gain) {
        self.gain = gain;
    }

    /// Render through the cache: returns a cached texture if available,
    /// otherwise renders and inserts.
    pub fn render_cached(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let comp = project
            .composition(comp_id)
            .ok_or(CompositorError::UnknownComposition)?;
        let layer = comp
            .layers
            .iter()
            .find(|l| frame >= l.in_frame && frame < l.out_frame)
            .ok_or(CompositorError::NoVisibleLayer)?;
        let key = Self::cache_key(comp_id, frame, layer);
        if let Some(tex) = self.cache.get(key) {
            return Ok(tex);
        }
        let tex = self.render(project, comp_id, frame)?;
        self.cache.insert(key, tex.clone());
        Ok(tex)
    }

    fn cache_key(comp_id: CompId, frame: u32, layer: &Layer) -> CacheKey {
        let stack_hash = hash_effect_stack(layer.effects.iter());
        CacheKey::new(comp_id.0, frame, stack_hash)
    }

    /// Render the first layer of the named composition that's visible at
    /// `frame`, applying its effect stack. The output texture is owned by
    /// the caller (not pooled).
    pub fn render(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let _span = info_span!("compositor.render", frame, comp = comp_id.0).entered();

        let comp = project
            .composition(comp_id)
            .ok_or(CompositorError::UnknownComposition)?;
        let layer = comp
            .layers
            .iter()
            .find(|l| frame >= l.in_frame && frame < l.out_frame)
            .ok_or(CompositorError::NoVisibleLayer)?;

        let source_image = {
            let _s = debug_span!("compositor.resolve_source").entered();
            self.resolve_layer_source(project, &layer.kind, comp.width, comp.height)?
        };
        let mut current_tex = upload_image(&self.renderer, &source_image);

        for eff in &layer.effects {
            if !eff.enabled {
                continue;
            }
            current_tex = self.apply_effect(eff, current_tex, comp.width, comp.height)?;
        }

        Ok(current_tex)
    }

    fn apply_effect(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let _span = debug_span!("compositor.effect", id = %eff.id).entered();

        match eff.id.as_str() {
            "gain" => {
                let gain_value = eff.values.float("gain").unwrap_or(1.0);
                let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
                let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
                let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

                let mut encoder = self.renderer.device().create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("compositor.gain"),
                    },
                );
                self.gain.render(
                    &self.renderer,
                    &mut encoder,
                    &in_view,
                    &out_view,
                    GainParams::new(gain_value),
                );
                self.renderer.queue().submit(Some(encoder.finish()));

                self.pool.release(input);
                Ok(output)
            }
            "invert" => {
                let output = run_cpu_pass(&self.renderer, &input, "invert", invert_in_place);
                self.pool.release(input);
                Ok(output)
            }
            other => {
                warn!(effect_id = other, "skipping unknown effect");
                Ok(input)
            }
        }
    }

    fn resolve_layer_source(
        &self,
        project: &Project,
        kind: &LayerKind,
        comp_w: u32,
        comp_h: u32,
    ) -> Result<RgbaImage, CompositorError> {
        match kind {
            LayerKind::Image { asset } => {
                let a = project.asset(*asset).ok_or(CompositorError::UnknownAsset)?;
                let img = image::open(&a.path).map_err(CompositorError::AssetDecode)?;
                Ok(img.to_rgba8())
            }
            LayerKind::Solid { color } => {
                let r = (color[0].clamp(0.0, 1.0) * 255.0).round() as u8;
                let g = (color[1].clamp(0.0, 1.0) * 255.0).round() as u8;
                let b = (color[2].clamp(0.0, 1.0) * 255.0).round() as u8;
                let a = (color[3].clamp(0.0, 1.0) * 255.0).round() as u8;
                Ok(ImageBuffer::from_pixel(comp_w, comp_h, Rgba([r, g, b, a])))
            }
            LayerKind::Null | LayerKind::Adjustment => {
                // Null contributes nothing visible; Adjustment is handled at
                // the multi-layer compositor level (M3, F-060). Use a fully
                // transparent buffer for now.
                Ok(ImageBuffer::from_pixel(comp_w, comp_h, Rgba([0, 0, 0, 0])))
            }
            LayerKind::Video { .. } => Err(CompositorError::UnsupportedLayerKind("Video")),
            LayerKind::Audio { .. } => Err(CompositorError::UnsupportedLayerKind("Audio")),
            LayerKind::Composition { .. } => {
                Err(CompositorError::UnsupportedLayerKind("Composition"))
            }
        }
    }
}
