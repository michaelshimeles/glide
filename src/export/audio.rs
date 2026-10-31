//! Export audio: the recorded tracks cut and re-timed to match the edit,
//! with per-slice volumes, background music, click sounds and an optional
//! microphone clean-up.

use std::path::Path;

use crate::capture::mixer::RATE;
use crate::engine::events::Events;
use crate::engine::timemap::TimeMap;
use crate::project::model::{ClickSound, KeySound, Project};
use crate::project::recording::{Recording, mods};

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

/// A short synthetic percussive sound: a band-passed noise burst and a
/// gliding tone, decaying, with an optional second tick (a key release, a
/// lever) and an optional ring. Everything is generated, so exports carry no
/// audio assets and repeated exports are identical.
#[derive(Clone, Copy)]
struct Tick {
    len_ms: f32,
    /// Noise burst level and the band-pass corners, Hz.
    noise: f32,
    band_lo: f32,
    band_hi: f32,
    /// Tonal body: level, frequency and its end (a glide over `glide_ms`).
    tone: f32,
    tone_hz: f32,
    tone_hz2: f32,
    glide_ms: f32,
    /// Decay rate (1/s) of the burst and body.
    decay: f32,
    /// Second tick: level, delay (ms) and decay.
    second: f32,
    second_ms: f32,
    second_decay: f32,
    /// A metallic ring (level, Hz, decay), for typewriter keys.
    ring: f32,
    ring_hz: f32,
    ring_decay: f32,
}

impl Tick {
    fn render(&self, pitch: f32, seed: u32) -> Vec<Stereo> {
        let n = (RATE * self.len_ms as f64 / 1000.0) as usize;
        let mut out = Vec::with_capacity(n);
        let mut rng = seed | 1;
        let mut lp = [0.0f32; 2];
        let a = |f: f32| 1.0 - (-std::f32::consts::TAU * f * pitch / RATE as f32).exp();
        let (a1, a2) = (a(self.band_hi), a(self.band_lo));
        let (mut phase, mut ring_phase) = (0.0f32, 0.0f32);
        let second_at = self.second_ms / 1000.0;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let white = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
            lp[0] += (white - lp[0]) * a1;
            lp[1] += (white - lp[1]) * a2;
            let env = (-t * self.decay).exp() + if t >= second_at { self.second * (-(t - second_at) * self.second_decay).exp() } else { 0.0 };
            let glide = (t * 1000.0 / self.glide_ms.max(0.01)).min(1.0);
            phase += std::f32::consts::TAU * (self.tone_hz + (self.tone_hz2 - self.tone_hz) * glide) * pitch / RATE as f32;
            ring_phase += std::f32::consts::TAU * self.ring_hz * pitch / RATE as f32;
            let attack = (t * RATE as f32 / 2.0).min(1.0);
            let v = ((lp[0] - lp[1]) * self.noise + phase.sin() * self.tone) * env * attack + ring_phase.sin() * self.ring * (-t * self.ring_decay).exp() * attack;
            out.push([v, v]);
        }
        out
    }
}

