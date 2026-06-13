// Bloom — F-076. The runtime drives three separate pipelines:
//   1) threshold: extract bright pixels above a soft-kneed cutoff
//   2) separable Gaussian blur (horizontal then vertical, possibly N times
//      across a downsample chain)
//   3) additive composite: original + intensity * blurred-bright
//
// The actual WGSL for each pipeline lives inline in
// crates/felx-render/src/effects/bloom.rs because the three passes have
// distinct bind-group layouts and overlapping bindings can't co-exist
// in one module. This file is a placeholder + reference doc.
