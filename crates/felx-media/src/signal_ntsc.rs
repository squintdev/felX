//! F-071a: full ntsc-rs adapter.
//!
//! Thin wrapper over `ntsc-rs`'s `NtscEffect::apply_effect_to_buffer`. Maps
//! a `felx_core::params::ParamValues` view (the per-effect parameter tree)
//! onto `NtscEffect`'s full settings surface — including the optional
//! sub-blocks (`head_switching`, `tracking_noise`, `composite_noise`,
//! `ringing`, `luma_noise`, `chroma_noise`, `vhs_settings`).
//!
//! Operates in place on an `RgbaImage` so the compositor can use the
//! existing CPU-pass machinery.

use felx_core::params::ParamValues;
use image::RgbaImage;
use ntscrs::settings::standard::{NtscEffect, VHSTapeSpeed};
use ntscrs::yiq_fielding::Rgbx;

/// Apply the ntsc-rs effect in place. `frame_num` is the comp-timeline
/// frame; ntsc-rs uses it to advance per-frame state (noise seeds,
/// scanline phase wobble).
pub fn apply_signal(img: &mut RgbaImage, values: &ParamValues, frame_num: u32) {
    let effect = build_effect(values);
    let (w, h) = img.dimensions();
    effect.apply_effect_to_buffer::<Rgbx, u8>(
        (w as usize, h as usize),
        img.as_mut(),
        frame_num as usize,
        [1.0, 1.0],
    );
}

fn build_effect(p: &ParamValues) -> NtscEffect {
    use ntscrs::settings::standard::{
        ChromaDemodulationFilter, FbmNoiseSettings, FilterType, HeadSwitchingSettings, PhaseShift,
        RingingSettings, TrackingNoiseSettings, UseField, VHSEdgeWaveSettings, VHSSettings,
        VHSSharpenSettings,
    };

    let mut e = NtscEffect::default();
    e.random_seed = p.int("random_seed").unwrap_or(0);
    e.use_field = match p.enum_str("use_field").unwrap_or("interleaved_upper") {
        "alternating" => UseField::Alternating,
        "upper" => UseField::Upper,
        "lower" => UseField::Lower,
        "both" => UseField::Both,
        "interleaved_lower" => UseField::InterleavedLower,
        _ => UseField::InterleavedUpper,
    };
    e.filter_type = match p.enum_str("filter_type").unwrap_or("butterworth") {
        "constant_k" => FilterType::ConstantK,
        _ => FilterType::Butterworth,
    };
    e.input_luma_filter = lumalp(p.enum_str("input_luma_filter").unwrap_or("notch"));
    e.chroma_lowpass_in = chromalp(p.enum_str("chroma_lowpass_in").unwrap_or("full"));
    e.chroma_demodulation = match p.enum_str("chroma_demodulation").unwrap_or("notch") {
        "box" => ChromaDemodulationFilter::Box,
        "one_line_comb" => ChromaDemodulationFilter::OneLineComb,
        "two_line_comb" => ChromaDemodulationFilter::TwoLineComb,
        _ => ChromaDemodulationFilter::Notch,
    };
    e.luma_smear = p.float("luma_smear").unwrap_or(0.5);
    e.composite_sharpening = p.float("composite_sharpening").unwrap_or(1.0);
    e.video_scanline_phase_shift = match p.int("video_scanline_phase_shift_deg").unwrap_or(180) {
        0 => PhaseShift::Degrees0,
        90 => PhaseShift::Degrees90,
        270 => PhaseShift::Degrees270,
        _ => PhaseShift::Degrees180,
    };
    e.video_scanline_phase_shift_offset = p.int("video_scanline_phase_shift_offset").unwrap_or(0);
    e.snow_intensity = p.float("snow_intensity").unwrap_or(0.0);
    e.snow_anisotropy = p.float("snow_anisotropy").unwrap_or(0.5);
    e.chroma_phase_noise_intensity = p.float("chroma_phase_noise_intensity").unwrap_or(0.0);
    e.chroma_phase_error = p.float("chroma_phase_error").unwrap_or(0.0);
    e.chroma_delay_horizontal = p.float("chroma_delay_horizontal").unwrap_or(0.0);
    e.chroma_delay_vertical = p.int("chroma_delay_vertical").unwrap_or(0);
    e.chroma_vert_blend = p.bool("chroma_vert_blend").unwrap_or(true);
    e.chroma_lowpass_out = chromalp(p.enum_str("chroma_lowpass_out").unwrap_or("full"));

    // Composite noise.
    if p.group_enabled("composite_noise").unwrap_or(false) {
        e.composite_noise = Some(FbmNoiseSettings {
            intensity: p.float("composite_noise.intensity").unwrap_or(0.1),
            frequency: p.float("composite_noise.frequency").unwrap_or(0.5),
            detail: p.int("composite_noise.detail").unwrap_or(1),
        });
    } else {
        e.composite_noise = None;
    }

    // Luma noise.
    if p.group_enabled("luma_noise").unwrap_or(false) {
        e.luma_noise = Some(FbmNoiseSettings {
            intensity: p.float("luma_noise.intensity").unwrap_or(0.0),
            frequency: p.float("luma_noise.frequency").unwrap_or(0.5),
            detail: p.int("luma_noise.detail").unwrap_or(1),
        });
    } else {
        e.luma_noise = None;
    }

    // Chroma noise.
    if p.group_enabled("chroma_noise").unwrap_or(false) {
        e.chroma_noise = Some(FbmNoiseSettings {
            intensity: p.float("chroma_noise.intensity").unwrap_or(0.0),
            frequency: p.float("chroma_noise.frequency").unwrap_or(0.05),
            detail: p.int("chroma_noise.detail").unwrap_or(1),
        });
    } else {
        e.chroma_noise = None;
    }

    // Ringing.
    if p.group_enabled("ringing").unwrap_or(false) {
        e.ringing = Some(RingingSettings {
            frequency: p.float("ringing.frequency").unwrap_or(0.5),
            power: p.float("ringing.power").unwrap_or(3.0),
            intensity: p.float("ringing.intensity").unwrap_or(0.5),
        });
    } else {
        e.ringing = None;
    }

    // Head switching.
    if p.group_enabled("head_switching").unwrap_or(false) {
        e.head_switching = Some(HeadSwitchingSettings {
            height: p.int("head_switching.height").unwrap_or(8),
            offset: p.int("head_switching.offset").unwrap_or(3),
            horiz_shift: p.float("head_switching.horiz_shift").unwrap_or(72.0),
            mid_line: None,
        });
    } else {
        e.head_switching = None;
    }

    // Tracking noise.
    if p.group_enabled("tracking_noise").unwrap_or(false) {
        e.tracking_noise = Some(TrackingNoiseSettings {
            height: p.int("tracking_noise.height").unwrap_or(24),
            wave_intensity: p.float("tracking_noise.wave_intensity").unwrap_or(5.0),
            snow_intensity: p.float("tracking_noise.snow_intensity").unwrap_or(0.05),
            snow_anisotropy: p.float("tracking_noise.snow_anisotropy").unwrap_or(0.5),
            noise_intensity: p.float("tracking_noise.noise_intensity").unwrap_or(0.005),
        });
    } else {
        e.tracking_noise = None;
    }

    // VHS settings (the user explicitly asked for this).
    if p.group_enabled("vhs_settings").unwrap_or(false) {
        let sharpen = if p.group_enabled("vhs_settings.sharpen").unwrap_or(true) {
            Some(VHSSharpenSettings {
                intensity: p.float("vhs_settings.sharpen.intensity").unwrap_or(1.5),
                frequency: p.float("vhs_settings.sharpen.frequency").unwrap_or(1.0),
            })
        } else {
            None
        };
        let edge_wave = if p.group_enabled("vhs_settings.edge_wave").unwrap_or(true) {
            Some(VHSEdgeWaveSettings {
                intensity: p.float("vhs_settings.edge_wave.intensity").unwrap_or(1.0),
                speed: p.float("vhs_settings.edge_wave.speed").unwrap_or(4.0),
                frequency: p.float("vhs_settings.edge_wave.frequency").unwrap_or(0.05),
                detail: p.int("vhs_settings.edge_wave.detail").unwrap_or(1),
            })
        } else {
            None
        };
        let tape_speed = match p.enum_str("vhs_settings.tape_speed").unwrap_or("sp") {
            "lp" => VHSTapeSpeed::LP,
            "ep" => VHSTapeSpeed::EP,
            "off" => VHSTapeSpeed::NONE,
            _ => VHSTapeSpeed::SP,
        };
        e.vhs_settings = Some(VHSSettings {
            tape_speed,
            chroma_loss: p.float("vhs_settings.chroma_loss").unwrap_or(0.0),
            sharpen,
            edge_wave,
        });
    } else {
        e.vhs_settings = None;
    }

    // Scale stays at default (None) — felx already owns layer transforms.
    e.scale = None;

    e
}