/// A small deterministic hash, for per-key pitch and gain.
fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// A synthesized click, one flavour each.
pub fn click_sound(kind: ClickSound) -> Vec<Stereo> {
    let t = match kind {
        ClickSound::Classic => Tick {
            len_ms: 45.0, noise: 0.5, band_lo: 40.0, band_hi: 16_000.0,
            tone: 0.25, tone_hz: 2400.0, tone_hz2: 2400.0, glide_ms: 1.0,
            decay: 900.0, second: 0.6, second_ms: 25.0, second_decay: 1100.0,
            ring: 0.0, ring_hz: 5200.0, ring_decay: 60.0,
        },
        ClickSound::Soft => Tick {
            len_ms: 42.0, noise: 0.3, band_lo: 150.0, band_hi: 2600.0,
            tone: 0.3, tone_hz: 820.0, tone_hz2: 600.0, glide_ms: 20.0,
            decay: 1500.0, second: 0.25, second_ms: 22.0, second_decay: 1600.0,
            ring: 0.0, ring_hz: 2600.0, ring_decay: 60.0,
        },
        ClickSound::Sharp => Tick {
            len_ms: 30.0, noise: 0.65, band_lo: 900.0, band_hi: 12_000.0,
            tone: 0.3, tone_hz: 3600.0, tone_hz2: 3100.0, glide_ms: 10.0,
            decay: 2100.0, second: 0.3, second_ms: 12.0, second_decay: 2400.0,
            ring: 0.0, ring_hz: 6000.0, ring_decay: 60.0,
        },
        ClickSound::Thock => Tick {
            len_ms: 65.0, noise: 0.35, band_lo: 80.0, band_hi: 1600.0,
            tone: 0.55, tone_hz: 175.0, tone_hz2: 130.0, glide_ms: 30.0,
            decay: 650.0, second: 0.35, second_ms: 28.0, second_decay: 700.0,
            ring: 0.0, ring_hz: 900.0, ring_decay: 60.0,
        },
        ClickSound::Typewriter => Tick {
            len_ms: 62.0, noise: 0.55, band_lo: 300.0, band_hi: 7000.0,
            tone: 0.3, tone_hz: 2300.0, tone_hz2: 1400.0, glide_ms: 25.0,
            decay: 1300.0, second: 0.4, second_ms: 24.0, second_decay: 2100.0,
            ring: 0.09, ring_hz: 5200.0, ring_decay: 55.0,
        },
        ClickSound::Pop => Tick {
            len_ms: 45.0, noise: 0.12, band_lo: 200.0, band_hi: 4000.0,
            tone: 0.7, tone_hz: 900.0, tone_hz2: 420.0, glide_ms: 22.0,
            decay: 1300.0, second: 0.0, second_ms: 20.0, second_decay: 1000.0,
            ring: 0.0, ring_hz: 3000.0, ring_decay: 60.0,
        },
    };
    t.render(1.0, hash32(kind as u32 + 1))
}

/// Modifier key codes: they get no sound of their own, so ⌘S is one clack.
pub fn is_modifier(code: u16) -> bool {
    matches!(code, 54..=63)
}

/// A synthesized keyboard key: `code` picks a small pitch and gain variation
/// so neighbouring keys sound distinct, and space and return land heavier
/// than letters.
pub fn key_sound(kind: KeySound, code: u16) -> Vec<Stereo> {
    let t = match kind {
        KeySound::Mech => Tick {
            len_ms: 55.0, noise: 0.4, band_lo: 100.0, band_hi: 2200.0,
            tone: 0.45, tone_hz: 215.0, tone_hz2: 160.0, glide_ms: 25.0,
            decay: 700.0, second: 0.3, second_ms: 22.0, second_decay: 900.0,
            ring: 0.0, ring_hz: 800.0, ring_decay: 60.0,
        },
        KeySound::Typewriter => Tick {
            len_ms: 58.0, noise: 0.5, band_lo: 400.0, band_hi: 7500.0,
            tone: 0.25, tone_hz: 1900.0, tone_hz2: 1150.0, glide_ms: 18.0,
            decay: 1200.0, second: 0.35, second_ms: 20.0, second_decay: 2000.0,
            ring: 0.08, ring_hz: 5600.0, ring_decay: 45.0,
        },
        KeySound::Soft => Tick {
            len_ms: 38.0, noise: 0.25, band_lo: 800.0, band_hi: 4500.0,
            tone: 0.2, tone_hz: 1500.0, tone_hz2: 1200.0, glide_ms: 10.0,
            decay: 1700.0, second: 0.0, second_ms: 18.0, second_decay: 1800.0,
            ring: 0.0, ring_hz: 4000.0, ring_decay: 60.0,
        },
        KeySound::Pop => Tick {
            len_ms: 42.0, noise: 0.1, band_lo: 200.0, band_hi: 3500.0,
            tone: 0.65, tone_hz: 850.0, tone_hz2: 430.0, glide_ms: 20.0,
            decay: 1300.0, second: 0.0, second_ms: 18.0, second_decay: 1000.0,
            ring: 0.0, ring_hz: 3000.0, ring_decay: 60.0,
        },
    };
    let h = hash32(code as u32);
    let semis = (h % 5) as f32 - 2.0;
    let mut pitch = 2f32.powf(semis / 12.0);
    let mut gain = 1.0 + ((h >> 8) % 3) as f32 * 0.06 - 0.06;
    match code {
        // Space lands heavier and deeper; return a little heavier too.
        49 => {
            pitch *= 0.78;
            gain *= 1.35;
        }
        36 | 76 => {
            pitch *= 0.85;
            gain *= 1.15;
        }
        // Delete keys are crisper and a touch louder.
        51 | 117 => {
            pitch *= 1.06;
            gain *= 1.08;
        }
        48 => pitch *= 0.9,
        _ => {}
    }
    let mut s = t.render(pitch, h | 1);
    for v in &mut s {
        v[0] *= gain;
        v[1] *= gain;
    }
    s
}

