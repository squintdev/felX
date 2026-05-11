//! Single-layer compositor (M0). M3's F-040 generalizes this to arbitrary
//! layer stacks with blending modes and track mattes; here we render the
//! first visible layer's source through its effect stack.

use crate::blend_pass::{BlendParams, BlendPass};
use crate::clear_pass::clear_to;
use crate::cpu_pass::run_cpu_pass;
use crate::effect_state::{EffectStateRegistry, StateKey};
use crate::effects::bloom::{Bloom, BlurParams, CompositeParams, ThresholdParams};
use crate::effects::cc_toner::{CcToner, CcTonerParams, TonesMode};
use crate::effects::crt::{self, Crt, CrtParams};
use crate::effects::crt_persistence::{CrtPersistence, CrtPersistenceParams};
use crate::effects::gain::{Gain, GainParams};
use crate::effects::invert::invert_in_place;
use crate::effects::squint_diffusion::{self, DiffusionParams};
// Legacy Signal-Lite (WGSL approximation) — superseded by F-071a's
// ntsc-rs CPU pass in `felx_media::signal_ntsc`. The pipeline file
// (`crates/felx-render/src/effects/signal.rs`) stays around as
// reference but is no longer wired into the compositor.
use crate::effects::vhs::{Vhs, VhsParams};
use crate::frame_cache::{CacheKey, FrameCache, hash_effect_stack};
use crate::mask_pass::{MaskApply, rasterize_masks};
use crate::matte_pass::{MatteParams, MattePass};
use crate::srgb_wrap::SrgbWrap;
use crate::texture_io::{COMPOSITOR_FORMAT, upload_image};
use crate::transform_pass::{TransformParams, TransformPass};
use crate::{Renderer, RendererError};
use felx_core::model::{CompId, Effect, Frame, Framerate, Layer, LayerId, LayerKind, Project};
use felx_media::{FfmpegDecoder, HwaccelKind, VideoDecoder};
use image::{ImageBuffer, Rgba, RgbaImage, imageops};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
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

/// Hard cap on how deep pre-comp nesting can go before we bail out.
/// Eight feels like more than anyone realistically needs and keeps the
/// recursive call stack bounded.
const MAX_PRECOMP_DEPTH: usize = 8;

#[derive(Debug)]
pub enum CompositorError {
    UnknownComposition,
    NoVisibleLayer,
    UnknownAsset,
    UnsupportedLayerKind(&'static str),
    AssetIo(std::io::Error),
    AssetDecode(image::ImageError),
    RendererInit(RendererError),
    /// Pre-comp graph contains a cycle (A nests B nests A …) or exceeds
    /// [`MAX_PRECOMP_DEPTH`].
    PrecompCycle(u32),
    VideoDecode(felx_media::DecodeError),
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
            CompositorError::PrecompCycle(c) => {
                write!(f, "pre-comp cycle or depth-limit hit at comp {c}")
            }
            CompositorError::VideoDecode(e) => write!(f, "video decode: {e}"),
        }
    }
}

/// Fit `img` into an `out_w` × `out_h` canvas preserving the source's
/// aspect ratio. Letterboxes / pillarboxes with transparent borders so
/// imported images and video frames don't get stretched.
fn fit_into_canvas(img: RgbaImage, out_w: u32, out_h: u32) -> RgbaImage {
    if img.width() == out_w && img.height() == out_h {
        return img;
    }
    let (src_w, src_h) = img.dimensions();
    if src_w == 0 || src_h == 0 || out_w == 0 || out_h == 0 {
        return ImageBuffer::from_pixel(out_w.max(1), out_h.max(1), Rgba([0, 0, 0, 0]));
    }
    let scale = (out_w as f32 / src_w as f32).min(out_h as f32 / src_h as f32);
    let new_w = ((src_w as f32 * scale).round() as u32).clamp(1, out_w);
    let new_h = ((src_h as f32 * scale).round() as u32).clamp(1, out_h);
    let resized = if new_w == src_w && new_h == src_h {
        img
    } else {
        imageops::resize(&img, new_w, new_h, imageops::FilterType::Triangle)
    };
    let mut canvas: RgbaImage = ImageBuffer::from_pixel(out_w, out_h, Rgba([0, 0, 0, 0]));
    let off_x = (out_w.saturating_sub(new_w)) / 2;
    let off_y = (out_h.saturating_sub(new_h)) / 2;
    imageops::overlay(&mut canvas, &resized, off_x as i64, off_y as i64);
    canvas
}

