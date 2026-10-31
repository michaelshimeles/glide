//! Export audio: the recorded tracks cut and re-timed to match the edit,
//! with per-slice volumes, background music, click sounds and an optional
//! microphone clean-up.

use std::path::{Path, PathBuf};

use crate::capture::mixer::RATE;
use crate::engine::events::Events;
use crate::engine::timemap::TimeMap;
use crate::project::model::{AudioStyle, Project};
use crate::project::recording::Recording;

pub type Stereo = [f32; 2];

/// Read a track's audio, or nothing if it's missing.
fn load(dir: &Path, track: &Option<crate::project::recording::AudioTrack>) -> Option<Vec<Stereo>> {
    let t = track.as_ref()?;
    crate::media::read_audio(&dir.join(&t.file), t.track).ok().filter(|v| !v.is_empty())
}

/// Peak level of a recorded track (0..1), or None if it wasn't recorded or
/// can't be read.
pub fn track_peak(dir: &Path, track: &Option<crate::project::recording::AudioTrack>) -> Option<f32> {
    let t = track.as_ref()?;
    let buf = crate::media::read_audio(&dir.join(&t.file), t.track).ok()?;
    Some(buf.iter().fold(0.0f32, |m, s| m.max(s[0].abs()).max(s[1].abs())))
}

/// Below this peak a track is silence (AAC-coded digital silence is -91 dBFS).
pub const SILENT_PEAK: f32 = 1e-4;

/// Linear-interpolated sample at fractional index.
pub fn sample(src: &[Stereo], x: f64) -> Stereo {
    if src.is_empty() || x < 0.0 {
        return [0.0, 0.0];
    }
    let i = x as usize;
    if i + 1 >= src.len() {
        return src.get(i).copied().unwrap_or([0.0, 0.0]);
    }
    let f = (x - i as f64) as f32;
    [src[i][0] + (src[i + 1][0] - src[i][0]) * f, src[i][1] + (src[i + 1][1] - src[i][1]) * f]
}

/// Place a source track on the playback timeline following the slices.
fn retime(src: &[Stereo], map: &TimeMap, gain: impl Fn(usize) -> f32, out: &mut [Stereo]) {
    for l in &map.links {
        let g = if l.slice == usize::MAX { 1.0 } else { gain(l.slice) };
        if g <= 0.0 {
            continue;
        }
        let a = (l.pb_start / 1000.0 * RATE) as usize;
        let b = ((l.pb_end / 1000.0 * RATE) as usize).min(out.len());
        for (j, o) in out.iter_mut().enumerate().take(b).skip(a) {
            let pb_ms = j as f64 / RATE * 1000.0;
            let src_ms = l.src_start + (pb_ms - l.pb_start) * l.speed;
            let s = sample(src, src_ms / 1000.0 * RATE);
            o[0] += s[0] * g;
            o[1] += s[1] * g;
        }
    }
}

/// Gentle speech clean-up: high-pass, soft noise gate, level to ~-18 dBFS RMS.
pub fn enhance_voice(buf: &mut [Stereo]) {
    // One-pole high-pass at ~90 Hz.
    let rc = 1.0 / (2.0 * std::f32::consts::PI * 90.0);
    let dt = 1.0 / RATE as f32;
    let alpha = rc / (rc + dt);
    let (mut px, mut py) = ([0.0f32; 2], [0.0f32; 2]);
    for s in buf.iter_mut() {
        for c in 0..2 {
            let y = alpha * (py[c] + s[c] - px[c]);
            px[c] = s[c];
            py[c] = y;
            s[c] = y;
        }
    }
    // Soft gate on a 10 ms envelope.
    let win = (RATE * 0.01) as usize;
    let mut env = 0.0f32;
    let mut gain = 1.0f32;
    let floor = 0.006;
    for (i, s) in buf.iter_mut().enumerate() {
        let lvl = s[0].abs().max(s[1].abs());
        env = env.max(lvl) * 0.999 + lvl * 0.001;
        if i % win == 0 {
            let target = if env > floor { 1.0 } else { (env / floor).powi(2).max(0.05) };
            gain += (target - gain) * if target > gain { 0.6 } else { 0.08 };
        }
        s[0] *= gain;
        s[1] *= gain;
    }
    level_voice(buf, floor);
}

