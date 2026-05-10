//! Single-layer compositor (M0). M3's F-040 generalizes this to arbitrary
//! layer stacks with blending modes and track mattes; here we render the
//! first visible layer's source through its effect stack.

use crate::cpu_pass::run_cpu_pass;
use crate::effects::cc_toner::{CcToner, CcTonerParams, TonesMode};
use crate::effects::gain::{Gain, GainParams};
use crate::effects::invert::invert_in_place;
use crate::frame_cache::{CacheKey, FrameCache, hash_effect_stack};
use crate::srgb_wrap::SrgbWrap;
use crate::texture_io::{COMPOSITOR_FORMAT, upload_image};
use crate::transform_pass::{TransformParams, TransformPass};
use crate::{Renderer, RendererError};
use felx_core::model::{CompId, Effect, Layer, LayerKind, Project};
use image::{ImageBuffer, Rgba, RgbaImage, imageops};
use tracing::{debug, debug_span, info_span, warn};

/// Preview-resolution scale factor. Renders at `comp_dims / scale_div` in
/// each axis. The cache keys frames per scale, so toggling Half ↔ Full
/// reuses both populations on the next swap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewScale {
    Full,
    #[default]
    Half,
    Quarter,
    Eighth,
}

impl PreviewScale {
    pub fn divisor(self) -> u8 {
        match self {
            PreviewScale::Full => 1,
            PreviewScale::Half => 2,
            PreviewScale::Quarter => 4,
            PreviewScale::Eighth => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PreviewScale::Full => "Full",
            PreviewScale::Half => "Half",
            PreviewScale::Quarter => "Quarter",
            PreviewScale::Eighth => "Eighth",
        }
    }

    pub const ALL: [PreviewScale; 4] = [
        PreviewScale::Full,
        PreviewScale::Half,
        PreviewScale::Quarter,
        PreviewScale::Eighth,
    ];

    pub fn scale_dims(self, w: u32, h: u32) -> (u32, u32) {
        let d = self.divisor() as u32;
        ((w / d).max(1), (h / d).max(1))
    }
}

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
    cc_toner: CcToner,
    srgb_wrap: SrgbWrap,
    transform_pass: TransformPass,
    pool: TexturePool,
    cache: FrameCache,
}

impl Compositor {
    pub fn new(renderer: Renderer) -> Self {
        Self::with_cache_capacity(renderer, 64)
    }

