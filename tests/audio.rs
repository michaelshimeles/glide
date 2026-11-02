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
fn export_puts_the_mic_and_the_system_sound_on_separate_tracks() {
    use glide::project::recording::AudioTrack;
    let (dir, path) = package("tracks", Some(880.0));
    let pkg = glide::project::Package::new(&path);
    let (mut project, mut recording) = pkg.load().unwrap();
    // A 440 Hz "voice" on the microphone track, alongside the 880 Hz system tone.
    let n = (recording.meta.duration_ms / 1000.0 * glide::capture::mixer::RATE) as usize;
    let voice: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let v = 0.1 * (std::f64::consts::TAU * 440.0 * i as f64 / glide::capture::mixer::RATE).sin() as f32;
            [v, v]
        })
        .collect();
    glide::app::player::write_wav(&pkg.rec("mic.wav"), &voice).unwrap();
    recording.meta.mic = Some(AudioTrack { file: "mic.wav".into(), track: 0 });
    project.style.audio.enhance_mic = false;
    let output = dir.join("out.mp4");
    let settings = ExportSettings { height: 240, fps: 10, quality: Quality::Web, ..Default::default() };
    let job = Job { project, recording, source: Source::Package(path.clone()), settings, output: output.clone() };
    export::run(&job, &mut |_| {}, &AtomicBool::new(false)).unwrap();

    let system = glide::media::read_audio(&output, 0).expect("the export has a system track");
    let mic = glide::media::read_audio(&output, 1).expect("the export has a mic track");
    assert!(glide::media::read_audio(&output, 2).is_err(), "exactly two audio tracks");
    let (sys_sys, sys_mic) = (glide::audiocheck::analyze(&system, 880.0), glide::audiocheck::analyze(&system, 440.0));
    let (mic_sys, mic_mic) = (glide::audiocheck::analyze(&mic, 880.0), glide::audiocheck::analyze(&mic, 440.0));
    assert!(sys_sys.tone_seconds > 1.8 && sys_mic.tone_seconds < 0.2, "system track: {sys_sys:?} {sys_mic:?}");
    assert!(mic_mic.tone_seconds > 1.8 && mic_sys.tone_seconds < 0.2, "mic track: {mic_mic:?} {mic_sys:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
