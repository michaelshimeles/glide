//! A recording's audio reaches the editor and the export: track levels are
//! measured (and silence explained), the playback mix follows mutes while
//! the project is open, and the exported MP4 carries the sound. Playing
//! through the speakers is checked by the GUI play-test in
//! scripts/selftest.sh.

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use glide::app::editor::{Editor, TrackPeaks, silence_note};
use glide::export::{self, Job, Source};
use glide::project::model::{ExportSettings, Quality};
use glide::render::Gpu;

fn package(name: &str, tone: Option<f64>) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("glide-audio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Audio.glide");
    glide::synth::write_package(&path, 2.0, 640, 400).unwrap();
    if let Some(hz) = tone {
        glide::synth::add_tone(&path, hz).unwrap();
    }
    (dir, path)
}

fn wait(mut done: impl FnMut() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn peaks(ed: &Editor) -> TrackPeaks {
    let mut p = None;
    wait(|| {
        p = *ed.peaks.lock().unwrap();
        p.is_some()
    });
    p.unwrap()
}

#[test]
fn track_levels_explain_a_silent_recording() {
    let (dir, path) = package("levels", Some(880.0));
    let ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    let p = peaks(&ed);
    assert!(p.system.is_some_and(|v| (v - 0.1).abs() < 0.01), "{p:?}");
    assert!(p.mic.is_none());
    assert!(!TrackPeaks::silent(p.system));
    drop(ed);

    // Same recording with a silent system track and no microphone.
    let (_, mut rec) = glide::project::Package::new(&path).load().unwrap();
    glide::app::player::write_wav(&glide::project::Package::new(&path).rec("system.wav"), &vec![[0.0; 2]; 96_000]).unwrap();
    let ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    let p = peaks(&ed);
    assert!(TrackPeaks::silent(p.system), "{p:?}");
    assert!(silence_note(&ed.rec, p).contains("microphone was off"));
    rec.meta.system_audio = None;
    assert!(silence_note(&rec, p).contains("no audio"));
    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn playback_mix_follows_mutes_while_open() {
    let (dir, path) = package("mix", Some(880.0));
    let mut ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    let settle = |ed: &mut Editor| {
        wait(|| {
            ed.tick();
            !ed.audio.mixing()
        })
    };
    settle(&mut ed);
    assert!(ed.audio.has_audio(), "the tone is mixed for playback");
    let mut d = ed.project.clone();
    d.style.audio.mute_system = true;
    ed.commit(d);
    settle(&mut ed);
    assert!(!ed.audio.has_audio(), "muted: nothing to play");
    let mut d = ed.project.clone();
    d.style.audio.mute_system = false;
    d.style.audio.volume = 0.5;
    ed.commit(d);
    settle(&mut ed);
    assert!(ed.audio.has_audio(), "unmuted again");
    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_carries_the_recorded_sound() {
    let (dir, path) = package("export", Some(880.0));
    let pkg = glide::project::Package::new(&path);
    let (project, recording) = pkg.load().unwrap();
    let output = dir.join("out.mp4");
    let settings = ExportSettings { height: 240, fps: 10, quality: Quality::Web, ..Default::default() };
    let job = Job { project, recording, source: Source::Package(path.clone()), settings, output: output.clone() };
    export::run(&job, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let audio = glide::media::read_audio(&output, 0).expect("the export has an audio track");
    let level = glide::audiocheck::analyze(&audio, 880.0);
    assert!(level.tone_seconds > 1.8, "{level:?}");
    assert!((level.tone_db + 23.0).abs() < 1.5, "{level:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn package_music_is_mixed_and_a_missing_file_is_reported() {
    use glide::engine::timemap::TimeMap;
    use glide::export::audio::{mix_unmastered, music_missing};
    let (dir, path) = package("music", None);
    let pkg = glide::project::Package::new(&path);
    let (mut project, rec) = pkg.load().unwrap();
    let tone: Vec<[f32; 2]> = (0..96_000).map(|i| { let v = 0.2 * (i as f32 * 0.05).sin(); [v, v] }).collect();
    std::fs::create_dir_all(path.join("assets")).unwrap();
    let file = path.join("assets/music-1.wav");
    glide::app::player::write_wav(&file, &tone).unwrap();
    project.style.audio.click_sounds = false;
    project.style.audio.music = Some("assets/music-1.wav".into());
    let map = TimeMap::new(&project.timeline.slices, rec.meta.duration_ms);
    let peak = |m: &[[f32; 2]]| m.iter().fold(0.0f32, |p, s| p.max(s[0].abs()));
    let relative = mix_unmastered(&project, &rec, Some(&path), &map).expect("the packaged music is mixed");
    assert!(peak(&relative) > 0.001, "peak {}", peak(&relative));
    assert!(!music_missing(Some(&path), &project.style.audio));
    // An older project stored the absolute path; it mixes the same.
    project.style.audio.music = Some(file.to_string_lossy().into_owned());
    let absolute = mix_unmastered(&project, &rec, Some(&path), &map).expect("the absolute path still works");
    assert_eq!(relative, absolute);
    project.style.audio.music = Some("assets/music-1.wav".into());
    std::fs::remove_file(&file).unwrap();
    assert!(music_missing(Some(&path), &project.style.audio));
    assert!(mix_unmastered(&project, &rec, Some(&path), &map).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
