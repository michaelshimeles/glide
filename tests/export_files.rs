//! What an export leaves on disk: the finished file and nothing else, the
//! previous file untouched when a run is cancelled, and a new name for each
//! default export.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use glide::export::{self, Job, Source};
use glide::project::model::{ExportSettings, Quality};

const SENTINEL: &[u8] = b"an earlier export, not a movie";

fn package(name: &str, secs: f64, w: u32, h: u32) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("glide-export-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Clip.glide");
    glide::synth::write_package(&path, secs, w, h).unwrap();
    (dir, path)
}

fn job(pkg: &Path, settings: ExportSettings, output: PathBuf) -> Job {
    let (project, recording) = glide::project::Package::new(pkg).load().unwrap();
    Job { project, recording, source: Source::Package(pkg.to_path_buf()), settings, output }
}

fn mp4_settings() -> ExportSettings {
    ExportSettings { height: 240, fps: 10, quality: Quality::Web, ..Default::default() }
}

fn gif_settings() -> ExportSettings {
    ExportSettings { height: 240, fps: 10, ..ExportSettings::gif_default() }
}

/// Run the job and ask it to stop after `after` progress callbacks.
fn run_cancelled(job: &Job, after: usize) -> anyhow::Result<export::Stats> {
    let cancel = AtomicBool::new(false);
    let mut calls = 0;
    export::run(
        job,
        &mut |_| {
            calls += 1;
            if calls >= after {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        &cancel,
    )
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// No temp file or AVFoundation leftover is in `dir`.
fn assert_no_leftovers(dir: &Path) {
    let leftovers: Vec<String> = names(dir).into_iter().filter(|n| n.contains(".partial") || n.contains(".sb-")).collect();
    assert!(leftovers.is_empty(), "leftovers in {}: {leftovers:?}", dir.display());
}

#[test]
fn a_finished_mp4_export_leaves_only_the_final_file() {
    let (dir, pkg) = package("mp4-ok", 2.0, 640, 400);
    let out = dir.join("out");
    let job = job(&pkg, mp4_settings(), out.join("movie.mp4"));
    let stats = export::run(&job, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(names(&out), ["movie.mp4"]);
    assert_eq!(stats.bytes, std::fs::metadata(out.join("movie.mp4")).unwrap().len());
    assert!(stats.bytes > 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_mp4_export_keeps_the_file_it_would_have_replaced() {
    let (dir, pkg) = package("mp4-cancel", 2.0, 640, 400);
    let out = dir.join("out.mp4");
    std::fs::write(&out, SENTINEL).unwrap();
    let before = names(&dir);
    let err = run_cancelled(&job(&pkg, mp4_settings(), out.clone()), 10).unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err:#}");
    assert_eq!(std::fs::read(&out).unwrap(), SENTINEL);
    assert_eq!(names(&dir), before);
    assert_no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_gif_export_keeps_the_file_it_would_have_replaced() {
    let (dir, pkg) = package("gif-cancel", 2.0, 640, 400);
    let out = dir.join("out.gif");
    std::fs::write(&out, SENTINEL).unwrap();
    let before = names(&dir);
    let err = run_cancelled(&job(&pkg, gif_settings(), out.clone()), 10).unwrap_err();
    assert!(err.to_string().contains("cancelled"), "{err:#}");
    assert_eq!(std::fs::read(&out).unwrap(), SENTINEL);
    assert_eq!(names(&dir), before);
    assert_no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cancelled_export_to_a_new_name_leaves_nothing_behind() {
    let (dir, pkg) = package("new-name-cancel", 2.0, 640, 400);
    let before = names(&dir);
    assert!(run_cancelled(&job(&pkg, mp4_settings(), dir.join("new.mp4")), 10).is_err());
    assert!(run_cancelled(&job(&pkg, gif_settings(), dir.join("new.gif")), 10).is_err());
    assert_eq!(names(&dir), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_finished_gif_export_leaves_only_the_final_file() {
    let (dir, pkg) = package("gif-ok", 2.0, 640, 400);
    let out = dir.join("out");
    export::run(&job(&pkg, gif_settings(), out.join("loop.gif")), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(names(&out), ["loop.gif"]);
    let bytes = std::fs::read(out.join("loop.gif")).unwrap();
    assert!(bytes.starts_with(b"GIF89a"), "a GIF header");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_explicit_output_replaces_the_file_already_there() {
    let (dir, pkg) = package("replace", 2.0, 640, 400);
    let out = dir.join("out.mp4");
    std::fs::write(&out, SENTINEL).unwrap();
    export::run(&job(&pkg, mp4_settings(), out.clone()), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let new = std::fs::read(&out).unwrap();
    assert_ne!(new, SENTINEL);
    assert!(new.len() > SENTINEL.len());
    assert_no_leftovers(&dir);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn default_exports_of_one_package_get_different_names() {
    let (dir, pkg) = package("default-names", 2.0, 640, 400);
    let settings = mp4_settings();
    let first = export::default_output(&pkg, &settings);
    assert_eq!(first, dir.join("Clip.mp4"));
    export::run(&job(&pkg, settings, first.clone()), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let second = export::default_output(&pkg, &settings);
    assert_eq!(second, dir.join("Clip 2.mp4"));
    export::run(&job(&pkg, settings, second.clone()), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(export::default_output(&pkg, &settings), dir.join("Clip 3.mp4"));
    assert!(first.exists() && second.exists());
    // A GIF has its own extension, so its first name is still free.
    assert_eq!(export::default_output(&pkg, &gif_settings()), dir.join("Clip.gif"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fit_to_size_ends_at_the_final_path_with_no_first_pass_left() {
    let (dir, pkg) = package("fit", 6.0, 1280, 800);
    let settings = ExportSettings { height: 720, fps: 30, quality: Quality::Web, target_mb: Some(1), ..Default::default() };
    let out = dir.join("fit.mp4");
    let stats = export::run(&job(&pkg, settings, out.clone()), &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert!(out.exists());
    assert_eq!(stats.bytes, std::fs::metadata(&out).unwrap().len());
    assert_no_leftovers(&dir);
    assert_eq!(names(&dir), ["Clip.glide", "fit.mp4"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_250_byte_output_name_exports() {
    let (dir, pkg) = package("long-name", 1.0, 320, 200);
    let out = dir.join("out");
    let file = format!("{}.mp4", "a".repeat(246));
    let job = job(&pkg, mp4_settings(), out.join(&file));
    export::run(&job, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(names(&out), [file]);
    assert_no_leftovers(&out);
}