fn ffmpeg_error_invalid_data() -> ffmpeg_next::Error {
    ffmpeg_next::Error::InvalidData
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
    crt: Crt,
    crt_persistence: CrtPersistence,
    bloom: Bloom,
    vhs: Vhs,
    srgb_wrap: SrgbWrap,
    transform_pass: TransformPass,
    blend_pass: BlendPass,
    matte_pass: MattePass,
    mask_apply: MaskApply,
    pool: TexturePool,
    cache: FrameCache,
    /// Per-effect-instance frame-to-frame state (F-070). Holds ping-pong
    /// textures for stateful effects like CRT phosphor persistence.
    state: EffectStateRegistry,
    /// Per-(asset path, layer id) video decoder cache. Each Video layer
    /// instance owns its own decoder so concurrent layers don't trample
    /// each other's seek state. Monotonic playback hits the fast path
    /// (decode_next); seeks happen only on scrub or out-of-order frames.
    video_cache: HashMap<(PathBuf, LayerId), VideoDecoderEntry>,
}

struct VideoDecoderEntry {
    decoder: FfmpegDecoder,
    fps: f64,
    /// Last source-frame index successfully emitted, if any.
    last_frame: Option<u32>,
    /// Last decoded image, kept around so a repeat-frame request (paused
    /// playhead, multiple effects asking for the same source frame) is
    /// free.
    last_image: Option<RgbaImage>,
}

impl Compositor {
    pub fn new(renderer: Renderer) -> Self {
        Self::with_cache_capacity(renderer, 64)
    }

    pub fn with_cache_capacity(renderer: Renderer, cache_entries: usize) -> Self {
        let gain = Gain::new(&renderer, COMPOSITOR_FORMAT);
        let cc_toner = CcToner::new(&renderer, COMPOSITOR_FORMAT);
        let crt = Crt::new(&renderer, COMPOSITOR_FORMAT);
        let crt_persistence = CrtPersistence::new(&renderer, COMPOSITOR_FORMAT);
        let bloom = Bloom::new(&renderer, COMPOSITOR_FORMAT);
        let vhs = Vhs::new(&renderer, COMPOSITOR_FORMAT);
        let srgb_wrap = SrgbWrap::new(&renderer, COMPOSITOR_FORMAT);
        let transform_pass = TransformPass::new(&renderer, COMPOSITOR_FORMAT);
        let blend_pass = BlendPass::new(&renderer, COMPOSITOR_FORMAT);
        let matte_pass = MattePass::new(&renderer, COMPOSITOR_FORMAT);
        let mask_apply = MaskApply::new(&renderer, COMPOSITOR_FORMAT);
        Self {
            renderer,
            gain,
            cc_toner,
            crt,
            crt_persistence,
            bloom,
            vhs,
            srgb_wrap,
            transform_pass,
            blend_pass,
            matte_pass,
            mask_apply,
            pool: TexturePool::new("compositor-pool"),
            cache: FrameCache::new(cache_entries),
            state: EffectStateRegistry::new(),
            video_cache: HashMap::new(),
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
        let visible: Vec<&Layer> = comp
            .layers
            .iter()
            .filter(|l| frame >= l.in_frame && frame < l.out_frame)
            .collect();
        if visible.is_empty() {
            return Err(CompositorError::NoVisibleLayer);
        }
        let key = Self::cache_key_multilayer(comp_id, frame, &visible, scale);
        if let Some(tex) = self.cache.get(key) {
            return Ok(tex);
        }
        let tex = self.render_at(project, comp_id, frame, scale)?;
        self.cache.insert(key, tex.clone());
        Ok(tex)
    }

    fn cache_key_multilayer(
        comp_id: CompId,
        frame: u32,
        layers: &[&Layer],
        scale: PreviewScale,
    ) -> CacheKey {
        // Hash every visible layer's effect stack into a single key.
        let stack_hash = hash_effect_stack(layers.iter().flat_map(|l| l.effects.iter()));
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
        let mut visiting: std::collections::HashSet<u32> = std::collections::HashSet::new();
        self.render_at_inner(project, comp_id, frame, scale, &mut visiting)
    }

    fn render_at_inner(
        &mut self,
        project: &Project,
        comp_id: CompId,
        frame: u32,
        scale: PreviewScale,
        visiting: &mut std::collections::HashSet<u32>,
    ) -> Result<wgpu::Texture, CompositorError> {
        if !visiting.insert(comp_id.0) || visiting.len() > MAX_PRECOMP_DEPTH {
            return Err(CompositorError::PrecompCycle(comp_id.0));
        }

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

        let visible: Vec<&Layer> = comp
            .layers
            .iter()
            .filter(|l| frame >= l.in_frame && frame < l.out_frame)
            .collect();
        if visible.is_empty() {
            return Err(CompositorError::NoVisibleLayer);
        }

        // Compute the indices into `visible` that are matte sources for the
        // layer immediately *below* them. Those layers contribute their
        // gating but are not blended onto the accumulator on their own.
        let matte_source_indices: std::collections::HashSet<usize> = visible
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.track_matte.is_some() && i + 1 < visible.len() {
                    Some(i + 1)
                } else {
                    None
                }
            })
            .collect();

