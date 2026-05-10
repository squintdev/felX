//! Single-layer compositor (M0). M3's F-040 generalizes this to arbitrary
//! layer stacks with blending modes and track mattes; here we render the
//! first visible layer's source through its effect stack.

use crate::blend_pass::{BlendParams, BlendPass};
use crate::clear_pass::clear_to;
use crate::cpu_pass::run_cpu_pass;
use crate::effects::cc_toner::{CcToner, CcTonerParams, TonesMode};
use crate::effects::crt::{self, Crt, CrtParams};
use crate::effects::gain::{Gain, GainParams};
use crate::effects::invert::invert_in_place;
use crate::effects::squint_diffusion::{self, DiffusionParams};
use crate::effects::signal::{Signal, SignalParams};
use crate::effects::vhs::{Vhs, VhsParams};
use crate::frame_cache::{CacheKey, FrameCache, hash_effect_stack};
use crate::mask_pass::{MaskApply, rasterize_masks};
use crate::matte_pass::{MatteParams, MattePass};
use crate::srgb_wrap::SrgbWrap;
use crate::texture_io::{COMPOSITOR_FORMAT, upload_image};
use crate::transform_pass::{TransformParams, TransformPass};
use crate::{Renderer, RendererError};
use felx_core::model::{CompId, Effect, Frame, Framerate, Layer, LayerKind, Project};
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
    signal: Signal,
    crt: Crt,
    vhs: Vhs,
    srgb_wrap: SrgbWrap,
    transform_pass: TransformPass,
    blend_pass: BlendPass,
    matte_pass: MattePass,
    mask_apply: MaskApply,
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
        let signal = Signal::new(&renderer, COMPOSITOR_FORMAT);
        let crt = Crt::new(&renderer, COMPOSITOR_FORMAT);
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
            signal,
            crt,
            vhs,
            srgb_wrap,
            transform_pass,
            blend_pass,
            matte_pass,
            mask_apply,
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
                    accumulator = self.apply_effect_stack(layer, accumulator, rw, rh, time)?;
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
        } else {
            let source_image = {
                let _s = debug_span!("compositor.resolve_source", layer = layer.id.0).entered();
                self.resolve_layer_source(project, &layer.kind, rw, rh)?
            };
            upload_image(&self.renderer, &source_image)
        };

        let time = Frame(frame).to_time(framerate);
        for eff in &layer.effects {
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
            current_tex = self.apply_effect(&resolved, current_tex, rw, rh)?;
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
            "signal" => self.apply_signal(eff, input, w, h),
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

    fn apply_signal(
        &mut self,
        eff: &Effect,
        input: wgpu::Texture,
        w: u32,
        h: u32,
    ) -> Result<wgpu::Texture, CompositorError> {
        let chroma_blur = eff.values.float("chroma_blur").unwrap_or(0.4);
        let ringing = eff.values.float("ringing_intensity").unwrap_or(0.5);
        let snow = eff.values.float("snow_intensity").unwrap_or(0.0);
        let composite_noise = eff.values.float("composite_noise").unwrap_or(0.1);
        let head_h = eff.values.float("head_switch_height").unwrap_or(8.0);
        let head_shift = eff.values.float("head_switch_shift").unwrap_or(4.0);
        let seed = eff.values.int("seed").unwrap_or(0) as f32;
        let params = SignalParams::new(
            chroma_blur,
            ringing,
            snow,
            composite_noise,
            head_h,
            head_shift,
            seed,
            [w as f32, h as f32],
        );

        let output = self.pool.acquire(&self.renderer, w, h, COMPOSITOR_FORMAT);
        let in_view = input.create_view(&wgpu::TextureViewDescriptor::default());
        let out_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        let mut cmd =
            self.renderer
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("compositor.signal"),
                });
        self.signal
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
    ) -> Result<wgpu::Texture, CompositorError> {
        let mut current = input;
        for eff in &layer.effects {
            if !eff.enabled {
                continue;
            }
            let resolved = Effect {
                id: eff.id.clone(),
                enabled: eff.enabled,
                values: eff.values.resolved_at(time),
            };
            current = self.apply_effect(&resolved, current, w, h)?;
        }
        Ok(current)
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
