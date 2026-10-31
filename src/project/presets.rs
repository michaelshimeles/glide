//! Style presets: a project's `Style` saved by name to
//! `~/Library/Application Support/Glide/presets/<name>.json`, applied to any
//! project, and optionally used as the default for new recordings.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use super::model::{Background, Style};
use super::{read_json, write_json};

pub fn dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/Glide/presets")
}

fn default_marker(dir: &Path) -> PathBuf {
    dir.join(".default")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub path: PathBuf,
}

/// Presets in `dir`, sorted by name.
pub fn list_in(dir: &Path) -> Vec<Preset> {
    let mut out: Vec<Preset> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "json" {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(Preset { name, path })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn list() -> Vec<Preset> {
    list_in(&dir())
}

fn sanitize(name: &str) -> Result<String> {
    let n: String = name.trim().chars().map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c }).collect();
    if n.is_empty() || n.starts_with('.') {
        bail!("preset names can't be empty or start with a dot");
    }
    Ok(n)
}

/// Save `style` as a preset. A background image or music file stored inside the
/// package (`package_dir`) is copied next to the preset so it works in any project.
pub fn save_in(dir: &Path, name: &str, style: &Style, package_dir: Option<&Path>) -> Result<Preset> {
    let name = sanitize(name)?;
    std::fs::create_dir_all(dir)?;
    let mut style = style.clone();
    if let Background::Image { path } = &mut style.background
        && Path::new(path.as_str()).is_relative()
        && let Some(pkg) = package_dir
    {
        let src = pkg.join(path.as_str());
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_string();
        let dst = dir.join("assets").join(format!("{name}.{ext}"));
        std::fs::create_dir_all(dst.parent().unwrap())?;
        std::fs::copy(&src, &dst)?;
        *path = dst.to_string_lossy().into_owned();
    }
    if let Some(music) = &mut style.audio.music
        && Path::new(music.as_str()).is_relative()
        && let Some(pkg) = package_dir
    {
        let src = pkg.join(music.as_str());
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_string();
        let dst = dir.join("assets").join(format!("{name}-music.{ext}"));
        std::fs::create_dir_all(dst.parent().unwrap())?;
        std::fs::copy(&src, &dst)?;
        *music = dst.to_string_lossy().into_owned();
    }
    let path = dir.join(format!("{name}.json"));
    write_json(&path, &style)?;
    Ok(Preset { name, path })
}

pub fn save(name: &str, style: &Style, package_dir: Option<&Path>) -> Result<Preset> {
    save_in(&dir(), name, style, package_dir)
}

pub fn load(p: &Preset) -> Result<Style> {
    read_json(&p.path)
}

pub fn delete(p: &Preset) -> Result<()> {
    if default_name_in(p.path.parent().unwrap_or(Path::new("."))).as_deref() == Some(p.name.as_str()) {
        let _ = std::fs::remove_file(default_marker(p.path.parent().unwrap()));
    }
    std::fs::remove_file(&p.path)?;
    Ok(())
}

pub fn default_name_in(dir: &Path) -> Option<String> {
    let n = std::fs::read_to_string(default_marker(dir)).ok()?;
    let n = n.trim().to_string();
    dir.join(format!("{n}.json")).exists().then_some(n)
}

/// Mark a preset as the style for new recordings (None clears it).
pub fn set_default_in(dir: &Path, name: Option<&str>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    match name {
        Some(n) => std::fs::write(default_marker(dir), n)?,
        None => {
            let _ = std::fs::remove_file(default_marker(dir));
        }
    }
    Ok(())
}

pub fn default_name() -> Option<String> {
    default_name_in(&dir())
}

pub fn set_default(name: Option<&str>) -> Result<()> {
    set_default_in(&dir(), name)
}

/// The default preset's style, if one is set.
pub fn default_style() -> Option<Style> {
    let d = dir();
    let name = default_name_in(&d)?;
    read_json(&d.join(format!("{name}.json"))).ok()
}

/// Apply a preset to a project's style, keeping what belongs to the
/// recording rather than the look: crop and camera position.
pub fn apply(style: &mut Style, preset: &Style) {
    let crop = style.crop;
    let camera_position = style.camera.position;
    *style = preset.clone();
    style.crop = crop;
    style.camera.position = camera_position;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_default_and_delete() {
        let dir = std::env::temp_dir().join(format!("glide-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut style = Style::default();
        style.padding = 12.5;
        let p = save_in(&dir, "Launch video", &style, None).unwrap();
        save_in(&dir, "a/b", &Style::default(), None).unwrap();
        let names: Vec<_> = list_in(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["a-b", "Launch video"]);
        assert_eq!(load(&p).unwrap().padding, 12.5);
        set_default_in(&dir, Some("Launch video")).unwrap();
        assert_eq!(default_name_in(&dir).as_deref(), Some("Launch video"));
        delete(&p).unwrap();
        assert_eq!(default_name_in(&dir), None);
        assert!(save_in(&dir, "  ", &style, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relative_music_file_is_copied_next_to_the_preset() {
        let root = std::env::temp_dir().join(format!("glide-preset-music-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (pkg, presets) = (root.join("Demo.glide"), root.join("presets"));
        std::fs::create_dir_all(pkg.join("assets")).unwrap();
        std::fs::write(pkg.join("assets/music-1.mp3"), b"tune").unwrap();
        let mut style = Style::default();
        style.audio.music = Some("assets/music-1.mp3".into());
        let preset = save_in(&presets, "Calm", &style, Some(&pkg)).unwrap();
        let saved = load(&preset).unwrap().audio.music.unwrap();
        assert_eq!(Path::new(&saved), presets.join("assets/Calm-music.mp3"));
        assert_eq!(std::fs::read(&saved).unwrap(), b"tune");
        assert_eq!(style.audio.music.as_deref(), Some("assets/music-1.mp3"), "the project keeps its relative path");
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn apply_keeps_crop_and_camera_position() {
        let mut style = Style::default();
        style.crop.x = 0.1;
        style.camera.position.x = 0.2;
        let mut preset = Style::default();
        preset.padding = 30.0;
        apply(&mut style, &preset);
        assert_eq!(style.padding, 30.0);
        assert_eq!(style.crop.x, 0.1);
        assert_eq!(style.camera.position.x, 0.2);
    }
}
