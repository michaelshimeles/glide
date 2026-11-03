//! Finding the hits that effects follow, and remembering them in the
//! package so the audio is only analysed once.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::model::Project;
use super::recording::Recording;
use super::{Package, read_json_or_default, write_json};
use crate::capture::mixer::RATE;
use crate::engine::beats::{self, Beats};

/// What `recording/beats.json` holds: the hits, and which audio they are from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub key: String,
    pub beats: Beats,
}

/// The audio effects follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Wanted {
    /// Names the audio, so a cache from other audio is not used.
    pub key: String,
    music: bool,
    /// The file to read, and which of its audio tracks.
    file: PathBuf,
    track: usize,
}

/// The audio effects follow: the background music if there is some, else
/// the sound the Mac played during the recording (a muted track doesn't
/// count). The microphone is never used, because it is the voice. None when
/// there is neither.
pub fn wanted(project: &Project, rec: &Recording, pkg: &Package) -> Option<Wanted> {
    let audio = &project.style.audio;
    if let Some(music) = &audio.music {
        return Some(Wanted { key: format!("music:{music}"), music: true, file: PathBuf::from(music), track: 0 });
    }
    let sys = rec.meta.system_audio.as_ref().filter(|_| !audio.mute_system)?;
    Some(Wanted { key: format!("system:{}:{}", sys.file, sys.track), music: false, file: pkg.rec(&sys.file), track: sys.track })
}

impl Wanted {
    /// Whether it is the background music (else the recording's own sound).
    pub fn is_music(&self) -> bool {
        self.music
    }
}

/// The hits in `want`: from the package's cache when it is for the same
/// audio, else found by reading the audio (which takes a moment) and saved.
pub fn analyze(pkg: &Package, want: &Wanted) -> Result<Beats> {
    if let Ok(cache) = read_json_or_default::<Cache>(&pkg.rec("beats.json"))
        && cache.key == want.key
    {
        return Ok(cache.beats);
    }
    let samples = crate::media::read_audio(&want.file, want.track).with_context(|| format!("reading {}", want.file.display()))?;
    let found = Beats {
        beats: beats::onsets(&samples, RATE),
        // Music repeats when it is shorter than the edit.
        loop_ms: if want.music { samples.len() as f64 / RATE * 1000.0 } else { 0.0 },
    };
    // A package that can't be written to still gets its beats this time.
    let _ = write_json(&pkg.rec("beats.json"), &Cache { key: want.key.clone(), beats: found.clone() });
    Ok(found)
}

/// Bring `rec.beats` in line with the project's audio, waiting for the
/// analysis if it is needed. For the command line and for exports, where
/// there is nothing to keep responsive.
pub fn ensure(pkg: &Package, project: &Project, rec: &mut Recording) -> Result<()> {
    rec.beats = match wanted(project, rec, pkg) {
        Some(want) if !project.timeline.effects.is_empty() => analyze(pkg, &want)?,
        _ => Beats::default(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::recording::AudioTrack;

    fn pkg() -> Package {
        Package::new("/tmp/Demo.glide")
    }

    fn rec_with_system() -> Recording {
        let mut rec = Recording::default();
        rec.meta.system_audio = Some(AudioTrack { file: "screen.mov".into(), track: 1 });
        rec
    }

    #[test]
    fn music_wins_then_system_audio_and_never_the_microphone() {
        let mut project = Project::default();
        let mut rec = rec_with_system();
        rec.meta.mic = Some(AudioTrack { file: "screen.mov".into(), track: 2 });
        let w = wanted(&project, &rec, &pkg()).unwrap();
        assert_eq!(w.key, "system:screen.mov:1");
        assert_eq!(w.file, PathBuf::from("/tmp/Demo.glide/recording/screen.mov"));
        project.style.audio.music = Some("/Music/beat.mp3".into());
        let w = wanted(&project, &rec, &pkg()).unwrap();
        assert_eq!((w.key.as_str(), w.music), ("music:/Music/beat.mp3", true));
        // Only the microphone: nothing to follow.
        let mut only_mic = Recording::default();
        only_mic.meta.mic = Some(AudioTrack { file: "screen.mov".into(), track: 2 });
        assert!(wanted(&Project::default(), &only_mic, &pkg()).is_none());
    }

    #[test]
    fn muted_system_audio_is_not_followed() {
        let mut project = Project::default();
        project.style.audio.mute_system = true;
        assert!(wanted(&project, &rec_with_system(), &pkg()).is_none());
    }

    #[test]
    fn a_cache_for_the_same_audio_is_used_and_another_is_not() {
        let dir = std::env::temp_dir().join(format!("glide-beats-{}", std::process::id()));
        let pkg = Package::new(dir.join("Demo.glide"));
        std::fs::create_dir_all(pkg.recording_dir()).unwrap();
        let cached = Beats { beats: vec![beats::Beat { t: 500.0, strength: 0.9 }], loop_ms: 0.0 };
        write_json(&pkg.rec("beats.json"), &Cache { key: "system:screen.mov:1".into(), beats: cached.clone() }).unwrap();
        let want = wanted(&Project::default(), &rec_with_system(), &pkg).unwrap();
        // The audio file doesn't exist, so a result can only come from the cache.
        assert_eq!(analyze(&pkg, &want).unwrap(), cached);
        let other = Wanted { key: "system:other.mov:1".into(), ..want };
        assert!(analyze(&pkg, &other).is_err(), "other audio isn't answered from the cache");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_is_analysed_without_effects() {
        let mut rec = rec_with_system();
        rec.beats = Beats { beats: vec![beats::Beat { t: 1.0, strength: 1.0 }], loop_ms: 0.0 };
        ensure(&pkg(), &Project::default(), &mut rec).unwrap();
        assert!(rec.beats.beats.is_empty());
    }
}
