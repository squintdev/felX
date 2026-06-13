//! `felx` CLI — headless render runner (F-109).
//!
//! `felx render <project.felx> --comp <name> --out <path> --format <fmt> [opts]`
//!
//! Format options: h264 | h265 | prores422 | prores4444 | gif | png | exr | wav.
//! Common encoder options surface as `--crf`, `--bitrate`, `--max-bitrate`,
//! `--preset`, `--profile`, `--gop`, `--hw`. Defaults match the
//! `EncodeOptions::*_default` profiles per codec.

use felx_core::model::{CompId, Project};
use felx_media::{EncodeOptions, HwEncoder, RateControl, WavBitDepth};
use felx_render::audio_export::export_wav;
use felx_render::compositor::Compositor;
use felx_render::gif_export::{GifDither, GifOptions, export_gif};
use felx_render::video_export::export_video;
use felx_render::walker::{
    ExrSequenceOptions, PngSequenceOptions, render_to_exr_sequence, render_to_png_sequence,
};
use felx_render::{Renderer, RendererOptions};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    felx_core::diagnostics::init_tracing();
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "felx starting");
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return print_help();
    }
    match args[1].as_str() {
        "render" => match cmd_render(&args[2..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("felx render: {e}");
                ExitCode::FAILURE
            }
        },
        "help" | "-h" | "--help" => print_help(),
        other => {
            eprintln!("unknown subcommand: {other}");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn print_help() -> ExitCode {
    eprintln!(
        "felx — analog-felx CLI\n\
         \n\
         Usage:\n\
         \tfelx render <project.felx> --out <path> --format <fmt> [opts]\n\
         \n\
         Formats:  h264 | h265 | prores422 | prores4444 | gif | png | exr | wav\n\
         \n\
         Common options:\n\
         \t--comp <name>             Composition name (default: first comp)\n\
         \t--crf <0..51>             CRF rate-control value (h264/h265)\n\
         \t--bitrate <bps>           Target bitrate (CBR / VBR)\n\
         \t--max-bitrate <bps>       Max bitrate (VBR / VBV)\n\
         \t--preset <name>           Encoder preset (ultrafast..veryslow)\n\
         \t--profile <name>          baseline/main/high (h264), main/main10 (h265),\n\
         \t                          proxy/lt/standard/hq/4444 (prores)\n\
         \t--gop <frames>            Keyframe interval\n\
         \t--hw <auto|nvenc|vaapi|videotoolbox>\n\
         \t--gif-palette <8..256>    GIF palette size\n\
         \t--gif-dither <none|bayer|floyd|sierra>\n\
         \t--png-pattern <pat>       PNG filename pattern (default frame_{{frame:05}}.png)\n\
         \t--exr-pattern <pat>       EXR filename pattern\n\
         \t--wav-depth <16|24|f32>   WAV bit depth\n"
    );
    ExitCode::SUCCESS
}

fn cmd_render(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("missing project file path".into());
    }
    let project_path = PathBuf::from(&args[0]);
    let mut comp_name: Option<String> = None;
    let mut out_path: Option<PathBuf> = None;
    let mut format: Option<String> = None;
    let mut crf: Option<u32> = None;
    let mut bitrate: Option<u64> = None;
    let mut max_bitrate: Option<u64> = None;
    let mut preset: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut gop: Option<u32> = None;
    let mut hw = HwEncoder::Software;
    let mut gif_palette: u32 = 128;
    let mut gif_dither = GifDither::FloydSteinberg;
    let mut png_pattern: String = "frame_{frame:05}.png".to_string();
    let mut exr_pattern: String = "frame_{frame:05}.exr".to_string();
    let mut wav_depth = WavBitDepth::Pcm16;

    let mut i = 1;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args.get(i + 1);
        match flag {
            "--comp" => {
                comp_name = Some(value.ok_or("missing --comp value")?.clone());
                i += 2;
            }
            "--out" => {
                out_path = Some(PathBuf::from(value.ok_or("missing --out value")?));
                i += 2;
            }
            "--format" => {
                format = Some(value.ok_or("missing --format value")?.clone());
                i += 2;
            }
            "--crf" => {
                crf = Some(
                    value
                        .ok_or("missing --crf value")?
                        .parse()
                        .map_err(|_| "invalid --crf")?,
                );
                i += 2;
            }
            "--bitrate" => {
                bitrate = Some(
                    value
                        .ok_or("missing --bitrate value")?
                        .parse()
                        .map_err(|_| "invalid --bitrate")?,
                );
                i += 2;
            }
            "--max-bitrate" => {
                max_bitrate = Some(
                    value
                        .ok_or("missing --max-bitrate value")?
                        .parse()
                        .map_err(|_| "invalid --max-bitrate")?,
                );
                i += 2;
            }
            "--preset" => {
                preset = Some(value.ok_or("missing --preset value")?.clone());
                i += 2;
            }
            "--profile" => {
                profile = Some(value.ok_or("missing --profile value")?.clone());
                i += 2;
            }
            "--gop" => {
                gop = Some(
                    value
                        .ok_or("missing --gop value")?
                        .parse()
                        .map_err(|_| "invalid --gop")?,
                );
                i += 2;
            }
            "--hw" => {
                hw = match value.ok_or("missing --hw value")?.as_str() {
                    "auto" | "software" => HwEncoder::Software,
                    "nvenc" => HwEncoder::Nvenc,
                    "vaapi" => HwEncoder::Vaapi,
                    "videotoolbox" => HwEncoder::VideoToolbox,
                    other => return Err(format!("unknown --hw value: {other}")),
                };
                i += 2;
            }
            "--gif-palette" => {
                gif_palette = value
                    .ok_or("missing --gif-palette value")?
                    .parse()
                    .map_err(|_| "invalid --gif-palette")?;
                i += 2;
            }
            "--gif-dither" => {
                gif_dither = match value.ok_or("missing --gif-dither value")?.as_str() {
                    "none" => GifDither::None,
                    "bayer" => GifDither::Bayer,
                    "floyd" | "floyd_steinberg" => GifDither::FloydSteinberg,
                    "sierra" | "sierra2_4a" => GifDither::Sierra2_4a,
                    other => return Err(format!("unknown --gif-dither: {other}")),
                };
                i += 2;
            }
            "--png-pattern" => {
                png_pattern = value.ok_or("missing --png-pattern value")?.clone();
                i += 2;
            }
            "--exr-pattern" => {
                exr_pattern = value.ok_or("missing --exr-pattern value")?.clone();
                i += 2;
            }
            "--wav-depth" => {
                wav_depth = match value.ok_or("missing --wav-depth value")?.as_str() {
                    "16" => WavBitDepth::Pcm16,
                    "24" => WavBitDepth::Pcm24,
                    "f32" | "32f" => WavBitDepth::Float32,
                    other => return Err(format!("unknown --wav-depth: {other}")),
                };
                i += 2;
            }
            other => return Err(format!("unknown flag: {other}")),
        }
    }

    let out_path = out_path.ok_or("--out required")?;
    let format = format.ok_or("--format required")?;

    // Load the project file.
    let project = Project::load(&project_path).map_err(|e| format!("load project: {e}"))?;
    let comp_id = pick_comp(&project, comp_name.as_deref())?;
    let comp = project.composition(comp_id).ok_or("comp not found")?;
    let (w, h) = (comp.width, comp.height);
    let (fps_num, fps_den) = (comp.framerate.0.num, comp.framerate.0.den);
    let dur = comp.duration_frames;

    // Set up the compositor headlessly.
    let renderer = Renderer::new_headless(RendererOptions {
        allow_software_fallback: true,
        ..Default::default()
    })
    .map_err(|e| format!("renderer init: {e}"))?;
    let mut compositor = Compositor::new(renderer);

    match format.as_str() {
        "h264" | "h265" => {
            let mut opts = if format == "h264" {
                EncodeOptions::h264_default(w, h, fps_num, fps_den)
            } else {
                EncodeOptions::h265_default(w, h, fps_num, fps_den)
            };
            if let Some(v) = crf {
                opts.crf = v;
                opts.rate_control = RateControl::Crf;
            }
            if let Some(v) = bitrate {
                opts.target_bitrate = v;
                opts.rate_control = if max_bitrate.is_some() {
                    RateControl::Vbr
                } else {
                    RateControl::Cbr
                };
            }
            if let Some(v) = max_bitrate {
                opts.max_bitrate = v;
            }
            if let Some(v) = preset {
                opts.preset = v;
            }
            if let Some(v) = profile {
                opts.profile = v;
            }
            if let Some(v) = gop {
                opts.keyframe_interval = v;
            }
            opts.hw = hw;
            encode_video(&mut compositor, &project, comp_id, dur, &out_path, opts)?;
        }
        "prores422" | "prores4444" => {
            let mut opts = if format == "prores422" {
                EncodeOptions::prores422_default(w, h, fps_num, fps_den)
            } else {
                EncodeOptions::prores4444_default(w, h, fps_num, fps_den)
            };
            if let Some(v) = profile {
                opts.profile = v;
            }
            encode_video(&mut compositor, &project, comp_id, dur, &out_path, opts)?;
        }
        "gif" => {
            let opts = GifOptions {
                max_palette: gif_palette,
                dither: gif_dither,
                framerate_cap: None,
                loop_forever: true,
                transparency: false,
            };
            export_gif(&mut compositor, &project, comp_id, &out_path, &opts)
                .map_err(|e| format!("gif: {e}"))?;
        }
        "png" => {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("mkdir out: {e}"))?;
            let opts = PngSequenceOptions::new(out_path.clone(), png_pattern);
            render_to_png_sequence(&mut compositor, &project, comp_id, 0..dur, &opts)
                .map_err(|e| format!("png: {e}"))?;
        }
        "exr" => {
            std::fs::create_dir_all(&out_path).map_err(|e| format!("mkdir out: {e}"))?;
            let opts = ExrSequenceOptions::new(out_path.clone(), exr_pattern);
            render_to_exr_sequence(&mut compositor, &project, comp_id, 0..dur, &opts)
                .map_err(|e| format!("exr: {e}"))?;
        }
        "wav" => {
            export_wav(&project, comp_id, &out_path, 48_000, wav_depth)
                .map_err(|e| format!("wav: {e}"))?;
        }
        other => return Err(format!("unknown format: {other}")),
    }
    tracing::info!(out = %out_path.display(), "render done");
    Ok(())
}

fn pick_comp(project: &Project, name: Option<&str>) -> Result<CompId, String> {
    let comps = &project.compositions;
    if let Some(name) = name {
        comps
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.id)
            .ok_or_else(|| format!("no comp named '{name}'"))
    } else {
        comps
            .first()
            .map(|c| c.id)
            .ok_or_else(|| "project has no compositions".into())
    }
}

fn encode_video(
    compositor: &mut Compositor,
    project: &Project,
    comp_id: CompId,
    _duration: u32,
    out_path: &std::path::Path,
    opts: EncodeOptions,
) -> Result<(), String> {
    export_video(
        compositor,
        project,
        comp_id,
        out_path,
        opts,
        report_progress,
    )
    .map_err(|e| e.to_string())
}

/// F-108 v1: structured progress via tracing.
fn report_progress(done: u32, total: u32) {
    let pct = if total == 0 {
        0.0
    } else {
        (done as f32 / total as f32) * 100.0
    };
    tracing::info!(target: "felx::progress", done, total, pct, "render progress");
}
