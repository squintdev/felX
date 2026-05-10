//! Integration tests for project file load / save.

use felx_core::model::{AssetKind, Effect, Framerate, LayerKind, LoadError, Project};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("felx-io-{label}-{pid}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn three_layer_project(asset_path: impl Into<PathBuf>) -> Project {
    let mut p = Project::new();
    let asset = p.add_asset(asset_path, AssetKind::Video);
    let comp_id = p.add_composition("main", 1920, 1080);
    let comp = p.composition_mut(comp_id).unwrap();
    comp.framerate = Framerate::FPS_30;
    comp.duration_frames = 300;
    let _video = comp.add_video("clip", asset);
    let _solid = comp.add_solid("bg", [0.05, 0.05, 0.08, 1.0]);
    let adj = comp.add_layer("color grade", LayerKind::Adjustment, 0, 300);
    comp.push_effect(adj, Effect::new("cc_toner"));
    p
}

#[test]
fn round_trip_preserves_project() {
    let dir = scratch_dir("round-trip");
    let asset_path = dir.join("media.mp4");
    std::fs::write(&asset_path, b"fake video bytes").unwrap();

    let original = three_layer_project(&asset_path);
    let project_path = dir.join("test.felx");
    original.save(&project_path).unwrap();
    let loaded = Project::load(&project_path).unwrap();

    assert_eq!(original, loaded);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn asset_path_under_project_dir_is_stored_relative() {
    let dir = scratch_dir("relative");
    let asset_path = dir.join("nested/clip.mp4");
    std::fs::create_dir_all(asset_path.parent().unwrap()).unwrap();
    std::fs::write(&asset_path, b"fake").unwrap();

    let project = three_layer_project(&asset_path);
    let project_path = dir.join("proj.felx");
    project.save(&project_path).unwrap();

    let raw = std::fs::read_to_string(&project_path).unwrap();
    assert!(
        raw.contains("\"nested/clip.mp4\"") || raw.contains("\"nested\\\\clip.mp4\""),
        "expected relative path 'nested/clip.mp4' in saved file, got:\n{raw}"
    );
    assert!(
        !raw.contains(dir.to_str().unwrap()),
        "absolute project dir should not appear in saved file"
    );

    let loaded = Project::load(&project_path).unwrap();
    assert_eq!(loaded.assets[0].path, asset_path);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn asset_path_outside_project_dir_stays_absolute() {
    let project_dir = scratch_dir("outside");
    let asset_dir = scratch_dir("asset");
    let asset_path = asset_dir.join("clip.mp4");
    std::fs::write(&asset_path, b"fake").unwrap();

    let project = three_layer_project(&asset_path);
    let project_path = project_dir.join("proj.felx");
    project.save(&project_path).unwrap();

    let raw = std::fs::read_to_string(&project_path).unwrap();
    let abs = asset_path.to_string_lossy().to_string();
    let abs_escaped = abs.replace('\\', "\\\\");
    assert!(
        raw.contains(&abs) || raw.contains(&abs_escaped),
        "expected absolute path in saved file"
    );

    let loaded = Project::load(&project_path).unwrap();
    assert_eq!(loaded.assets[0].path, asset_path);

    let _ = std::fs::remove_dir_all(&project_dir);
    let _ = std::fs::remove_dir_all(&asset_dir);
}

#[test]
fn unknown_format_version_fails_to_load() {
    let dir = scratch_dir("future");
    let project_path = dir.join("future.felx");
    let body = r#"(
        format_version: 999,
        assets: [],
        compositions: [],
        next_asset_id: 1,
        next_comp_id: 1,
    )"#;
    std::fs::write(&project_path, body).unwrap();

    let err = Project::load(&project_path).unwrap_err();
    assert!(matches!(
        err,
        LoadError::UnsupportedFormatVersion { found: 999, .. }
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_file_fails_to_load() {
    let dir = scratch_dir("corrupt");
    let project_path = dir.join("bad.felx");
    std::fs::write(&project_path, "this is not valid RON {{{ ").unwrap();

    let err = Project::load(&project_path).unwrap_err();
    assert!(matches!(err, LoadError::Parse(_)));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_file_returns_io_error() {
    let path = std::env::temp_dir().join(format!(
        "felx-missing-{}-{}.felx",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let err = Project::load(&path).unwrap_err();
    assert!(matches!(err, LoadError::Io(_)));
}

#[test]
fn loaded_project_validates() {
    // Make sure save/load doesn't subtly corrupt the model in a way that
    // breaks validate().
    let dir = scratch_dir("validate");
    let asset_path = dir.join("media.mp4");
    std::fs::write(&asset_path, b"fake").unwrap();

    let original = three_layer_project(&asset_path);
    assert!(original.validate().is_ok());

    let project_path = dir.join("p.felx");
    original.save(&project_path).unwrap();
    let loaded = Project::load(&project_path).unwrap();
    assert!(loaded.validate().is_ok());

    let _ = std::fs::remove_dir_all(&dir);
}