/// Level speech to about -18 dBFS RMS on voiced parts, never above 4x, then
/// even out quieter and louder passages with `agc`.
fn level_voice(buf: &mut [Stereo], floor: f32) {
    let voiced: Vec<f32> = buf.iter().map(|s| (s[0] * s[0] + s[1] * s[1]) / 2.0).filter(|v| *v > floor * floor).collect();
    if !voiced.is_empty() {
        let rms = (voiced.iter().sum::<f32>() / voiced.len() as f32).sqrt();
        let g = (0.125 / rms.max(1e-6)).min(4.0);
        for s in buf.iter_mut() {
            s[0] *= g;
            s[1] *= g;
        }
    }
    agc(buf, floor);
}

/// Automatic gain control: a slow gain (±9 dB) that pulls each passage of
/// speech toward the target level. Measured on 50 ms windows; gain only
/// moves while someone is speaking (rises over ~1 s, falls over ~0.3 s), so
/// pauses aren't pumped up.
pub fn agc(buf: &mut [Stereo], floor: f32) {
    const TARGET: f32 = 0.125;
    let win = (RATE * 0.05) as usize;
    if buf.len() < win {
        return;
    }
    let mut gains = Vec::with_capacity(buf.len() / win + 1);
    let mut gain = 1.0f32;
    let mut level = TARGET;
    for chunk in buf.chunks(win) {
        let rms = (chunk.iter().map(|s| (s[0] * s[0] + s[1] * s[1]) * 0.5).sum::<f32>() / chunk.len() as f32).sqrt();
        if rms > floor {
            // Smoothed speech level (~0.5 s), then the gain that would hit the target.
            level += (rms - level) * 0.1;
            let want = (TARGET / level.max(1e-6)).clamp(0.35, 2.8);
            let rate = if want < gain { 0.15 } else { 0.05 };
            gain += (want - gain) * rate;
        }
        gains.push(gain);
    }
    // Apply with linear interpolation between window centres.
    for (i, s) in buf.iter_mut().enumerate() {
        let x = i as f32 / win as f32 - 0.5;
        let j = (x.max(0.0) as usize).min(gains.len() - 1);
        let k = (j + 1).min(gains.len() - 1);
        let f = (x - j as f32).clamp(0.0, 1.0);
        let g = gains[j] + (gains[k] - gains[j]) * f;
        s[0] *= g;
        s[1] *= g;
    }
}

/// `clean_voice` for a file, cached next to the recording as raw f32 stereo
/// (`<package>/cache/`), so preview rebuilds after edits don't redo it.
fn cleaned(dir: &Path, file: &str, track: usize, raw: Vec<Stereo>) -> Vec<Stereo> {
    let src = dir.join("recording").join(file);
    // Bump when clean_voice changes so old caches aren't reused.
    const VERSION: u32 = 2;
    let cache = dir.join("cache").join(format!("voice-v{VERSION}-{file}-{track}.f32"));
    let fresh = |c: &Path| {
        let (Ok(a), Ok(b)) = (std::fs::metadata(c), std::fs::metadata(&src)) else { return false };
        a.modified().ok() >= b.modified().ok() && a.len() == (raw.len() * 8) as u64
    };
    if fresh(&cache)
        && let Ok(bytes) = std::fs::read(&cache)
    {
        return bytes.chunks_exact(8).map(|c| [f32::from_le_bytes(c[0..4].try_into().unwrap()), f32::from_le_bytes(c[4..8].try_into().unwrap())]).collect();
    }
    let mut buf = raw;
    clean_voice(&mut buf);
    let mut bytes = Vec::with_capacity(buf.len() * 8);
    for s in &buf {
        bytes.extend_from_slice(&s[0].to_le_bytes());
        bytes.extend_from_slice(&s[1].to_le_bytes());
    }
    if std::fs::create_dir_all(cache.parent().unwrap()).is_ok() {
        // Drop caches of this track made by older versions.
        let suffix = format!("-{file}-{track}.f32");
        for e in std::fs::read_dir(cache.parent().unwrap()).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("voice-") && name.ends_with(&suffix) && e.path() != cache {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let tmp = cache.with_extension("tmp");
        if std::fs::write(&tmp, &bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &cache);
        }
    }
    buf
}

