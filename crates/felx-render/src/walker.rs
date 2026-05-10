//! Headless render walker. Walks a composition's timeline frame-by-frame,
//! renders each through the [`Compositor`], and writes results as a PNG
//! sequence. Used by the CLI render path (F-109) and as a pre-encoder
//! step until the H.264 encoder ships in F-014.

use crate::compositor::{Compositor, CompositorError};
use crate::texture_io::download_image;
use felx_core::model::{CompId, Project};
use std::ops::Range;
use std::path::{Path, PathBuf};
use tracing::{debug_span, info};

#[derive(Debug)]
pub enum WalkError {
    Compositor(CompositorError),
    Io(std::io::Error),
    ImageEncode(image::ImageError),
    UnknownComposition,
}

impl std::fmt::Display for WalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalkError::Compositor(e) => write!(f, "compositor: {e}"),
            WalkError::Io(e) => write!(f, "io: {e}"),
            WalkError::ImageEncode(e) => write!(f, "image encode: {e}"),
            WalkError::UnknownComposition => write!(f, "unknown composition"),
        }
    }
}

impl std::error::Error for WalkError {}

impl From<CompositorError> for WalkError {
    fn from(e: CompositorError) -> Self {
        WalkError::Compositor(e)
    }
}
impl From<std::io::Error> for WalkError {
    fn from(e: std::io::Error) -> Self {
        WalkError::Io(e)
    }
}
impl From<image::ImageError> for WalkError {
    fn from(e: image::ImageError) -> Self {
        WalkError::ImageEncode(e)
    }
}

#[derive(Clone, Debug)]
pub struct PngSequenceOptions {
    /// Directory the frames are written to. Created if missing.
    pub output_dir: PathBuf,
    /// Filename pattern. `{frame:05}` is replaced with the zero-padded
    /// frame index. Example: `"frame_{frame:05}.png"` →
    /// `frame_00000.png`, `frame_00001.png`, ...
    pub filename_pattern: String,
}

impl PngSequenceOptions {
    pub fn new(output_dir: impl Into<PathBuf>, filename_pattern: impl Into<String>) -> Self {
        Self {
            output_dir: output_dir.into(),
            filename_pattern: filename_pattern.into(),
        }
    }
}

/// Render `range` of frames (exclusive upper bound) of `comp_id` and write
/// each as a PNG to `opts.output_dir`. Returns the number of frames written.
///
/// The compositor is borrowed mutably so its frame cache participates;
/// renders within the same comp on subsequent calls reuse cached frames.
pub fn render_to_png_sequence(
    compositor: &mut Compositor,
    project: &Project,
    comp_id: CompId,
    range: Range<u32>,
    opts: &PngSequenceOptions,
) -> Result<u32, WalkError> {
    let _comp = project
        .composition(comp_id)
        .ok_or(WalkError::UnknownComposition)?;
    std::fs::create_dir_all(&opts.output_dir)?;

    let mut written = 0u32;
    for frame in range.clone() {
        let _span = debug_span!("walker.frame", frame).entered();
        let texture = compositor.render_cached(project, comp_id, frame)?;
        let img = download_image(compositor.renderer(), &texture);
        let filename = render_filename(&opts.filename_pattern, frame);
        let path = opts.output_dir.join(filename);
        img.save(&path)?;
        written += 1;
    }
    info!(
        comp = comp_id.0,
        first = range.start,
        last = range.end,
        written,
        "render walker done"
    );
    Ok(written)
}

/// Tiny `{frame:NN}` formatter. Only the `frame:NN` substitution is
/// supported — keep it predictable.
fn render_filename(pattern: &str, frame: u32) -> String {
    if let Some(start) = pattern.find("{frame:") {
        let rest = &pattern[start + "{frame:".len()..];
        let Some(close) = rest.find('}') else {
            return pattern.replace("{frame}", &frame.to_string());
        };
        let width: usize = rest[..close].parse().unwrap_or(0);
        let after = &rest[close + 1..];
        let before = &pattern[..start];
        return format!("{before}{frame:0width$}{after}");
    }
    pattern.replace("{frame}", &frame.to_string())
}

/// Convenience: render every frame of `comp_id` (0..duration_frames).
pub fn render_full_comp_to_png_sequence(
    compositor: &mut Compositor,
    project: &Project,
    comp_id: CompId,
    output_dir: impl Into<PathBuf>,
    filename_pattern: impl Into<String>,
) -> Result<u32, WalkError> {
    let comp = project
        .composition(comp_id)
        .ok_or(WalkError::UnknownComposition)?;
    let range = 0..comp.duration_frames;
    let opts = PngSequenceOptions::new(output_dir, filename_pattern);
    render_to_png_sequence(compositor, project, comp_id, range, &opts)
}

/// Helper used by the CLI render runner when a destination doesn't already
/// exist as a directory.
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_substitutes_zero_padded_frame() {
        assert_eq!(
            render_filename("frame_{frame:05}.png", 7),
            "frame_00007.png"
        );
        assert_eq!(render_filename("out/{frame:03}.png", 124), "out/124.png");
        assert_eq!(render_filename("{frame}.png", 99), "99.png");
    }

    #[test]
    fn filename_no_template_is_pass_through() {
        assert_eq!(render_filename("static.png", 5), "static.png");
    }
}