    pub fn with_cache_capacity(renderer: Renderer, cache_entries: usize) -> Self {
        let gain = Gain::new(&renderer, COMPOSITOR_FORMAT);
        let cc_toner = CcToner::new(&renderer, COMPOSITOR_FORMAT);
        let srgb_wrap = SrgbWrap::new(&renderer, COMPOSITOR_FORMAT);
        let transform_pass = TransformPass::new(&renderer, COMPOSITOR_FORMAT);
        Self {
            renderer,
            gain,
            cc_toner,
            srgb_wrap,
            transform_pass,
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
    /// otherwise renders and inserts. Equivalent to
    /// `render_cached_at(_, PreviewScale::Full)`.
    pub fn render_cached(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        self.render_cached_at(project, comp_id, frame, PreviewScale::Full)
    }

    pub fn render_cached_at(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
        scale: PreviewScale,
    ) -> Result<wgpu::Texture, CompositorError> {
        let comp = project
            .composition(comp_id)
            .ok_or(CompositorError::UnknownComposition)?;
        let layer = comp
            .layers
            .iter()
            .find(|l| frame >= l.in_frame && frame < l.out_frame)
            .ok_or(CompositorError::NoVisibleLayer)?;
        let key = Self::cache_key(comp_id, frame, layer, scale);
        if let Some(tex) = self.cache.get(key) {
            return Ok(tex);
        }
        let tex = self.render_at(project, comp_id, frame, scale)?;
        self.cache.insert(key, tex.clone());
        Ok(tex)
    }

    fn cache_key(comp_id: CompId, frame: u32, layer: &Layer, scale: PreviewScale) -> CacheKey {
        let stack_hash = hash_effect_stack(layer.effects.iter());
        CacheKey::with_scale(comp_id.0, frame, stack_hash, scale.divisor())
    }

    /// Render the first layer of the named composition that's visible at
    /// `frame`, applying its effect stack. The output texture is owned by
    /// the caller (not pooled). Renders at full resolution.
    pub fn render(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        self.render_at(project, comp_id, frame, PreviewScale::Full)
    }

    pub fn render_at(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
        scale: PreviewScale,
    ) -> Result<wgpu::Texture, CompositorError> {
        let _span = info_span!(
            "compositor.render",
            frame,
            comp = comp_id.0,
            scale = scale.label()
        )
        .entered();

        let comp = project
            .composition(comp_id)
            .ok_or(CompositorError::UnknownComposition)?;
        let layer = comp
            .layers
            .iter()
            .find(|l| frame >= l.in_frame && frame < l.out_frame)
            .ok_or(CompositorError::NoVisibleLayer)?;

        let (rw, rh) = scale.scale_dims(comp.width, comp.height);

        let source_image = {
            let _s = debug_span!("compositor.resolve_source").entered();
            self.resolve_layer_source(project, &layer.kind, rw, rh)?
        };
        let mut current_tex = upload_image(&self.renderer, &source_image);

        for eff in &layer.effects {
            if !eff.enabled {
                continue;
            }
            current_tex = self.apply_effect(eff, current_tex, rw, rh)?;
        }

        // Apply the layer's transform onto the comp canvas. v1 treats the
        // post-effects texture as covering the comp at scale 1.0; F-040
        // generalizes to multi-layer compositing.
        let transformed = self.apply_transform(
            &layer.transform,
            current_tex,
            comp.background,
            scale,
            rw,
            rh,
            frame,
        );
        Ok(transformed)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_transform(
        &mut self,
        transform: &felx_core::model::Transform,
        input: wgpu::Texture,
        background: [f32; 4],
        preview_scale: PreviewScale,
        rw: u32,
        rh: u32,
        frame: u32,
    ) -> wgpu::Texture {
        let scale_div = preview_scale.divisor() as f32;
        let position = transform.position.sample_at(frame);
        let anchor = transform.anchor.sample_at(frame);
        let scale_v = transform.scale.sample_at(frame);
        let rotation_deg = transform.rotation.sample_at(frame);
        let opacity = transform.opacity.sample_at(frame);

        let identity = position == [0.0, 0.0]
            && anchor == [0.0, 0.0]
            && scale_v == [1.0, 1.0]
            && rotation_deg == 0.0
            && opacity == 1.0;
        if identity {
            return input;
        }

        let params = TransformParams::build(
            [position[0] / scale_div, position[1] / scale_div],
            [anchor[0] / scale_div, anchor[1] / scale_div],
            scale_v,
            rotation_deg,
            opacity,
            [rw as f32, rh as f32],
            [rw as f32, rh as f32],
            background,
        );

        let output = self.pool.acquire(&self.renderer, rw, rh, COMPOSITOR_FORMAT);
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.transform"),
                });
        self.transform_pass
            .render(&self.renderer, &mut cmd, &in_view, &out_view, params);
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(input);
        output
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
            "cc_toner" => self.apply_cc_toner(eff, input, w, h),
            other => {
                warn!(effect_id = other, "skipping unknown effect");
                Ok(input)
            }
        }
    }

    fn apply_cc_toner(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let mode = eff
            .values
            .enum_str("tones")
            .and_then(TonesMode::from_id)
            .unwrap_or(TonesMode::Tritone);
        let highlights = eff
            .values
            .color("highlights")
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        let brights = eff
            .values
            .color("brights")
            .unwrap_or([0.75, 0.75, 0.75, 1.0]);
        let midtones = eff.values.color("midtones").unwrap_or([0.5, 0.5, 0.5, 1.0]);
        let darktones = eff
            .values
            .color("darktones")
            .unwrap_or([0.25, 0.25, 0.25, 1.0]);
        let shadows = eff.values.color("shadows").unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let blend = eff.values.float("blend").unwrap_or(0.0);
        let params = CcTonerParams::pack(
            mode, highlights, brights, midtones, darktones, shadows, blend,
        );

        let encoded = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let toned = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let decoded = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);

        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let enc_view = encoded.create_view(&wgpu::TextureViewDescriptor::default());
        let toned_view = toned.create_view(&wgpu::TextureViewDescriptor::default());
        let dec_view = decoded.create_view(&wgpu::TextureViewDescriptor::default());

        // 1) Encode linear → sRGB-encoded (the wrap pipeline submits its own
        //    encoder).
        self.srgb_wrap.encode(&self.renderer, &in_view, &enc_view);

        // 2) Run CC Toner on the encoded texture.
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.cc_toner"),
                });
        self.cc_toner
            .render(&self.renderer, &mut cmd, &enc_view, &toned_view, params);
        self.renderer.queue().submit(Some(cmd.finish()));

        // 3) Decode sRGB-encoded → linear back.
        self.srgb_wrap
            .decode(&self.renderer, &toned_view, &dec_view);

        self.pool.release(input);
        self.pool.release(encoded);
        self.pool.release(toned);
        Ok(decoded)
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
                let mut rgba = img.to_rgba8();
                if rgba.width() != comp_w || rgba.height() != comp_h {
                    rgba = imageops::resize(&rgba, comp_w, comp_h, imageops::FilterType::Triangle);
                }
                Ok(rgba)
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
