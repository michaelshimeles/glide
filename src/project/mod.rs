//! `.glide` packages: a folder holding the editable project and the
//! untouched recording.
//!
//! ```text
//! Name.glide/
//!   project.json        style, timeline, export settings
//!   recording/
//!     meta.json         what was recorded
//!     screen.mov        screen video (no cursor) + audio tracks
//!     camera.mov        camera video, if recorded
//!     mouse.json        mouse events
//!     keys.json         key presses
//!     cursors.json      cursor images (PNG files in cursors/)
//!     transcript.json   captions, once transcribed
//! ```

pub mod model;
pub mod presets;
pub mod recording;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::engine::zoom;
use model::{DeviceFrame, PROJECT_VERSION, Project, Slice, Style};
use recording::{Recording, RecordingMeta, SourceKind};

pub const EXTENSION: &str = "glide";

/// `project.json` has a format version newer than this Glide understands.
#[derive(Debug)]
pub struct NewerProject {
    pub found: u64,
}

impl std::fmt::Display for NewerProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "this project was saved by a newer Glide (project format {}, this version reads up to {PROJECT_VERSION}); update Glide to open it",
            self.found
        )
    }
}

impl std::error::Error for NewerProject {}

#[derive(Clone, Debug)]
pub struct Package {
    pub root: PathBuf,
}

impl Package {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Package { root: root.into() }
    }
    pub fn project_file(&self) -> PathBuf {
        self.root.join("project.json")
    }
    pub fn recording_dir(&self) -> PathBuf {
        self.root.join("recording")
    }
    pub fn rec(&self, name: &str) -> PathBuf {
        self.recording_dir().join(name)
    }
    pub fn name(&self) -> String {
        self.root.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn load_recording(&self) -> Result<Recording> {
        let meta: RecordingMeta = read_json(&self.rec("meta.json"))?;
        Ok(Recording {
            meta,
            mouse: read_json_or_default(&self.rec("mouse.json"))?,
            keys: read_json_or_default(&self.rec("keys.json"))?,
            cursors: read_json_or_default(&self.rec("cursors.json"))?,
            transcript: read_json_or_default(&self.rec("transcript.json"))?,
        })
    }

    /// Fails with [`NewerProject`] when `project.json` was saved by a newer Glide, since saving it
    /// again from this version would drop what this version doesn't know about.
    pub fn load_project(&self) -> Result<Project> {
        let path = self.project_file();
        let value: serde_json::Value = read_json(&path)?;
        if let Some(found) = value.get("version").and_then(|v| v.as_u64()).filter(|v| *v > PROJECT_VERSION as u64) {
            return Err(NewerProject { found }.into());
        }
        let mut project: Project = serde_json::from_value(value).with_context(|| format!("parsing {}", path.display()))?;
        project.export = project.export.sanitized();
        Ok(project)
    }

    /// A package with no `project.json` yet gets a fresh project. A `project.json` that exists but
    /// can't be read is an error, and the file stays exactly as it is, so nothing saves a fresh
    /// project over it. A copy goes beside it first and the error names that copy. One from a
    /// newer Glide fails with [`NewerProject`].
    pub fn load(&self) -> Result<(Project, Recording)> {
        let rec = self.load_recording()?;
        if !self.project_file().exists() {
            return Ok((new_project(&self.name(), &rec), rec));
        }
        match self.load_project() {
            Ok(project) => Ok((project, rec)),
            Err(e) if e.is::<NewerProject>() => Err(e),
            Err(e) => {
                let kept = match self.keep_unreadable_project() {
                    Some(copy) => format!(" and kept a copy as {}", copy.display()),
                    None => String::new(),
                };
                Err(anyhow::anyhow!("{} couldn't be read ({}). Glide left it as it is{kept}", self.project_file().display(), e.root_cause()))
            }
        }
    }

    /// Copies `project.json` to `project.json.broken` (`-2`, `-3`… when that holds different
    /// content) unless an identical copy is already there. Returns the copy's path, or None when
    /// no copy could be made.
    fn keep_unreadable_project(&self) -> Option<PathBuf> {
        let bytes = fs::read(self.project_file()).ok()?;
        for n in 1..100 {
            let name = if n == 1 { "project.json.broken".to_string() } else { format!("project.json.broken-{n}") };
            let copy = self.root.join(name);
            match fs::read(&copy) {
                Ok(existing) if existing == bytes => return Some(copy),
                Ok(_) => continue,
                Err(_) => return fs::write(&copy, &bytes).ok().map(|_| copy),
            }
        }
        None
    }

    /// Copy a file into the package's `assets/` folder; returns the path
    /// relative to the package, so projects stay self-contained.
    pub fn import_asset(&self, src: &Path, stem: &str) -> Result<String> {
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase();
        let dir = self.root.join("assets");
        std::fs::create_dir_all(&dir)?;
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let name = format!("{stem}-{ms}.{ext}");
        std::fs::copy(src, dir.join(&name)).with_context(|| format!("copying {}", src.display()))?;
        Ok(format!("assets/{name}"))
    }

    pub fn save_project(&self, p: &Project) -> Result<()> {
        write_json(&self.project_file(), p)
    }
}

