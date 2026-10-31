//! Export settings are a preference of the export sheet, not part of the
//! edit: undo and redo leave them alone, and they are still saved.
use std::path::PathBuf;
use glide::app::editor::Editor;
use glide::project::model::{ExportSettings, Quality};
use glide::render::Gpu;
fn open(name: &str) -> (PathBuf, PathBuf, Editor) {
    let dir = std::env::temp_dir().join(format!("glide-export-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Clip.glide");
    glide::synth::write_package(&path, 4.0, 1280, 800).unwrap();
    let ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    (dir, path, ed)
}
fn custom() -> ExportSettings {
    ExportSettings { height: 720, fps: 30, quality: Quality::Web, ..Default::default() }
}
#[test]
fn export_settings_stay_out_of_the_undo_history() {
    let (dir, _, mut ed) = open("history");
    let s = custom();
    let n = ed.project.timeline.zooms.len();
    ed.set_export(s);
    ed.edit(|p| {
        p.timeline.add_zoom(1000.0, 2.0, 4000.0);
    });
    assert_eq!(ed.project.timeline.zooms.len(), n + 1);
    ed.undo();
    assert_eq!(ed.project.timeline.zooms.len(), n, "the zoom is undone");
    assert_eq!(ed.project.export, s, "undo must not change the export settings");
    ed.redo();
    assert_eq!(ed.project.timeline.zooms.len(), n + 1);
    assert_eq!(ed.project.export, s, "redo must not change the export settings");
    ed.undo();
    ed.undo();
    assert_eq!(ed.project.timeline.zooms.len(), n, "the export change was never an undo step");
    assert_eq!(ed.project.export, s, "an undo on an empty history leaves them alone");
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn undoing_an_edit_made_before_an_export_change_keeps_the_new_export_settings() {
    let (dir, _, mut ed) = open("before");
    let before = ed.project.export;
    let n = ed.project.timeline.zooms.len();
    ed.edit(|p| {
        p.timeline.add_zoom(1000.0, 2.0, 4000.0);
    });
    let s2 = custom();
    assert_ne!(s2, before);
    ed.set_export(s2);
    ed.undo();
    assert_eq!(ed.project.timeline.zooms.len(), n);
    assert_eq!(ed.project.export, s2);
    ed.redo();
    assert_eq!(ed.project.timeline.zooms.len(), n + 1);
    assert_eq!(ed.project.export, s2);
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn export_settings_are_still_saved() {
    let (dir, path, mut ed) = open("saved");
    let s = custom();
    ed.set_export(s);
    ed.save();
    let loaded = glide::project::Package::new(&path).load_project().unwrap();
    assert_eq!(loaded.export, s);
    let _ = std::fs::remove_dir_all(&dir);
}