        let (rw, rh) = scale.scale_dims(comp.width, comp.height);

        // Initialize accumulator with the comp's background color via a
        // clear-to-color pass on a pool-acquired texture so subsequent
        // frames at the same dims reuse the underlying allocation.
        let mut accumulator = self.pool.acquire(&self.renderer, rw, rh, COMPOSITOR_FORMAT);
        clear_to(
            &self.renderer,
            &accumulator.create_view(&wgpu::TextureViewDescriptor::default()),
            comp.background,
        );

        let framerate = comp.framerate;
        let time = felx_core::model::Frame(frame).to_time(framerate);
        for (idx, layer) in visible.iter().enumerate() {
            if matte_source_indices.contains(&idx) {
                continue;
            }

            // Adjustment layer (F-060): apply its effect stack to the
            // accumulator (the flattened layers below) and replace the
            // accumulator with the result. No per-layer source pixels of
            // its own, no blend onto the accumulator beyond the effect's
            // pass-through. Track-matte interaction with adjustment layers
            // is unusual and explicitly out of scope for v1.
            if matches!(layer.kind, LayerKind::Adjustment) {
                if !layer.effects.is_empty() {
                    accumulator =
                        self.apply_effect_stack(layer, accumulator, rw, rh, time, frame)?;
                }
                continue;
            }

            let mut layer_tex = self.render_layer(
                project,
                layer,
                comp.background,
                scale,
                rw,
                rh,
                frame,
                framerate,
                visiting,
            )?;

            if let Some(mode) = layer.track_matte
                && idx + 1 < visible.len()
            {
                let source_layer = visible[idx + 1];
                let source_tex = self.render_layer(
                    project,
                    source_layer,
                    comp.background,
                    scale,
                    rw,
                    rh,
                    frame,
                    framerate,
                    visiting,
                )?;
                layer_tex = self.apply_matte(layer_tex, source_tex, rw, rh, mode.shader_index());
            }

            let blend_mode = layer.blend_mode.shader_index();
            accumulator = self.blend_layer_onto(accumulator, layer_tex, rw, rh, 1.0, blend_mode);
        }