/// "Enhance voice": Apple's on-device voice isolation (removes background
/// noise and room tone) plus levelling; the simple DSP chain if isolation
/// isn't available.
pub fn clean_voice(buf: &mut Vec<Stereo>) {
    match super::isolate::isolate_voice(buf) {
        Some(isolated) => {
            *buf = isolated;
            level_voice(buf, 0.006);
        }
        None => enhance_voice(buf),
    }
}

/// A short synthetic mouse click (two damped noise ticks).
fn click_sound() -> Vec<Stereo> {
    let n = (RATE * 0.045) as usize;
    let mut seed = 0x1234_5678u32;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let t = i as f32 / RATE as f32;
            let tick = (-t * 900.0).exp() + 0.6 * (-(t - 0.025).max(0.0) * 1100.0).exp() * if t > 0.025 { 1.0 } else { 0.0 };
            let v = noise * tick * 0.5 + (t * 2.0 * std::f32::consts::PI * 2400.0).sin() * tick * 0.25;
            [v, v]
        })
        .collect()
}

pub fn soft_clip(x: f32) -> f32 {
    if x.abs() < 0.9 { x } else { x.signum() * (0.9 + 0.1 * ((x.abs() - 0.9) / 0.1).tanh()) }
}

/// Gain envelope for the recorded audio: `level` while any voice-over plays
/// (with 150 ms ramps), 1 elsewhere.
pub fn duck_envelope(len: usize, spans: &[(usize, usize)], level: f32) -> Vec<f32> {
    let mut env = vec![1.0f32; len];
    let ramp = (RATE * 0.15) as usize;
    for &(a, b) in spans {
        let lo = a.saturating_sub(ramp);
        let hi = (b + ramp).min(len);
        for (j, e) in env.iter_mut().enumerate().take(hi).skip(lo) {
            let t = if j < a {
                (a - j) as f32 / ramp as f32
            } else if j >= b {
                (j - b) as f32 / ramp as f32
            } else {
                0.0
            };
            let g = level + (1.0 - level) * t.clamp(0.0, 1.0);
            *e = e.min(g);
        }
    }
    env
}

/// Where the music file is. Older projects hold an absolute path, which is used as is.
pub fn music_path(dir: Option<&Path>, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    match dir {
        Some(d) if p.is_relative() => d.join(p),
        _ => p.to_path_buf(),
    }
}
/// Music is set but its file is gone (or isn't a file).
pub fn music_missing(dir: Option<&Path>, style: &AudioStyle) -> bool {
    style.music.as_ref().is_some_and(|m| !music_path(dir, m).is_file())
}
/// Build the export soundtrack. Returns None when there's nothing to hear.
pub fn mix(project: &Project, rec: &Recording, dir: Option<&Path>, map: &TimeMap) -> Option<Vec<Stereo>> {
    let mut out = mix_unmastered(project, rec, dir, map)?;
    let v = project.style.audio.volume as f32;
    for o in out.iter_mut() {
        o[0] = soft_clip(o[0] * v);
        o[1] = soft_clip(o[1] * v);
    }
    Some(out)
}

