//! Frame cache. Per-comp, per-frame, per-effect-stack texture caching with
//! LRU eviction across two tiers:
//!
//! - **VRAM tier** — live `wgpu::Texture` handles, bounded by both an entry
//!   ceiling and a byte budget. This is the only tier the compositor reads
//!   from directly.
//! - **RAM tier** — frames evicted from VRAM are demoted to CPU memory as
//!   raw RGBA (a GPU readback), bounded by its own byte budget. A hit here
//!   re-uploads to a texture and promotes it back to VRAM.
//!
//! wgpu exposes no portable way to query a device's total/free VRAM, so the
//! VRAM budget is a conservative, caller-set ceiling rather than a fraction
//! of physically-detected memory. The point is the same: cap what the app
//! keeps GPU-resident and spill the rest to RAM instead of letting the
//! cache grow until `create_texture` returns out-of-memory.
//!
//! The key includes a hash of the layer's effect stack so a parameter
//! change implicitly invalidates the affected entries — no manual
//! invalidation API needed for the common edit-and-replay flow.

use crate::Renderer;
use crate::texture_io::{download_image, upload_image};
use felx_core::model::{Curve, Effect, InterpKind};
use felx_core::params::ParamValue;
use image::RgbaImage;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use tracing::debug;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub comp_id: u32,
    pub frame: u32,
    pub stack_hash: u64,
    /// Preview-scale denominator: 1 for full, 2 for half, 4 for quarter, etc.
    /// Scale changes naturally invalidate cached frames at the previous scale.
    pub scale_div: u8,
}

impl CacheKey {
    pub fn new(comp_id: u32, frame: u32, stack_hash: u64) -> Self {
        Self::with_scale(comp_id, frame, stack_hash, 1)
    }

    pub fn with_scale(comp_id: u32, frame: u32, stack_hash: u64, scale_div: u8) -> Self {
        Self {
            comp_id,
            frame,
            stack_hash,
            scale_div,
        }
    }
}

/// Hash a layer's effect stack — id, enabled flag, and every live
/// parameter value — into a stable u64. Any change a user can make from
/// the UI flips the hash, naturally invalidating the cache entry.
pub fn hash_effect_stack<'a, I>(stack: I) -> u64
where
    I: IntoIterator<Item = &'a Effect>,
{
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for eff in stack {
        eff.id.hash(&mut h);
        eff.enabled.hash(&mut h);
        for (id, val) in eff.values.iter() {
            id.hash(&mut h);
            hash_param_value(val, &mut h);
        }
    }
    h.finish()
}

fn hash_param_value(v: &ParamValue, h: &mut impl Hasher) {
    match v {
        ParamValue::Float(f) => {
            0u8.hash(h);
            f.to_bits().hash(h);
        }
        ParamValue::Int(i) => {
            1u8.hash(h);
            i.hash(h);
        }
        ParamValue::Bool(b) => {
            2u8.hash(h);
            b.hash(h);
        }
        ParamValue::Color(c) => {
            3u8.hash(h);
            for x in c {
                x.to_bits().hash(h);
            }
        }
        ParamValue::Vec2(v) => {
            4u8.hash(h);
            for x in v {
                x.to_bits().hash(h);
            }
        }
        ParamValue::Enum(s) => {
            5u8.hash(h);
            s.hash(h);
        }
        ParamValue::GroupEnabled(b) => {
            6u8.hash(h);
            b.hash(h);
        }
        ParamValue::FloatCurve(c) => {
            7u8.hash(h);
            hash_float_curve(c, h);
        }
    }
}

fn hash_float_curve(c: &Curve<f32>, h: &mut impl Hasher) {
    match c {
        Curve::Static(v) => {
            0u8.hash(h);
            v.to_bits().hash(h);
        }
        Curve::Animated(kfs) => {
            1u8.hash(h);
            (kfs.len() as u32).hash(h);
            for k in kfs {
                k.t.num.hash(h);
                k.t.den.hash(h);
                k.v.to_bits().hash(h);
                interp_kind_tag(k.interp).hash(h);
            }
        }
    }
}