/// A fresh project for a recording: one slice covering everything,
/// automatic zooms on clicks, and a style suited to the source.
pub fn new_project(name: &str, rec: &Recording) -> Project {
    let mut p = Project { name: name.into(), ..Default::default() };
    p.id = format!("{:016x}", rec.meta.created_ms as u64 ^ 0x9e37_79b9_7f4a_7c15);
    p.created_ms = rec.meta.created_ms;
    p.updated_ms = rec.meta.created_ms;
    if rec.meta.kind == SourceKind::Display {
        // Full-screen recordings sit on the background too (a little less
        // padding than windows), so the background settings show from the
        // start; set Padding to 0 % for an edge-to-edge video.
        p.style.padding = Style::REVEAL_PADDING;
    }
    if rec.meta.kind == SourceKind::Device {
        // iPhone/iPad screens get a device frame; there's no cursor to follow.
        let (w, h) = (rec.meta.screen.width, rec.meta.screen.height);
        p.style.device = if h > w { DeviceFrame::Phone } else { DeviceFrame::Tablet };
        p.style.padding = 8.0;
    }
    let id = p.timeline.new_id();
    p.timeline.slices.push(Slice { id, start: 0.0, end: rec.meta.duration_ms, ..Default::default() });
    let events = crate::engine::events::Events::new(rec.mouse.clone());
    let zooms = zoom::auto_zooms(&events.downs, rec.meta.duration_ms, p.style.default_zoom, &mut p.timeline);
    p.timeline.zooms = zooms;
    p
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if path.exists() { read_json(path) } else { Ok(T::default()) }
}

/// Write JSON atomically (temp file + rename) so a crash never leaves a
/// half-written project.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(value)?;
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

/// Default folder for new projects.
pub fn projects_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Movies").join("Glide")
}

