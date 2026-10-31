//! Opening a project: edits are flushed first, a failed open leaves the
//! running export alone, and reopening the open project builds nothing.
use std::path::{Path, PathBuf};
use glide::app::editor::Editor;
use glide::app::lifecycle::{Leaving, Switch, same_project, switch_to};
use glide::project::model::{ExportSettings, Quality};
use glide::render::Gpu;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("glide-open-flow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An 8 s package, long enough that an export is still running when the test acts.
fn package(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    glide::synth::write_package(&path, 8.0, 1280, 800).unwrap();
    path
}

#[test]
fn pending_edits_survive_reopening_the_same_project() {
    let dir = scratch("same");
    let path = package(&dir, "Clip.glide");
    let gpu = Gpu::headless().unwrap();
    let mut ed = Editor::open(&path, gpu.clone()).unwrap();
    ed.edit(|p| p.style.padding = 37.0);

    // Same project, spelled with dot segments: nothing is built and the edit stays.
    let spelled = dir.join(".").join("Clip.glide").join("");
    let same = same_project(&ed.package.root, &spelled);
    let out = switch_to(Some(&mut ed), same, false, Leaving::OpenProject, || -> anyhow::Result<Editor> { panic!("no second editor") }, |_| panic!("no question"));
    assert!(matches!(out, Switch::Same));
    assert_eq!(ed.project.style.padding, 37.0);

    // A different open of the file: the flush has written the edit before the new editor reads it.
    let out = switch_to(Some(&mut ed), false, false, Leaving::OpenProject, || Editor::open(&path, gpu), |_| panic!("no export, no question"));
    let Switch::Opened(fresh) = out else { panic!("expected the new editor") };
    assert_eq!(fresh.project.style.padding, 37.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_project_does_not_cancel_the_running_export() {
    let dir = scratch("corrupt");
    let good = package(&dir, "Clip.glide");
    let bad = package(&dir, "Broken.glide");
    std::fs::write(bad.join("project.json"), "not json").unwrap();
    let gpu = Gpu::headless().unwrap();
    let mut ed = Editor::open(&good, gpu.clone()).unwrap();
    ed.project.export = ExportSettings { height: 480, fps: 30, quality: Quality::Web, ..Default::default() };
    ed.start_export();
    assert!(ed.export_running());

    let out = switch_to(Some(&mut ed), false, false, Leaving::OpenProject, || Editor::open(&bad, gpu), |_| panic!("a failed open asks nothing"));
    assert!(matches!(out, Switch::Failed(_)));
    assert!(ed.export_running());
    ed.cancel_export();
    let _ = std::fs::remove_dir_all(&dir);
}
