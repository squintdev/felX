//! Frame cache. Per-comp, per-frame, per-effect-stack texture caching with
//! LRU eviction. The on-disk RAM tier (zstd-compressed pixel buffers) is
//! deferred until the VRAM tier proves insufficient in practice.
//!
//! The key includes a hash of the layer's effect stack so a parameter
//! change implicitly invalidates the affected entries — no manual
//! invalidation API needed for the common edit-and-replay flow.

use felx_core::model::{Curve, Effect, InterpKind};
use felx_core::params::ParamValue;
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

pub struct FrameCache {
    entries: VecDeque<(CacheKey, wgpu::Texture)>,
    max_entries: usize,
    /// Reserved for the RAM-tier rollover (zstd-compressed pixel buffers).
    /// Currently unused; tracked here so the configuration surface is
    /// stable when the second tier lands.
    pub max_ram_bytes: usize,
    stats: CacheStats,
}

impl FrameCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
            max_ram_bytes: 0,
            stats: CacheStats::default(),
        }
    }

    /// Look up `key`, returning a (cheap) cloned texture handle on hit.
    /// Side effect: bumps the entry to most-recent and updates stats.
    pub fn get(&mut self, key: CacheKey) -> Option<wgpu::Texture> {
        if let Some(idx) = self.entries.iter().position(|(k, _)| *k == key) {
            let (k, tex) = self.entries.remove(idx).expect("position checked");
            let cloned = tex.clone();
            self.entries.push_back((k, tex));
            self.stats.hits += 1;
            Some(cloned)
        } else {
            self.stats.misses += 1;
            None
        }
    }

    pub fn insert(&mut self, key: CacheKey, texture: wgpu::Texture) {
        // Replace any existing entry for the same key.
        if let Some(idx) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(idx);
        }

        if self.max_entries == 0 {
            // Disabled cache: don't store.
            return;
        }

        while self.entries.len() >= self.max_entries {
            self.entries.pop_front();
            self.stats.evictions += 1;
        }
        self.entries.push_back((key, texture));
    }

    pub fn invalidate(&mut self, key: CacheKey) -> bool {
        if let Some(idx) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn invalidate_comp(&mut self, comp_id: u32) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| k.comp_id != comp_id);
        before - self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
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
        assert!(cache.get(key).is_none());
        assert_eq!(cache.stats().misses, 1);

        cache.insert(key, fake_texture(&r, "a"));
        assert!(cache.get(key).is_some());
        assert_eq!(cache.stats().hits, 1);
    }

    #[test]
    fn lru_evicts_oldest_at_cap() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(2);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "0"));
        cache.insert(CacheKey::new(1, 1, 0), fake_texture(&r, "1"));
        cache.insert(CacheKey::new(1, 2, 0), fake_texture(&r, "2"));

        assert!(
            cache.get(CacheKey::new(1, 0, 0)).is_none(),
            "frame 0 should have been evicted"
        );
        assert!(cache.get(CacheKey::new(1, 1, 0)).is_some());
        assert!(cache.get(CacheKey::new(1, 2, 0)).is_some());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn get_bumps_entry_to_most_recent() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(2);
        let k0 = CacheKey::new(1, 0, 0);
        let k1 = CacheKey::new(1, 1, 0);
        let k2 = CacheKey::new(1, 2, 0);
        cache.insert(k0, fake_texture(&r, "0"));
        cache.insert(k1, fake_texture(&r, "1"));
        // Touch k0 — now k0 is most recent, k1 is oldest.
        let _ = cache.get(k0);
        cache.insert(k2, fake_texture(&r, "2")); // evicts k1, not k0
        assert!(cache.get(k0).is_some());
        assert!(cache.get(k1).is_none());
        assert!(cache.get(k2).is_some());
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

        cache.insert(key_v1, fake_texture(&r, "v1"));
        assert!(cache.get(key_v2).is_none());
        assert!(cache.get(key_v1).is_some());
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
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "a"));
        cache.insert(CacheKey::new(1, 1, 0), fake_texture(&r, "b"));
        cache.insert(CacheKey::new(2, 0, 0), fake_texture(&r, "c"));
        assert_eq!(cache.invalidate_comp(1), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zero_capacity_disables_caching() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(0);
        cache.insert(CacheKey::new(1, 0, 0), fake_texture(&r, "x"));
        assert!(cache.get(CacheKey::new(1, 0, 0)).is_none());
    }

    #[test]
    fn hit_rate_is_correct() {
        let Some(r) = try_renderer() else {
            return;
        };
        let mut cache = FrameCache::new(4);
        let k = CacheKey::new(1, 0, 0);
        cache.insert(k, fake_texture(&r, "a"));
        let _ = cache.get(k); // hit
        let _ = cache.get(k); // hit
        let _ = cache.get(CacheKey::new(9, 9, 9)); // miss
        assert!((cache.stats().hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }
}