/// A new, unused package path like `~/Movies/Glide/Recording 2026-10-11 at 11.30.12.glide`.
pub fn new_package_path(dir: &Path, stamp: &str) -> PathBuf {
    let base = format!("Recording {stamp}");
    let mut path = dir.join(format!("{base}.{EXTENSION}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base} {n}.{EXTENSION}"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::ExportSettings;
    use recording::{MouseEvent, MouseKind};

    #[test]
    fn new_project_has_slice_and_zooms() {
        let rec = Recording {
            meta: RecordingMeta { duration_ms: 10_000.0, kind: SourceKind::Window, ..Default::default() },
            mouse: vec![MouseEvent { t: 2000.0, x: 1.0, y: 1.0, kind: MouseKind::Down, button: 0, cursor: 0 }],
            ..Default::default()
        };
        let p = new_project("x", &rec);
        assert_eq!(p.timeline.slices.len(), 1);
        assert_eq!(p.timeline.zooms.len(), 1);
        assert_eq!(p.style.padding, 10.0);
    }

    #[test]
    fn every_kind_of_recording_starts_with_the_background_in_view() {
        for kind in [SourceKind::Display, SourceKind::Window, SourceKind::Area, SourceKind::Device] {
            let rec = Recording { meta: RecordingMeta { duration_ms: 1000.0, kind, ..Default::default() }, ..Default::default() };
            let p = new_project("x", &rec);
            assert!(p.style.padding > 0.0, "{kind:?}: padding {}", p.style.padding);
            assert!(p.style.shadow.intensity > 0.0, "{kind:?}");
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("glide-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("recording")).unwrap();
        let pkg = Package::new(&dir);
        write_json(&pkg.rec("meta.json"), &RecordingMeta { duration_ms: 1000.0, ..Default::default() }).unwrap();
        let (p, rec) = pkg.load().unwrap();
        pkg.save_project(&p).unwrap();
        assert_eq!(pkg.load_project().unwrap(), p);
        assert_eq!(rec.meta.duration_ms, 1000.0);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A package holding only a recording, in a folder of its own.
    fn package(name: &str) -> Package {
        let dir = std::env::temp_dir().join(format!("glide-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("recording")).unwrap();
        let pkg = Package::new(&dir);
        write_json(&pkg.rec("meta.json"), &RecordingMeta { duration_ms: 1000.0, ..Default::default() }).unwrap();
        pkg
    }

    #[test]
    fn importing_music_copies_it_into_assets_and_returns_a_package_relative_path() {
        let pkg = package("music");
        let src_dir = pkg.root.join("elsewhere");
        fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("song.MP3");
        fs::write(&src, b"not really an mp3").unwrap();
        let rel = pkg.import_asset(&src, "music").unwrap();
        assert!(rel.starts_with("assets/music-") && rel.ends_with(".mp3"), "{rel}");
        assert!(Path::new(&rel).is_relative());
        assert_eq!(fs::read(pkg.root.join(&rel)).unwrap(), b"not really an mp3");
        fs::remove_dir_all(&pkg.root).unwrap();
    }
    #[test]
    fn a_package_without_a_project_gets_a_fresh_one_and_no_copy() {
        let pkg = package("missing");
        let (p, _) = pkg.load().unwrap();
        assert_eq!(p.timeline.slices.len(), 1);
        assert!(!pkg.root.join("project.json.broken").exists());
        fs::remove_dir_all(&pkg.root).unwrap();
    }

    #[test]
    fn a_truncated_project_fails_to_load_and_stays_as_it_is() {
        let pkg = package("corrupt");
        let broken = br#"{"name":"my edit","style":{"padding":"#;
        fs::write(pkg.project_file(), broken).unwrap();
        let err = pkg.load().unwrap_err();
        assert_eq!(fs::read(pkg.project_file()).unwrap(), broken, "project.json is not rewritten");
        assert_eq!(fs::read(pkg.root.join("project.json.broken")).unwrap(), broken);
        let msg = format!("{err:#}");
        assert!(msg.contains("project.json.broken"), "the message names the copy: {msg}");
        assert!(err.downcast_ref::<NewerProject>().is_none());
        fs::remove_dir_all(&pkg.root).unwrap();
    }

    #[test]
    fn a_project_with_an_unknown_variant_fails_to_load_and_stays_as_it_is() {
        let pkg = package("unknown-variant");
        let unknown = br#"{"style":{"background":{"type":"video"}}}"#;
        fs::write(pkg.project_file(), unknown).unwrap();
        let err = pkg.load().unwrap_err();
        assert_eq!(fs::read(pkg.project_file()).unwrap(), unknown);
        assert!(format!("{err:#}").contains("project.json"), "{err:#}");
        fs::remove_dir_all(&pkg.root).unwrap();
    }

    #[test]
    fn loading_the_same_unreadable_project_again_adds_no_second_copy() {
        let pkg = package("corrupt-twice");
        fs::write(pkg.project_file(), b"not json").unwrap();
        assert!(pkg.load().is_err());
        assert!(pkg.load().is_err());
        assert!(!pkg.root.join("project.json.broken-2").exists());
        // A different broken file doesn't overwrite the first copy.
        fs::write(pkg.project_file(), b"also not json").unwrap();
        assert!(pkg.load().is_err());
        assert_eq!(fs::read(pkg.root.join("project.json.broken")).unwrap(), b"not json");
        assert_eq!(fs::read(pkg.root.join("project.json.broken-2")).unwrap(), b"also not json");
        assert_eq!(fs::read(pkg.project_file()).unwrap(), b"also not json");
        fs::remove_dir_all(&pkg.root).unwrap();
    }

    #[test]
    fn a_project_from_a_newer_glide_is_refused_and_left_alone() {
        let pkg = package("newer");
        let newer = format!(r#"{{"version":{},"name":"from the future"}}"#, PROJECT_VERSION + 1);
        fs::write(pkg.project_file(), &newer).unwrap();
        let err = pkg.load().unwrap_err();
        assert_eq!(err.downcast_ref::<NewerProject>().map(|e| e.found), Some(PROJECT_VERSION as u64 + 1));
        assert!(err.to_string().contains("newer Glide"), "{err}");
        assert_eq!(fs::read_to_string(pkg.project_file()).unwrap(), newer);
        assert!(!pkg.root.join("project.json.broken").exists());
        fs::remove_dir_all(&pkg.root).unwrap();
    }

    #[test]
    fn export_settings_that_would_crash_the_encoder_are_clamped_on_load() {
        let pkg = package("export-limits");
        fs::write(pkg.project_file(), br#"{"export":{"fps":0,"height":999999,"target_mb":0}}"#).unwrap();
        let (p, _) = pkg.load().unwrap();
        assert!(p.export.fps >= ExportSettings::MIN_FPS);
        assert_eq!(p.export.height, ExportSettings::MAX_HEIGHT);
        assert_eq!(p.export.target_mb, None);
        fs::remove_dir_all(&pkg.root).unwrap();
    }
    #[test]
    fn the_current_version_still_loads() {
        let pkg = package("current");
        fs::write(pkg.project_file(), format!(r#"{{"version":{PROJECT_VERSION},"name":"kept"}}"#)).unwrap();
        let (p, _) = pkg.load().unwrap();
        assert_eq!(p.name, "kept");
        fs::remove_dir_all(&pkg.root).unwrap();
    }
}
