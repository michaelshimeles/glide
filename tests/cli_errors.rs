//! An unreadable `project.json` is reported, not replaced: `glide export` and
//! the editor refuse to open it and leave the file as it was.
use std::path::PathBuf;
use std::process::Command;

use glide::app::editor::Editor;
use glide::render::Gpu;

/// A synthetic package whose `project.json` is cut off mid-write.
fn package_with_truncated_project(name: &str) -> (PathBuf, PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("glide-cli-errors-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pkg = dir.join("Broken.glide");
    glide::synth::write_package(&pkg, 2.0, 640, 400).unwrap();
    let truncated = br#"{"name":"my edit","style":{"padding":33,"#.to_vec();
    std::fs::write(pkg.join("project.json"), &truncated).unwrap();
    (dir, pkg, truncated)
}

#[test]
fn export_of_an_unreadable_project_fails_and_leaves_it_alone() {
    let (dir, pkg, truncated) = package_with_truncated_project("export");
    let out = dir.join("out.mp4");
    let run = Command::new(env!("CARGO_BIN_EXE_glide"))
        .args(["export"])
        .arg(&pkg)
        .args(["--height", "400", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "export should fail; stderr: {stderr}");
    assert!(stderr.contains("project.json"), "stderr should name the file: {stderr}");
    assert_eq!(std::fs::read(pkg.join("project.json")).unwrap(), truncated);
    assert!(!out.exists(), "nothing was exported");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_an_unreadable_project_in_the_editor_fails_and_leaves_it_alone() {
    let (dir, pkg, truncated) = package_with_truncated_project("editor");
    let err = Editor::open(&pkg, Gpu::headless().unwrap()).err().expect("opening should fail");
    assert!(format!("{err:#}").contains("project.json"), "{err:#}");
    assert_eq!(std::fs::read(pkg.join("project.json")).unwrap(), truncated);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `glide export --synthetic` with one bad option: clap rejects it (exit 2)
/// before any encoder is created.
fn export_rejects(args: &[&str], names: &str) {
    let run = Command::new(env!("CARGO_BIN_EXE_glide")).args(["export", "--synthetic"]).args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(2), "{args:?} should be a usage error; stderr: {stderr}");
    assert!(stderr.contains(names), "stderr should name {names}: {stderr}");
}

#[test]
fn export_rejects_a_frame_rate_of_zero() {
    export_rejects(&["--fps", "0"], "--fps");
}

#[test]
fn export_rejects_a_frame_rate_above_120() {
    export_rejects(&["--fps", "121"], "--fps");
}

#[test]
fn export_rejects_a_height_above_8640() {
    export_rejects(&["--height", "99999"], "--height");
}

#[test]
fn export_rejects_a_target_size_of_zero() {
    export_rejects(&["--target-mb", "0"], "--target-mb");
}