/// The soundtrack before the master volume and limiter (the editor's
/// playback applies those live). None when nothing is audible.
pub fn mix_unmastered(project: &Project, rec: &Recording, dir: Option<&Path>, map: &TimeMap) -> Option<Vec<Stereo>> {
    let style = &project.style.audio;
    let len = (map.duration() / 1000.0 * RATE).ceil() as usize;
    let mut out = vec![[0.0f32; 2]; len];
    let mut any = false;
    let slices = &project.timeline.slices;
    // Voice-overs, placed at their playback position at normal speed.
    let mut voice = vec![[0.0f32; 2]; len];
    let mut spans = vec![];
    if let Some(dir) = dir {
        let rd = dir.join("recording");
        if !style.mute_system
            && let Some(sys) = load(&rd, &rec.meta.system_audio)
        {
            retime(&sys, map, |i| slices.get(i).map_or(1.0, |s| s.system_volume as f32), &mut out);
            any = true;
        }
        if !style.mute_mic
            && let Some(mut mic) = load(&rd, &rec.meta.mic)
        {
            if style.enhance_mic
                && let Some(t) = &rec.meta.mic
            {
                mic = cleaned(dir, &t.file, t.track, mic);
            }
            retime(&mic, map, |i| slices.get(i).map_or(1.0, |s| s.mic_volume as f32), &mut out);
            any = true;
        }
        for vo in &project.timeline.voiceovers {
            let Some(mut buf) = crate::media::read_audio(&rd.join(&vo.file), 0).ok().filter(|b| !b.is_empty()) else { continue };
            if style.enhance_mic {
                buf = cleaned(dir, &vo.file, 0, buf);
            }
            let at = (map.to_playback_nearest(vo.start) / 1000.0 * RATE) as usize;
            let g = vo.volume as f32;
            for (k, s) in buf.iter().enumerate() {
                if let Some(o) = voice.get_mut(at + k) {
                    o[0] += s[0] * g;
                    o[1] += s[1] * g;
                }
            }
            spans.push((at.min(len), (at + buf.len()).min(len)));
            any = true;
        }
    }
    if style.duck_under_voiceover && !spans.is_empty() {
        let env = duck_envelope(len, &spans, style.duck_level as f32);
        for (o, e) in out.iter_mut().zip(env) {
            o[0] *= e;
            o[1] *= e;
        }
    }
    for (o, v) in out.iter_mut().zip(&voice) {
        o[0] += v[0];
        o[1] += v[1];
    }
    if let Some(music) = style.music.as_ref().and_then(|p| crate::media::read_audio(&music_path(dir, p), 0).ok()).filter(|m| !m.is_empty()) {
        let g = style.music_volume as f32;
        // Fade the music out over the last second.
        let n = out.len();
        let fade = (RATE as usize).min(n).max(1);
        for (j, o) in out.iter_mut().enumerate() {
            let f = if j + fade > n { (n - j) as f32 / fade as f32 } else { 1.0 };
            let s = music[j % music.len()];
            o[0] += s[0] * g * f;
            o[1] += s[1] * g * f;
        }
        any = true;
    }
    if style.click_sounds {
        let click = click_sound();
        let g = style.click_volume as f32;
        for d in Events::new(rec.mouse.clone()).downs {
            if let Some(pb) = map.to_playback(d.t) {
                let at = (pb / 1000.0 * RATE) as usize;
                for (k, s) in click.iter().enumerate() {
                    if let Some(o) = out.get_mut(at + k) {
                        o[0] += s[0] * g;
                        o[1] += s[1] * g;
                    }
                }
            }
        }
        any = true;
    }
    any.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::model::Slice;

    #[test]
    fn a_relative_music_path_resolves_inside_the_package() {
        let dir = Path::new("/pkg/Demo.glide");
        assert_eq!(music_path(Some(dir), "assets/music-1.mp3"), dir.join("assets/music-1.mp3"));
        assert_eq!(music_path(None, "assets/music-1.mp3"), Path::new("assets/music-1.mp3"));
    }
    #[test]
    fn an_old_absolute_music_path_resolves_as_it_is() {
        let dir = Path::new("/pkg/Demo.glide");
        assert_eq!(music_path(Some(dir), "/Users/me/Music/song.mp3"), Path::new("/Users/me/Music/song.mp3"));
    }
    #[test]
    fn music_missing_is_true_for_an_absent_file_or_a_folder_and_false_for_a_file_or_no_music() {
        let root = std::env::temp_dir().join(format!("glide-music-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets/folder")).unwrap();
        std::fs::write(root.join("assets/song.wav"), b"x").unwrap();
        let with = |m: Option<&str>| AudioStyle { music: m.map(String::from), ..Default::default() };
        let dir = Some(root.as_path());
        assert!(!music_missing(dir, &with(None)));
        assert!(!music_missing(dir, &with(Some("assets/song.wav"))));
        assert!(!music_missing(None, &with(Some(root.join("assets/song.wav").to_str().unwrap()))));
        assert!(music_missing(dir, &with(Some("assets/gone.wav"))));
        assert!(music_missing(dir, &with(Some("assets/folder"))));
        let _ = std::fs::remove_dir_all(&root);
    }
    #[test]
    fn retime_follows_cuts_and_speed() {
        // 2 s ramp at 48 kHz; keep 0..0.5 s and 1..2 s at 2x.
        let src: Vec<Stereo> = (0..96_000).map(|i| [i as f32, 0.0]).collect();
        let slices = [
            Slice { start: 0.0, end: 500.0, ..Default::default() },
            Slice { start: 1000.0, end: 2000.0, speed: 2.0, ..Default::default() },
        ];
        let map = TimeMap::new(&slices, 2000.0);
        let mut out = vec![[0.0; 2]; (map.duration() / 1000.0 * RATE) as usize];
        retime(&src, &map, |_| 1.0, &mut out);
        assert_eq!(out.len(), 48_000);
        assert!((out[100][0] - 100.0).abs() < 1.0);
        // First sample of the second slice is source 1.0 s.
        assert!((out[24_000][0] - 48_000.0).abs() < 2.0);
        // Then it advances two source samples per output sample.
        assert!((out[24_010][0] - 48_020.0).abs() < 2.0);
    }

    #[test]
    fn agc_evens_out_quiet_and_loud_passages() {
        // 3 s of quiet "speech" then 3 s loud (a modulated tone), 12 dB apart.
        let n = (RATE * 3.0) as usize;
        let tone = |i: usize, a: f32| {
            let t = i as f32 / RATE as f32;
            a * (t * 220.0 * std::f32::consts::TAU).sin() * (0.6 + 0.4 * (t * 3.0 * std::f32::consts::TAU).sin())
        };
        let mut buf: Vec<Stereo> = (0..2 * n).map(|i| { let v = tone(i, if i < n { 0.05 } else { 0.2 }); [v, v] }).collect();
        let rms = |b: &[Stereo]| (b.iter().map(|s| s[0] * s[0]).sum::<f32>() / b.len() as f32).sqrt();
        let before = 20.0 * (rms(&buf[n..]) / rms(&buf[n / 2..n])).log10();
        agc(&mut buf, 0.006);
        // Compare the settled second halves of each passage.
        let after = 20.0 * (rms(&buf[n + n / 2..]) / rms(&buf[n / 2..n])).log10();
        assert!(before > 11.0 && after < before - 4.0, "loud/quiet gap {before:.1} dB -> {after:.1} dB");
    }

    #[test]
    fn duck_envelope_ramps_around_spans() {
        let r = RATE as usize;
        let env = duck_envelope(4 * r, &[(r, 2 * r)], 0.2);
        assert_eq!(env[0], 1.0);
        assert!((env[r + 10] - 0.2).abs() < 1e-6);
        assert!(env[r - (RATE * 0.075) as usize] > 0.2 && env[r - (RATE * 0.075) as usize] < 1.0);
        assert_eq!(env[3 * r], 1.0);
    }

    #[test]
    fn enhance_keeps_level_sane() {
        let mut buf: Vec<Stereo> = (0..48_000).map(|i| {
            let v = (i as f32 * 0.05).sin() * 0.02;
            [v, v]
        }).collect();
        enhance_voice(&mut buf);
        let peak = buf.iter().fold(0.0f32, |m, s| m.max(s[0].abs()));
        assert!(peak > 0.02 && peak <= 0.2, "{peak}");
    }
}
