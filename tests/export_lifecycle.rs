//! A running export belongs to its editor: leaving the editor stops it, and a
//! second start while one runs is ignored.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glide::app::editor::Editor;
use glide::project::model::{ExportSettings, Quality};
use glide::render::Gpu;

/// A package long enough that an export is still running when the test acts.
fn open(name: &str) -> (PathBuf, Editor) {
    let dir = std::env::temp_dir().join(format!("glide-export-lifecycle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Clip.glide");
    glide::synth::write_package(&path, 8.0, 1280, 800).unwrap();
    let mut ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    ed.project.export = ExportSettings { height: 480, fps: 30, quality: Quality::Web, ..Default::default() };
    (dir, ed)
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// Only the package is left in `dir`: no final file, no temp file, no leftover.
fn assert_only_package(dir: &Path) {
    assert_eq!(names(dir), vec!["Clip.glide".to_string()], "export files in {}", dir.display());
}

/// Give a detached export thread time to finish writing, if it is going to.
fn settle() {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn dropping_the_editor_stops_its_export() {
    let (dir, mut ed) = open("drop");
    ed.start_export();
    drop(ed);
    settle();
    assert_only_package(&dir);
}

#[test]
fn a_second_start_while_running_is_ignored() {
    let (dir, mut ed) = open("second");
    ed.start_export();
    let first = ed.export.running.as_ref().map(|r| r.output.clone());
    assert!(first.is_some());
    ed.start_export();
    assert_eq!(ed.export.running.as_ref().map(|r| r.output.clone()), first);
    ed.cancel_export();
    assert_only_package(&dir);
}

#[test]
fn cancel_export_joins_and_cleans_up() {
    let (dir, mut ed) = open("cancel");
    ed.start_export();
    assert!(ed.export_running());
    ed.cancel_export();
    assert!(!ed.export_running());
    assert!(ed.export.running.is_none());
    assert!(ed.export.result.is_none());
    // The thread is joined: nothing is written after this point.
    assert_only_package(&dir);
    settle();
    assert_only_package(&dir);
}