        visiting.remove(&comp_id.0);
        Ok(accumulator)
    }

    fn apply_matte(
        &mut self,
        target: wgpu::Texture,
        source: wgpu::Texture,
        rw: u32,
        rh: u32,
        mode: u32,
    ) -> wgpu::Texture {
        let output = self.pool.acquire(&self.renderer, rw, rh, COMPOSITOR_FORMAT);
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.matte"),
                });
        self.matte_pass.render(
            &self.renderer,
            &mut cmd,
            &target_view,
            &source_view,
            &out_view,
            MatteParams::new(mode),
        );
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(target);
        self.pool.release(source);
        output
    }

    #[allow(clippy::too_many_arguments)]
    fn render_layer(
        &mut self,
        project: &Project,
        layer: &Layer,
        comp_background: [f32; 4],
        scale: PreviewScale,
        rw: u32,
        rh: u32,
        frame: u32,
        framerate: Framerate,
        visiting: &mut std::collections::HashSet<u32>,
    ) -> Result<wgpu::Texture, CompositorError> {
        let mut current_tex = if let LayerKind::Composition { comp: inner_id } = &layer.kind {
            // Pre-comp: render the inner comp recursively, applying the
            // outer layer's time remap (offset + scale) to derive the inner
            // comp's playhead. Then resize its output to the outer's render
            // dims via a CPU readback. The GPU-direct blit-resize is a
            // follow-up; this keeps the v1 path simple and correctness-
            // focused.
            let _s = debug_span!("compositor.precomp", inner = inner_id.0).entered();
            let source_frame = layer.source_frame_for(frame);
            // Clamp source_frame to inner's last valid frame so out-of-range
            // remap does not blow up the visibility filter inside the inner
            // render.
            let inner = project
                .composition(*inner_id)
                .ok_or(CompositorError::UnknownComposition)?;
            let max_frame = inner.duration_frames.saturating_sub(1);
            let bounded = source_frame.min(max_frame);
            let inner_tex = self.render_at_inner(project, *inner_id, bounded, scale, visiting)?;
            self.resize_to(inner_tex, rw, rh)
        } else if matches!(layer.kind, LayerKind::Video { .. }) {
            // Video: per-(asset, layer) decoder cache, monotonic-playback
            // fast path, seek-and-walk on out-of-order frames, resize to
            // comp dims.
            let _s = debug_span!("compositor.video", layer = layer.id.0).entered();
            let source_frame = layer.source_frame_for(frame);
            let img = self.resolve_video_frame(project, layer, source_frame, rw, rh)?;
            upload_image(&self.renderer, &img)
        } else {
            let source_image = {
                let _s = debug_span!("compositor.resolve_source", layer = layer.id.0).entered();
                self.resolve_layer_source(project, &layer.kind, rw, rh)?
            };
            upload_image(&self.renderer, &source_image)
        };

        let time = Frame(frame).to_time(framerate);
        for (effect_index, eff) in layer.effects.iter().enumerate() {
            if !eff.enabled {
                continue;
            }
            // Resolve animated parameters at the playhead time once. Effect
            // dispatch reads from the resolved view so effect-specific code
            // never has to know whether a parameter was animated.
            let resolved = Effect {
                id: eff.id.clone(),
                enabled: eff.enabled,
                values: eff.values.resolved_at(time),
            };
            current_tex = self.apply_effect_at(
                &resolved,
                current_tex,
                rw,
                rh,
                layer.id.0,
                effect_index,
                frame,
            )?;
        }

        // Mask the layer's alpha (F-061…F-066).
        if !layer.masks.is_empty() {
            current_tex = self.apply_masks(&layer.masks, current_tex, rw, rh, time);
        }

        // Background for the per-layer transform pass is transparent so the
        // accumulator behind it shows through. The compositor's BG color
        // only fills the initial accumulator above.
        let transparent: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
        let _ = comp_background;
        let transformed = self.apply_transform(
            &layer.transform,
            current_tex,
            transparent,
            scale,
            rw,
            rh,
            frame,
        );
        Ok(transformed)
    }

    #[allow(clippy::too_many_arguments)]
    fn blend_layer_onto(
        &mut self,
        accumulator: wgpu::Texture,
        layer: wgpu::Texture,
        rw: u32,
        rh: u32,
        opacity: f32,
        mode: u32,
    ) -> wgpu::Texture {
        let output = self.pool.acquire(&self.renderer, rw, rh, COMPOSITOR_FORMAT);
        let acc_view = accumulator.create_view(&wgpu::TextureViewDescriptor::default());
        let layer_view = layer.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.blend"),
                });
        self.blend_pass.render(
            &self.renderer,
            &mut cmd,
            &acc_view,
            &layer_view,
            &out_view,
            BlendParams::with_mode(mode, opacity),
        );
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(accumulator);
        self.pool.release(layer);
        output
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

    #[allow(clippy::too_many_arguments)]
    fn apply_effect_at(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
        layer_id: u32,
        effect_index: usize,
        current_frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let _span = debug_span!("compositor.effect", id = %eff.id).entered();
        if eff.id == "bloom" {
            return Ok(self.apply_bloom(eff, input, w, h));
        }
        if eff.id == "crt_persistence" {
            return Ok(self.apply_crt_persistence(
                eff,
                input,
                w,
                h,
                layer_id,
                effect_index,
                current_frame,
            ));
        }

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
            "signal" => {
                // F-071a: real ntsc-rs algorithm via CPU pass. The legacy
                // WGSL Signal-Lite path stays in the codebase (effects/
                // signal/effect.wgsl + apply_signal below) but is no
                // longer the dispatch target — leaving it there means
                // hot-reload still has somewhere to write.
                let values = eff.values.clone();
                let frame = current_frame;
                let output = run_cpu_pass(&self.renderer, &input, "signal_ntsc", move |img| {
                    felx_media::signal_ntsc::apply_signal(img, &values, frame);
                });
                self.pool.release(input);
                Ok(output)
            }
            "squint_diffusion" => {
                let params = build_diffusion_params(eff);
                let output = run_cpu_pass(&self.renderer, &input, "squint_diffusion", |img| {
                    squint_diffusion::diffuse_in_place(img, &params);
                });
                self.pool.release(input);
                Ok(output)
            }
            "crt" => self.apply_crt(eff, input, w, h),
            "vhs" => self.apply_vhs(eff, input, w, h),
            other => {
                warn!(effect_id = other, "skipping unknown effect");
                Ok(input)
            }
        }
    }

    fn apply_crt(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let curvature_x = eff.values.float("curvature_x").unwrap_or(0.06);
        let curvature_y = eff.values.float("curvature_y").unwrap_or(0.08);
        let scanline_intensity = eff.values.float("scanline_intensity").unwrap_or(0.4);
        let scanline_thickness = eff.values.float("scanline_thickness").unwrap_or(0.5);
        let mask_intensity = eff.values.float("mask_intensity").unwrap_or(0.5);
        let mask_size = eff.values.float("mask_size").unwrap_or(3.0);
        let mask_id = eff
            .values
            .enum_str("mask_type")
            .unwrap_or("aperture_grille");
        let convergence_radial = eff.values.float("convergence_radial").unwrap_or(1.5);
        let vignette_intensity = eff.values.float("vignette_intensity").unwrap_or(0.4);
        let vignette_softness = eff.values.float("vignette_softness").unwrap_or(0.4);

        let params = CrtParams::new(
            [curvature_x, curvature_y],
            scanline_intensity,
            scanline_thickness,
            mask_intensity,
            mask_size,
            crt::mask_index(mask_id),
            convergence_radial,
            vignette_intensity,
            vignette_softness,
            [w as f32, h as f32],
        );

        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.crt"),
                });
        self.crt
            .render(&self.renderer, &mut cmd, &in_view, &out_view, params);
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(input);
        Ok(output)
    }

    fn apply_vhs(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let params = VhsParams::new(
            eff.values.float("sync_wobble").unwrap_or(1.0),
            eff.values.float("dropouts_density").unwrap_or(0.2),
            eff.values.float("dropouts_polarity").unwrap_or(1.0),
            eff.values.float("tape_damage").unwrap_or(0.2),
            eff.values.float("transport_brightness").unwrap_or(0.2),
            eff.values.float("transport_chroma_phase").unwrap_or(0.1),
            eff.values.float("transport_freq").unwrap_or(0.5),
            eff.values.float("vertical_scroll").unwrap_or(0.0),
            eff.values.int("seed").unwrap_or(0) as f32,
            eff.values.float("time_seconds").unwrap_or(0.0),
            [w as f32, h as f32],
        );

        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.vhs"),
                });
        self.vhs
            .render(&self.renderer, &mut cmd, &in_view, &out_view, params);
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(input);
        Ok(output)
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

    fn apply_bloom(&mut self, eff: &Effect, input: wgpu::Texture, w: u32, h: u32) -> wgpu::Texture {
        let threshold = eff.values.float("threshold").unwrap_or(0.7);
        let intensity = eff.values.float("intensity").unwrap_or(0.6);
        let radius = eff.values.float("radius").unwrap_or(4.0);
        let soft_knee = eff.values.float("soft_knee").unwrap_or(0.3);

        // 1) Threshold pass into a fresh pool texture.
        let bright = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        // 2) Horizontal blur into a second.
        let h_blur = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        // 3) Vertical blur into a third.
        let v_blur = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        // 4) Composite with the original into a final pool texture.
        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);

        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let bright_view = bright.create_view(&wgpu::TextureViewDescriptor::default());
        let h_blur_view = h_blur.create_view(&wgpu::TextureViewDescriptor::default());
        let v_blur_view = v_blur.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.bloom"),
                });
        self.bloom.render_threshold(
            &self.renderer,
            &mut cmd,
            &in_view,
            &bright_view,
            ThresholdParams {
                threshold,
                soft_knee,
                _pad0: 0.0,
                _pad1: 0.0,
            },
        );
        self.bloom.render_blur(
            &self.renderer,
            &mut cmd,
            &bright_view,
            &h_blur_view,
            BlurParams {
                direction_x: 1.0,
                direction_y: 0.0,
                radius,
                _pad: 0.0,
            },
        );
        self.bloom.render_blur(
            &self.renderer,
            &mut cmd,
            &h_blur_view,
            &v_blur_view,
            BlurParams {
                direction_x: 0.0,
                direction_y: 1.0,
                radius,
                _pad: 0.0,
            },
        );
        self.bloom.render_composite(
            &self.renderer,
            &mut cmd,
            &in_view,
            &v_blur_view,
            &out_view,
            CompositeParams {
                intensity,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            },
        );
        self.renderer.queue().submit(Some(cmd.finish()));

        self.pool.release(input);
        self.pool.release(bright);
        self.pool.release(h_blur);
        self.pool.release(v_blur);
        output
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_crt_persistence(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
        layer_id: u32,
        effect_index: usize,
        current_frame: u32,
    ) -> wgpu::Texture {
        let decay = eff.values.float("decay").unwrap_or(0.85);
        let tint_r = eff.values.float("tint_r").unwrap_or(1.0);
        let tint_g = eff.values.float("tint_g").unwrap_or(1.0);
        let tint_b = eff.values.float("tint_b").unwrap_or(1.0);
        let key = StateKey::new(layer_id, effect_index, "crt_persistence");
        // Acquire ping-pong state: read = previous output, write = new output.
        // Compute the views first so we don't hold the registry borrow
        // across the queue submit.
        let (prev_view, write_view, write_tex) = {
            let acq =
                self.state
                    .acquire(&self.renderer, key, w, h, COMPOSITOR_FORMAT, current_frame);
            (
                acq.read
                    .create_view(&wgpu::TextureViewDescriptor::default()),
                acq.write
                    .create_view(&wgpu::TextureViewDescriptor::default()),
                acq.write.clone(),
            )
        };
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        // Pool-owned output. Downstream passes (transform, blend) recycle
        // their input back into the pool, so handing them the state's
        // write_tex would corrupt the next frame's read. Render into the
        // state texture, then copy it into a fresh pool texture for the
        // rest of the pipeline to consume.
        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.crt_persistence"),
                });
        self.crt_persistence.render(
            &self.renderer,
            &mut cmd,
            &in_view,
            &prev_view,
            &write_view,
            CrtPersistenceParams {
                decay,
                tint_r,
                tint_g,
                tint_b,
            },
        );
        cmd.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &write_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &output,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(input);
        output
    }

    fn apply_masks(
        &mut self,
        masks: &[felx_core::model::Mask],
        input: wgpu::Texture,
        w: u32,
        h: u32,
        time: felx_core::model::Rational,
    ) -> wgpu::Texture {
        let _s = debug_span!("compositor.masks", count = masks.len()).entered();
        let mask_img = rasterize_masks(masks, w, h, time);
        let mask_tex = upload_image(&self.renderer, &mask_img);
        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_view = mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.masks"),
                });
        self.mask_apply
            .apply(&self.renderer, &mut cmd, &in_view, &mask_view, &out_view);
        self.renderer.queue().submit(Some(cmd.finish()));
        self.pool.release(input);
        output
    }

    /// Apply a layer's effect stack to an arbitrary input texture and
    /// return the resulting texture. Used by adjustment layers (F-060) so
    /// the same effect dispatch path runs against the comp accumulator.
    fn apply_effect_stack(
        &mut self,
        layer: &Layer,
        input: wgpu::Texture,
        w: u32,
        h: u32,
        time: felx_core::model::Rational,
        current_frame: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let mut current = input;
        for (effect_index, eff) in layer.effects.iter().enumerate() {
            if !eff.enabled {
                continue;
            }
            let resolved = Effect {
                id: eff.id.clone(),
                enabled: eff.enabled,
                values: eff.values.resolved_at(time),
            };
            current = self.apply_effect_at(
                &resolved,
                current,
                w,
                h,
                layer.id.0,
                effect_index,
                current_frame,
            )?;
        }
        Ok(current)
    }

    /// Resolve one source frame from a Video layer's referenced asset.
    ///
    /// Strategy: cache one [`FfmpegDecoder`] per (asset path, layer id) so
    /// concurrent Video layers don't trample each other's seek state.
    /// Monotonic playback (request frame N, then N+1, …) hits the fast
    /// path of just calling `next_frame`. Out-of-order or large jumps
    /// re-seek and walk forward to the requested frame. Repeat-frame
    /// requests (paused playhead, multiple effects on the same layer) are
    /// served from a one-frame image cache so we don't re-decode.
    fn resolve_video_frame(
        &mut self,
        project: &Project,
        layer: &Layer,
        source_frame: u32,
        comp_w: u32,
        comp_h: u32,
    ) -> Result<RgbaImage, CompositorError> {
        let asset_id = match layer.kind {
            LayerKind::Video { asset } => asset,
            _ => return Err(CompositorError::UnsupportedLayerKind("Video")),
        };
        let asset = project
            .asset(asset_id)
            .ok_or(CompositorError::UnknownAsset)?;
        let key = (asset.path.clone(), layer.id);

        // Open on first use.
        if !self.video_cache.contains_key(&key) {
            let decoder = FfmpegDecoder::open(&asset.path, HwaccelKind::Auto)
                .map_err(CompositorError::VideoDecode)?;
            let fps = decoder.fps().max(1.0);
            self.video_cache.insert(
                key.clone(),
                VideoDecoderEntry {
                    decoder,
                    fps,
                    last_frame: None,
                    last_image: None,
                },
            );
        }
        let entry = self.video_cache.get_mut(&key).expect("just inserted");

        // Repeat-frame fast path.
        if entry.last_frame == Some(source_frame)
            && let Some(img) = &entry.last_image
        {
            return Ok(fit_into_canvas(img.clone(), comp_w, comp_h));
        }

        // Decide whether to seek. Seek if there is no last frame, the
        // request goes backward, or it skips forward by more than one.
        let needs_seek = match entry.last_frame {
            None => true,
            Some(prev) => source_frame < prev || source_frame.saturating_sub(prev) > 1,
        };
        if needs_seek {
            let target = Duration::from_secs_f64(source_frame as f64 / entry.fps);
            entry
                .decoder
                .seek(target)
                .map_err(CompositorError::VideoDecode)?;
            entry.last_frame = None;
            entry.last_image = None;
        }

        // Walk forward to the requested frame.
        let mut latest: Option<RgbaImage> = None;
        loop {
            let dec = entry
                .decoder
                .next_frame()
                .map_err(CompositorError::VideoDecode)?;
            let Some(frame) = dec else {
                // EOF before reaching the requested frame — return the
                // most-recent decoded frame, or a transparent buffer if
                // we never got one. Honest fallback for over-length comps.
                let img = latest
                    .unwrap_or_else(|| ImageBuffer::from_pixel(comp_w, comp_h, Rgba([0, 0, 0, 0])));
                entry.last_image = Some(img.clone());
                return Ok(fit_into_canvas(img, comp_w, comp_h));
            };
            let frame_idx = (frame.pts.as_secs_f64() * entry.fps).round().max(0.0) as u32;
            let img = ImageBuffer::from_raw(frame.width, frame.height, frame.rgba).ok_or(
                CompositorError::VideoDecode(felx_media::DecodeError::Ffmpeg(
                    ffmpeg_error_invalid_data(),
                )),
            )?;
            latest = Some(img);
            if frame_idx >= source_frame {
                let img = latest.expect("just assigned");
                entry.last_frame = Some(frame_idx);
                entry.last_image = Some(img.clone());
                return Ok(fit_into_canvas(img, comp_w, comp_h));
            }
        }
    }

    /// CPU-readback resize. Used by the pre-comp path to fit the inner
    /// comp's render output into the outer's render dims. A GPU-direct
    /// blit-resize is a perf follow-up; functional correctness first.
    fn resize_to(&mut self, input: wgpu::Texture, w: u32, h: u32) -> wgpu::Texture {
        if input.width() == w && input.height() == h {
            return input;
        }
        let img = crate::texture_io::download_image(&self.renderer, &input);
        let resized = imageops::resize(&img, w, h, imageops::FilterType::Triangle);
        upload_image(&self.renderer, &resized)
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
                let rgba = img.to_rgba8();
                Ok(fit_into_canvas(rgba, comp_w, comp_h))
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

fn build_diffusion_params(eff: &Effect) -> DiffusionParams {
    let error_weight = eff.values.float("error_weight").unwrap_or(0.75);
    let alpha = eff.values.float("alpha").unwrap_or(1.0);
    let n = eff.values.int("num_colors").unwrap_or(4).clamp(2, 6) as usize;
    let mut palette = Vec::with_capacity(n);
    for i in 1..=6 {
        if let Some(c) = eff.values.color(&format!("color_{i}")) {
            palette.push(c);
        }
    }
    palette.truncate(n);
    if palette.len() < 2 {
        palette = vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]];
    }
    DiffusionParams::new(error_weight, alpha, palette)
}