fn lumalp(s: &str) -> ntscrs::settings::standard::LumaLowpass {
    use ntscrs::settings::standard::LumaLowpass;
    match s {
        "none" => LumaLowpass::None,
        "box" => LumaLowpass::Box,
        _ => LumaLowpass::Notch,
    }
}

fn chromalp(s: &str) -> ntscrs::settings::standard::ChromaLowpass {
    use ntscrs::settings::standard::ChromaLowpass;
    match s {
        "none" => ChromaLowpass::None,
        "light" => ChromaLowpass::Light,
        _ => ChromaLowpass::Full,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn default_settings_run_without_panicking() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(64, 32, Rgba([200, 50, 80, 255]));
        let values = ParamValues::new();
        apply_signal(&mut img, &values, 0);
        // Alpha channel should remain 255 (ntsc-rs operates on YIQ, doesn't
        // touch alpha through the Rgbx PixelFormat).
        for p in img.pixels() {
            assert_eq!(p[3], 255);
        }
    }

    #[test]
    fn high_snow_changes_pixels() {
        let mut img: RgbaImage = ImageBuffer::from_pixel(64, 32, Rgba([100, 100, 100, 255]));
        let original = img.clone();
        let mut values = ParamValues::new();
        values.set("snow_intensity", felx_core::params::ParamValue::Float(1.0));
        apply_signal(&mut img, &values, 0);
        // With heavy snow, at least some pixels should differ from input.
        let diff_count = img
            .pixels()
            .zip(original.pixels())
            .filter(|(a, b)| a.0 != b.0)
            .count();
        assert!(diff_count > 16, "expected snow to perturb pixels");
    }
}