/// A soft whoosh as a zoom moves the camera in: a noise band sweeping up,
/// quick to arrive and fading out.
pub fn zoom_whoosh() -> Vec<Stereo> {
    let n = (RATE * 0.45) as usize;
    let mut out = Vec::with_capacity(n);
    let mut rng = 0x51ed_2701u32;
    let mut lp = [0.0f32; 2];
    for i in 0..n {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let white = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let u = i as f32 / n as f32;
        let f = 380.0 * (2400.0f32 / 380.0).powf(u);
        let a = 1.0 - (-std::f32::consts::TAU * f / RATE as f32).exp();
        lp[0] += (white - lp[0]) * a;
        lp[1] += (white - lp[1]) * a * 0.25;
        let env = (u * 18.0).min(1.0) * (1.0 - u).powf(1.4);
        let v = (lp[0] - lp[1]) * env * 1.2;
        out.push([v, v]);
    }
    out
}

/// Add a short sound to the soundtrack at a sample position.
fn place(out: &mut [Stereo], at: usize, s: &[Stereo], g: f32) {
    for (k, v) in s.iter().enumerate() {
        if let Some(o) = out.get_mut(at + k) {
            o[0] += v[0] * g;
            o[1] += v[1] * g;
        }
    }
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
    if let Some(music) = style.music.as_ref().and_then(|p| crate::media::read_audio(Path::new(p), 0).ok()).filter(|m| !m.is_empty()) {
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
        let g = style.click_volume as f32;
        let sound = click_sound(style.click_sound);
        for d in Events::new(rec.mouse.clone()).downs {
            if let Some(pb) = map.to_playback(d.t) {
                place(&mut out, (pb / 1000.0 * RATE) as usize, &sound, g);
            }
        }
        any = true;
    }
    if style.key_sounds {
        let g = style.key_volume as f32;
        for k in rec.keys.iter().filter(|k| {
            k.down && !is_modifier(k.code) && (!style.key_typing_only || k.mods & (mods::CMD | mods::CTRL) == 0)
        }) {
            if let Some(pb) = map.to_playback(k.t) {
                place(&mut out, (pb / 1000.0 * RATE) as usize, &key_sound(style.key_sound, k.code), g);
            }
        }
        any = true;
    }
    if style.zoom_whoosh {
        let g = style.whoosh_volume as f32;
        let sound = zoom_whoosh();
        for z in project.timeline.zooms.iter().filter(|z| !z.disabled) {
            if let Some(pb) = map.to_playback(z.start) {
                place(&mut out, (pb / 1000.0 * RATE) as usize, &sound, g);
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

    /// Peak amplitude in a 1 ms window at `ms`.
    fn level_at(out: &[Stereo], ms: f64) -> f32 {
        let at = (ms / 1000.0 * RATE) as usize;
        out.iter().skip(at).take((RATE * 0.001) as usize).fold(0.0f32, |m, s| m.max(s[0].abs()).max(s[1].abs()))
    }

    fn click(t: f64) -> crate::project::recording::MouseEvent {
        crate::project::recording::MouseEvent { t, x: 0.0, y: 0.0, kind: crate::project::recording::MouseKind::Down, button: 0, cursor: 0 }
    }

    fn key(t: f64, code: u16, m: u8) -> crate::project::recording::KeyEvent {
        crate::project::recording::KeyEvent { t, code, key: "X".into(), mods: m, down: true }
    }

    #[test]
    fn every_sound_is_finite_unique_and_deterministic() {
        let mut all: Vec<(&str, Vec<Stereo>)> = vec![];
        for k in ClickSound::ALL {
            all.push((k.label(), click_sound(k)));
        }
        for k in KeySound::ALL {
            all.push((k.label(), key_sound(k, 8)));
        }
        all.push(("whoosh", zoom_whoosh()));
        for (name, s) in &all {
            assert!(!s.is_empty(), "{name} is empty");
            assert!(s.iter().all(|v| v[0].is_finite() && v[1].is_finite()), "{name} has non-finite samples");
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v[0].abs()));
            assert!(peak > 0.05 && peak < 1.5, "{name} peak {peak}");
        }
        // Flavour by flavour, no two sounds are the same buffer.
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i].1, all[j].1, "{} and {} are identical", all[i].0, all[j].0);
            }
        }
        // Deterministic: the same flavour renders the same samples.
        assert_eq!(click_sound(ClickSound::Thock), click_sound(ClickSound::Thock));
        assert_eq!(key_sound(KeySound::Mech, 12), key_sound(KeySound::Mech, 12));
    }

    #[test]
    fn keys_vary_and_accent_the_big_bar() {
        assert_ne!(key_sound(KeySound::Mech, 8), key_sound(KeySound::Mech, 40), "keys vary");
        let rms = |s: &[Stereo]| (s.iter().map(|v| v[0] * v[0]).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms(&key_sound(KeySound::Mech, 49)) > rms(&key_sound(KeySound::Mech, 8)) * 1.15, "space is heavier");
        assert!(is_modifier(55) && is_modifier(59) && is_modifier(63) && !is_modifier(8));
    }

    #[test]
    fn mix_places_sounds_on_the_edit() {
        use crate::project::model::{Slice, Zoom};
        use crate::project::recording::Recording;

        let mut rec = Recording::default();
        rec.mouse = vec![click(500.0)];
        rec.keys = vec![
            key(700.0, 8, 0),
            crate::project::recording::KeyEvent { t: 730.0, code: 8, key: "X".into(), mods: 0, down: false },
            key(800.0, 55, crate::project::recording::mods::CMD), // ⌘ alone: no sound
            key(1000.0, 3, crate::project::recording::mods::CMD), // ⌘F: a shortcut
        ];
        let mut p = Project::default();
        p.timeline.slices = vec![Slice { id: 1, start: 0.0, end: 3000.0, ..Default::default() }];
        let map = TimeMap::new(&p.timeline.slices, 3000.0);

        assert!(mix_unmastered(&p, &rec, None, &map).is_none(), "silent with every sound off");

        p.style.audio.click_sounds = true;
        p.style.audio.click_volume = 0.5;
        let out = mix_unmastered(&p, &rec, None, &map).expect("clicks are mixed");
        assert!(level_at(&out, 500.0) > 0.02, "the click is at its source time");
        assert!(level_at(&out, 1500.0) < 1e-4, "and nowhere else");

        p.style.audio.key_sounds = true;
        p.style.audio.key_volume = 0.5;
        let out = mix_unmastered(&p, &rec, None, &map).expect("keys are mixed");
        assert!(level_at(&out, 700.0) > 0.02, "a key press sounds");
        assert!(level_at(&out, 800.0) < 1e-4, "a modifier alone is silent");
        assert!(level_at(&out, 1000.0) > 0.02, "shortcuts sound by default");
        p.style.audio.key_typing_only = true;
        let out = mix_unmastered(&p, &rec, None, &map).expect("keys are mixed");
        assert!(level_at(&out, 1000.0) < 1e-4, "typing only skips the shortcut");
        assert!(level_at(&out, 700.0) > 0.02, "typing still sounds");

        p.style.audio.zoom_whoosh = true;
        p.timeline.zooms = vec![Zoom { id: 1, start: 2000.0, end: 2600.0, ..Default::default() }];
        let out = mix_unmastered(&p, &rec, None, &map).expect("the whoosh is mixed");
        assert!(level_at(&out, 2015.0) > 0.02, "the whoosh starts with the zoom");

        // A sped-up slice moves every sound with the edit.
        p.timeline.slices[0].speed = 2.0;
        let map = TimeMap::new(&p.timeline.slices, 3000.0);
        let out = mix_unmastered(&p, &rec, None, &map).expect("clicks are mixed");
        assert!(level_at(&out, 250.0) > 0.02 && level_at(&out, 500.0) < 1e-4, "2x moves the click to half its time");
    }
}