#[cfg(test)]
mod tests {
    use super::fit_into_canvas;
    use image::{ImageBuffer, Rgba};

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> image::RgbaImage {
        ImageBuffer::from_pixel(w, h, Rgba(rgba))
    }

    #[test]
    fn fit_preserves_landscape_aspect_with_pillarbox() {
        // 16:9 source into 16:9 comp — no borders.
        let out = fit_into_canvas(solid(160, 90, [255, 0, 0, 255]), 320, 180);
        assert_eq!(out.dimensions(), (320, 180));
        // Center should be source color.
        assert_eq!(out.get_pixel(160, 90)[0], 255);
        // Corners should be source color too (full coverage at matching aspect).
        assert_eq!(out.get_pixel(1, 1)[0], 255);
    }

    #[test]
    fn fit_landscape_source_in_square_comp_pillarboxes_top_bottom() {
        // 16:9 source into 1:1 comp — should letterbox top + bottom transparent.
        let out = fit_into_canvas(solid(160, 90, [0, 255, 0, 255]), 200, 200);
        assert_eq!(out.dimensions(), (200, 200));
        // Top edge should be transparent (above the fitted image).
        assert_eq!(out.get_pixel(100, 0)[3], 0);
        // Vertical center should be the fitted source.
        assert_eq!(out.get_pixel(100, 100)[1], 255);
        assert_eq!(out.get_pixel(100, 100)[3], 255);
    }

    #[test]
    fn fit_portrait_source_in_landscape_comp_pillarboxes_sides() {
        // 9:16 source into 16:9 comp — sides transparent.
        let out = fit_into_canvas(solid(90, 160, [0, 0, 255, 255]), 320, 180);
        assert_eq!(out.dimensions(), (320, 180));
        // Left edge should be transparent.
        assert_eq!(out.get_pixel(0, 90)[3], 0);
        // Center should be source.
        assert_eq!(out.get_pixel(160, 90)[2], 255);
    }

    #[test]
    fn fit_matching_dims_returns_unchanged() {
        let src = solid(64, 48, [10, 20, 30, 255]);
        let out = fit_into_canvas(src.clone(), 64, 48);
        assert_eq!(out.as_raw(), src.as_raw());
    }
}