fn interp_kind_tag(k: InterpKind) -> u8 {
    match k {
        InterpKind::Hold => 0,
        InterpKind::Linear => 1,
        InterpKind::EaseIn => 2,
        InterpKind::EaseOut => 3,
        InterpKind::EaseInOut => 4,
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Default VRAM byte budget for the GUI compositor's cache. Conservative
/// so it coexists with the viewer's working set, the video decoder's
/// surface pool, and (on a shared card) a concurrent export device,
/// without driving the GPU to out-of-memory. Tune via [`FrameCache::with_budget`].
pub const DEFAULT_VRAM_BUDGET_BYTES: usize = 384 * 1024 * 1024;
/// Default RAM spill budget — frames demoted out of VRAM live here.
pub const DEFAULT_RAM_BUDGET_BYTES: usize = 2 * 1024 * 1024 * 1024;
/// Floor the adaptive VRAM budget never drops below. Below this a single
/// frame barely fits and the cache stops being useful; if the GPU still
/// OOMs here, the render genuinely can't proceed at this resolution.
pub const MIN_VRAM_BUDGET_BYTES: usize = 32 * 1024 * 1024;

fn frame_bytes(width: u32, height: u32) -> usize {
    width as usize * height as usize * 4
}

/// A frame demoted from VRAM to CPU memory: raw RGBA8, tightly packed.
struct RamFrame {
    image: RgbaImage,
}

impl RamFrame {
    fn bytes(&self) -> usize {
        frame_bytes(self.image.width(), self.image.height())
    }
}

pub struct FrameCache {
    /// VRAM tier, LRU-ordered (front = oldest). Each entry carries its
    /// byte size so the budget check doesn't re-query the texture.
    entries: VecDeque<(CacheKey, wgpu::Texture, usize)>,
    /// RAM tier, LRU-ordered. Frames evicted from VRAM land here.
    ram: VecDeque<(CacheKey, RamFrame)>,
    max_entries: usize,
    vram_bytes: usize,
    ram_bytes: usize,
    max_vram_bytes: usize,
    max_ram_bytes: usize,
    stats: CacheStats,
}

impl FrameCache {
    /// Entry-count cap only; byte budgets default to the conservative
    /// constants above.
    pub fn new(max_entries: usize) -> Self {
        Self::with_budget(
            max_entries,
            DEFAULT_VRAM_BUDGET_BYTES,
            DEFAULT_RAM_BUDGET_BYTES,
        )
    }

    /// Full configuration: an entry ceiling plus VRAM and RAM byte budgets.
    /// A frame is admitted to VRAM until *either* the entry count or the
    /// VRAM byte budget would be exceeded, at which point the oldest VRAM
    /// frame is demoted to RAM; RAM evicts its oldest when its budget is
    /// exceeded.
    pub fn with_budget(max_entries: usize, max_vram_bytes: usize, max_ram_bytes: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            ram: VecDeque::new(),
            max_entries,
            vram_bytes: 0,
            ram_bytes: 0,
            max_vram_bytes,
            max_ram_bytes,
            stats: CacheStats::default(),
        }
    }

    /// Look up `key`. On a VRAM hit, bumps the entry to most-recent and
    /// returns a cheap cloned handle. On a RAM hit, re-uploads the frame to
    /// a texture, promotes it back into VRAM, and returns it. `renderer` is
    /// only touched on the RAM-promotion path.
    pub fn get(&mut self, key: CacheKey, renderer: &Renderer) -> Option<wgpu::Texture> {
        if let Some(idx) = self.entries.iter().position(|(k, _, _)| *k == key) {
            let (k, tex, bytes) = self.entries.remove(idx).expect("position checked");
            let cloned = tex.clone();
            self.entries.push_back((k, tex, bytes));
            self.stats.hits += 1;
            return Some(cloned);
        }
        // RAM tier: re-upload and promote back to VRAM.
        if let Some(idx) = self.ram.iter().position(|(k, _)| *k == key) {
            let (k, frame) = self.ram.remove(idx).expect("position checked");
            self.ram_bytes -= frame.bytes();
            let tex = upload_image(renderer, &frame.image);
            self.stats.hits += 1;
            self.admit_to_vram(k, tex, renderer);
            // Return the freshly-admitted handle (now at the back).
            return self.entries.back().map(|(_, t, _)| t.clone());
        }
        self.stats.misses += 1;
        None
    }

    pub fn insert(&mut self, key: CacheKey, texture: wgpu::Texture, renderer: &Renderer) {
        // Drop any existing entry for the same key from both tiers.
        self.invalidate(key);
        if self.max_entries == 0 {
            return; // Disabled cache.
        }
        self.admit_to_vram(key, texture, renderer);
    }

    /// Push a texture into the VRAM tier, then enforce both budgets by
    /// demoting oldest VRAM frames to RAM and evicting oldest RAM frames.
    fn admit_to_vram(&mut self, key: CacheKey, texture: wgpu::Texture, renderer: &Renderer) {
        let bytes = frame_bytes(texture.width(), texture.height());
        self.entries.push_back((key, texture, bytes));
        self.vram_bytes += bytes;

        // Demote oldest VRAM frames while over either the entry or byte cap.
        // Keep at least one frame so a single oversized frame still serves.
        while self.entries.len() > 1
            && (self.entries.len() > self.max_entries || self.vram_bytes > self.max_vram_bytes)
        {
            let (k, tex, b) = self.entries.pop_front().expect("len checked");
            self.vram_bytes -= b;
            self.stats.evictions += 1;
            self.demote_to_ram(k, &tex, renderer);
        }
        self.enforce_ram_budget();
    }

    /// Read a texture back to CPU memory and store it in the RAM tier.
    /// Skipped (frame simply dropped) if the RAM budget is zero or the
    /// frame alone exceeds it.
    fn demote_to_ram(&mut self, key: CacheKey, texture: &wgpu::Texture, renderer: &Renderer) {
        let bytes = frame_bytes(texture.width(), texture.height());
        if self.max_ram_bytes == 0 || bytes > self.max_ram_bytes {
            return;
        }
        let image = download_image(renderer, texture);
        self.ram.push_back((key, RamFrame { image }));
        self.ram_bytes += bytes;
        self.enforce_ram_budget();
    }

    fn enforce_ram_budget(&mut self) {
        while self.ram_bytes > self.max_ram_bytes {
            let Some((_, frame)) = self.ram.pop_front() else {
                break;
            };
            self.ram_bytes -= frame.bytes();
        }
    }

    pub fn invalidate(&mut self, key: CacheKey) -> bool {
        let mut hit = false;
        if let Some(idx) = self.entries.iter().position(|(k, _, _)| *k == key) {
            let (_, _, b) = self.entries.remove(idx).expect("position checked");
            self.vram_bytes -= b;
            hit = true;
        }
        if let Some(idx) = self.ram.iter().position(|(k, _)| *k == key) {
            let (_, frame) = self.ram.remove(idx).expect("position checked");
            self.ram_bytes -= frame.bytes();
            hit = true;
        }
        hit
    }

    pub fn invalidate_comp(&mut self, comp_id: u32) -> usize {
        let before = self.entries.len() + self.ram.len();
        self.entries.retain(|(k, _, b)| {
            let keep = k.comp_id != comp_id;
            if !keep {
                self.vram_bytes -= *b;
            }
            keep
        });
        self.ram.retain(|(k, frame)| {
            let keep = k.comp_id != comp_id;
            if !keep {
                self.ram_bytes -= frame.bytes();
            }
            keep
        });
        before - (self.entries.len() + self.ram.len())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.ram.clear();
        self.vram_bytes = 0;
        self.ram_bytes = 0;
    }

    /// Halve the VRAM budget (floored at [`MIN_VRAM_BUDGET_BYTES`]) and drop
    /// the entire VRAM tier to free GPU memory immediately. Called on a GPU
    /// out-of-memory: this is the adaptive part — the budget self-tunes
    /// down to what the running card can actually sustain, so the app isn't
    /// pinned to a hardcoded number. Frames are *dropped*, not demoted to
    /// RAM: a readback would allocate a staging buffer while already
    /// memory-starved. Returns the new VRAM budget in bytes.
    pub fn shrink_vram_budget(&mut self) -> usize {
        self.max_vram_bytes = (self.max_vram_bytes / 2).max(MIN_VRAM_BUDGET_BYTES);
        self.stats.evictions += self.entries.len() as u64;
        self.entries.clear();
        self.vram_bytes = 0;
        self.max_vram_bytes
    }

    pub fn vram_budget(&self) -> usize {
        self.max_vram_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.max_entries
    }

    /// Live VRAM bytes held by the cache's texture tier.
    pub fn vram_bytes(&self) -> usize {
        self.vram_bytes
    }

    /// Live RAM bytes held by the spill tier.
    pub fn ram_bytes(&self) -> usize {
        self.ram_bytes
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    /// Emit a debug-level log of current hit rate and counts. Call
    /// periodically from the compositor / playback loop to roll up.
    pub fn log_stats(&self) {
        let s = self.stats;
        debug!(
            target: "felx::cache",
            hits = s.hits,
            misses = s.misses,
            evictions = s.evictions,
            hit_rate = s.hit_rate(),
            entries = self.entries.len(),
            cap = self.max_entries,
            vram_mb = self.vram_bytes / (1024 * 1024),
            ram_frames = self.ram.len(),
            ram_mb = self.ram_bytes / (1024 * 1024),
            "frame cache stats"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wgpu::Texture` carries an `Arc` inside; we don't need a real device
    /// to test the cache structure, but we do need *some* texture to put in.
    /// Use a tiny CPU-only adapter to make the tests deterministic.
    fn fake_texture(renderer: &crate::Renderer, label: &str) -> wgpu::Texture {
        renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn try_renderer() -> Option<crate::Renderer> {
        crate::Renderer::new_headless(crate::RendererOptions {
            allow_software_fallback: true,
            ..Default::default()
        })
        .ok()
    }

    #[test]
    fn miss_then_hit() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(4);
        let key = CacheKey::new(1, 0, 0xAA);
        assert!(cache.get(key, &r).is_none());
        assert_eq!(cache.stats().misses, 1);

        cache.insert(key, fake_texture(&r, "a"), &r);
        assert!(cache.get(key, &r).is_some());
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn lru_evicts_oldest_at_cap() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::with_budget(2, usize::MAX, 0);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "0"), &r);
        cache.insert(CacheKey::new(1, 1, 0), fake_texture(&r, "1"), &r);
        cache.insert(CacheKey::new(1, 2, 0), fake_texture(&r, "2"), &r);

        assert!(
            cache.get(CacheKey::new(1, 0, 0), &r).is_none(),
            "frame 0 should have been evicted"
        );
        assert!(cache.get(CacheKey::new(1, 1, 0), &r).is_some());
        assert!(cache.get(CacheKey::new(1, 2, 0), &r).is_some());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn get_bumps_entry_to_most_recent() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::with_budget(2, usize::MAX, 0);
        let k0 = CacheKey::new(1, 0, 0);
        let k1 = CacheKey::new(1, 1, 0);
        let k2 = CacheKey::new(1, 2, 0);
        cache.insert(k0, fake_texture(&r, "0"), &r);
        cache.insert(k1, fake_texture(&r, "1"), &r);
        // Touch k0 — now k0 is most recent, k1 is oldest.
        let _ = cache.get(k0, &r);
        cache.insert(k2, fake_texture(&r, "2"), &r); // evicts k1, not k0
        assert!(cache.get(k0, &r).is_some());
        assert!(cache.get(k1, &r).is_none());
        assert!(cache.get(k2, &r).is_some());
    }

    #[test]
    fn parameter_change_invalidates_via_stack_hash() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(8);
        let mut e_on = Effect::new("gain");
        e_on.enabled = true;
        let mut e_off = Effect::new("gain");
        e_off.enabled = false;
        let key_v1 = CacheKey::new(1, 0, hash_effect_stack(std::slice::from_ref(&e_on)));
        let key_v2 = CacheKey::new(1, 0, hash_effect_stack(std::slice::from_ref(&e_off)));
        assert_ne!(key_v1.stack_hash, key_v2.stack_hash);

        cache.insert(key_v1, fake_texture(&r, "v1"), &r);
        assert!(cache.get(key_v2, &r).is_none());
        assert!(cache.get(key_v1, &r).is_some());
    }

    #[test]
    fn parameter_value_change_changes_stack_hash() {
        let mut e1 = Effect::new("gain");
        e1.values.set("gain", ParamValue::Float(1.0));
        let mut e2 = Effect::new("gain");
        e2.values.set("gain", ParamValue::Float(0.5));
        let h1 = hash_effect_stack(std::slice::from_ref(&e1));
        let h2 = hash_effect_stack(std::slice::from_ref(&e2));
        assert_ne!(h1, h2);
    }

    #[test]
    fn invalidate_comp_drops_only_that_comp() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(8);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "a"), &r);
        cache.insert(CacheKey::new(1, 1, 0), fake_texture(&r, "b"), &r);
        cache.insert(CacheKey::new(2, 0, 0), fake_texture(&r, "c"), &r);
        assert_eq!(cache.invalidate_comp(1), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zero_capacity_disables_caching() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(0);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "x"), &r);
        assert!(cache.get(CacheKey::new(1, 0, 0), &r).is_none());
    }

    #[test]
    fn hit_rate_is_correct() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(4);
        let k = CacheKey::new(1, 0, 0);
        cache.insert(k, fake_texture(&r, "a"), &r);
        let _ = cache.get(k, &r); // hit
        let _ = cache.get(k, &r); // hit
        let _ = cache.get(CacheKey::new(9, 9, 9), &r); // miss
        assert!((cache.stats().hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }

    /// An NxN RGBA texture with COPY_SRC so the cache can read it back when
    /// demoting to the RAM tier.
    fn sized_texture(renderer: &crate::Renderer, n: u32, label: &str) -> wgpu::Texture {
        renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    #[test]
    fn vram_byte_budget_demotes_to_ram_then_promotes_back() {
        let Some(r) = try_renderer() else {
            return;
        };
        // 64x64 RGBA = 16 KiB/frame. VRAM budget 40 KiB holds ~2 frames;
        // RAM budget generous. Entry ceiling high so bytes are the limit.
        let mut cache = FrameCache::with_budget(100, 40 * 1024, 4 * 1024 * 1024);
        let k0 = CacheKey::new(1, 0, 0);
        let k1 = CacheKey::new(1, 1, 0);
        let k2 = CacheKey::new(1, 2, 0);
        cache.insert(k0, sized_texture(&r, 64, "0"), &r);
        cache.insert(k1, sized_texture(&r, 64, "1"), &r);
        cache.insert(k2, sized_texture(&r, 64, "2"), &r); // pushes k0 to RAM

        assert_eq!(cache.len(), 2, "VRAM tier capped by byte budget");
        assert!(cache.ram_bytes() > 0, "k0 spilled to RAM");
        assert!(cache.vram_bytes() <= 40 * 1024);

        // k0 still served — promoted back from RAM, counts as a hit.
        assert!(cache.get(k0, &r).is_some());
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 0);
    }

    #[test]
    fn shrink_vram_budget_halves_and_drops_to_floor() {
        let mut cache = FrameCache::with_budget(64, 256 * 1024 * 1024, 0);
        assert_eq!(cache.shrink_vram_budget(), 128 * 1024 * 1024);
        assert_eq!(cache.shrink_vram_budget(), 64 * 1024 * 1024);
        // Ratchets down but never below the floor.
        for _ in 0..20 {
            cache.shrink_vram_budget();
        }
        assert_eq!(cache.vram_budget(), MIN_VRAM_BUDGET_BYTES);
    }

    #[test]
    fn shrink_clears_vram_tier_without_readback() {
        let Some(r) = try_renderer() else {
            return;
        };
        // 1x1 fake textures have no COPY_SRC; shrink must NOT read them back
        // (it drops, not demotes), so this must not panic.
        let mut cache = FrameCache::with_budget(64, 256 * 1024 * 1024, 1024 * 1024);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "a"), &r);
        cache.insert(CacheKey::new(1, 1, 0), fake_texture(&r, "b"), &r);
        cache.shrink_vram_budget();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.vram_bytes(), 0);
    }

    #[test]
    fn ram_budget_zero_drops_demoted_frames() {
        let Some(r) = try_renderer() else {
            return;
        };
        // No RAM tier: demoted frames are gone, not retrievable.
        let mut cache = FrameCache::with_budget(100, 40 * 1024, 0);
        cache.insert(CacheKey::new(1, 0, 0), sized_texture(&r, 64, "0"), &r);
        cache.insert(CacheKey::new(1, 1, 0), sized_texture(&r, 64, "1"), &r);
        cache.insert(CacheKey::new(1, 2, 0), sized_texture(&r, 64, "2"), &r);
        assert_eq!(cache.ram_bytes(), 0);
        assert!(cache.get(CacheKey::new(1, 0, 0), &r).is_none());
    }
}
