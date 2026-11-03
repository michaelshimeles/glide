//! Beats: the hits in a piece of audio (drums, loud peaks), found from how
//! fast its level rises, and the shake and pulse an effect makes of them.
//! Pure functions, so the preview and the export draw the same frames.

use serde::{Deserialize, Serialize};

/// Length of one level measurement.
const BLOCK_MS: f64 = 20.0;
/// A hit has to be at least this far from the one before it.
const MIN_GAP_MS: f64 = 120.0;
/// How many earlier blocks the level is compared with.
const LOOKBACK: usize = 8;
/// Level rise (dB) that counts as a full-strength hit.
const FULL_RISE_DB: f64 = 18.0;
/// Hits weaker than this are dropped, so a file holds no noise (loud hiss
/// makes random bumps up to about 0.1).
const MIN_STORED: f64 = 0.12;
/// Under this level (dBFS) nothing is audible enough to be a hit.
const GATE_DB: f32 = -55.0;
/// Where the low-pass that brings out kick drums starts (Hz).
const BASS_HZ: f64 = 180.0;

/// How long a hit takes to fade.
const DECAY_MS: f64 = 140.0;
/// Shake: size of a full hit as a fraction of the picture's height, and the
/// speed it wobbles at.
const SHAKE_FRACTION: f64 = 0.012;
const SHAKE_HZ: f64 = 14.0;
/// Pulse: how much bigger the picture gets on a full hit.
const PULSE_GROW: f64 = 0.04;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    /// Milliseconds into the audio.
    pub t: f64,
    /// 0..1; 1 is a very sharp hit.
    pub strength: f64,
}

/// The hits of the audio a project follows, and where they live in time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beats {
    pub beats: Vec<Beat>,
    /// Music repeats, so its hits repeat every `loop_ms` of the edit. 0 for
    /// the recording's own audio, whose hits are in recording time.
    pub loop_ms: f64,
}

impl Beats {
    /// The time to look hits up at, given the edit's time and the
    /// recording's time.
    pub fn time_at(&self, playback_ms: f64, source_ms: f64) -> f64 {
        if self.loop_ms > 0.0 { playback_ms.rem_euclid(self.loop_ms) } else { source_ms }
    }
}

/// RMS level in dBFS of each block of `mono`.
fn block_levels(mono: &[f32], block: usize) -> Vec<f32> {
    mono.chunks(block)
        .map(|b| {
            let ms = b.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / b.len() as f64;
            (10.0 * ms.max(1e-12).log10()) as f32
        })
        .collect()
}

/// How much louder each block is than the average of the blocks before it
/// (dB, never negative).
fn rises(levels: &[f32]) -> Vec<f64> {
    (0..levels.len())
        .map(|i| {
            let from = i.saturating_sub(LOOKBACK);
            if from == i {
                return 0.0;
            }
            let before = levels[from..i].iter().map(|&l| l as f64).sum::<f64>() / (i - from) as f64;
            (levels[i] as f64 - before).max(0.0)
        })
        .collect()
}

/// Find the hits in stereo `samples` at `rate`. The low end counts most, so
/// kick drums win over hiss; the whole spectrum adds a little, so snares and
/// claps register too.
pub fn onsets(samples: &[[f32; 2]], rate: f64) -> Vec<Beat> {
    let block = ((rate * BLOCK_MS / 1000.0) as usize).max(1);
    let mono: Vec<f32> = samples.iter().map(|s| (s[0] + s[1]) * 0.5).collect();
    // One-pole low-pass.
    let a = 1.0 - (-std::f64::consts::TAU * BASS_HZ / rate).exp();
    let mut y = 0.0f64;
    let bass: Vec<f32> = mono
        .iter()
        .map(|&x| {
            y += a * (x as f64 - y);
            y as f32
        })
        .collect();
    let full = block_levels(&mono, block);
    let low = block_levels(&bass, block);
    let (rise_full, rise_low) = (rises(&full), rises(&low));
    let flux: Vec<f64> = (0..full.len())
        .map(|i| if full[i] < GATE_DB { 0.0 } else { (0.7 * rise_low[i] + 0.3 * rise_full[i]) / FULL_RISE_DB })
        .collect();
    let strength = |i: usize| flux[i].min(1.0);
    let mut out: Vec<Beat> = vec![];
    for i in 0..flux.len() {
        let before = if i > 0 { flux[i - 1] } else { 0.0 };
        let after = flux.get(i + 1).copied().unwrap_or(0.0);
        if strength(i) < MIN_STORED || flux[i] < before || flux[i] <= after {
            continue;
        }
        let beat = Beat { t: i as f64 * BLOCK_MS, strength: strength(i) };
        match out.last_mut() {
            Some(last) if beat.t - last.t < MIN_GAP_MS => {
                if beat.strength > last.strength {
                    *last = beat;
                }
            }
            _ => out.push(beat),
        }
    }
    out
}

/// The weakest hit that counts, for a sensitivity of 0 (only sharp hits) to
/// 1 (everything).
pub fn threshold(sensitivity: f64) -> f64 {
    0.6 - 0.45 * sensitivity.clamp(0.0, 1.0)
}

/// A hit's effect on the picture at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// 1 at the hit, fading to 0, times the hit's strength.
    pub amp: f64,
    /// Milliseconds since the hit.
    pub age: f64,
    /// Which hit it was, for a direction that doesn't change between frames.
    pub index: usize,
}

/// The most recent hit at or before `t` that is at least `min_strength`,
/// as a fading pulse. None when there is none or it has faded out.
pub fn pulse(beats: &[Beat], t: f64, min_strength: f64) -> Option<Pulse> {
    let end = beats.partition_point(|b| b.t <= t);
    let index = (0..end).rev().take(8).find(|&i| beats[i].strength >= min_strength)?;
    let age = t - beats[index].t;
    let amp = beats[index].strength * (-age / DECAY_MS).exp();
    (amp > 0.01).then_some(Pulse { amp, age, index })
}

/// 0 at the ends of an effect, 1 inside, easing over `fade_ms`.
pub fn edge_gain(t: f64, start: f64, end: f64, fade_ms: f64) -> f64 {
    let edge = ((t - start).min(end - t) / fade_ms).clamp(0.0, 1.0);
    edge * edge * (3.0 - 2.0 * edge)
}

/// How far a shake moves the picture, in pixels, for a picture `height`
/// pixels tall and an effect strength 0..1. The direction depends on which
/// hit it was; the movement wobbles and dies down with the pulse.
pub fn shake_offset(p: Pulse, strength: f64, height: f64) -> (f64, f64) {
    let angle = (((p.index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) as f64 / (1u64 << 24) as f64) * std::f64::consts::TAU;
    let wobble = (std::f64::consts::TAU * SHAKE_HZ * p.age / 1000.0).cos();
    let size = p.amp * strength * SHAKE_FRACTION * height * wobble;
    (angle.cos() * size, angle.sin() * size)
}

/// How much bigger a pulse makes the picture (1 = not at all).
pub fn pulse_scale(p: Pulse, strength: f64) -> f64 {
    1.0 + PULSE_GROW * strength * p.amp
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// Quiet hiss for `secs`, with a 60 Hz kick of amplitude `amp` starting
    /// at each time in `kicks` (ms).
    fn track(secs: f64, kicks: &[(f64, f32)]) -> Vec<[f32; 2]> {
        track_over(secs, kicks, 0.004)
    }

    /// `track` over hiss of peak-to-peak size `hiss`.
    fn track_over(secs: f64, kicks: &[(f64, f32)], hiss: f32) -> Vec<[f32; 2]> {
        let n = (secs * RATE) as usize;
        let mut noise = 12345u32;
        let mut out: Vec<f32> = (0..n)
            .map(|_| {
                noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((noise >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * hiss
            })
            .collect();
        for &(ms, amp) in kicks {
            let start = (ms / 1000.0 * RATE) as usize;
            for k in 0..(0.2 * RATE) as usize {
                let t = k as f64 / RATE;
                if start + k < n {
                    out[start + k] += amp * (-t / 0.05).exp() as f32 * (std::f64::consts::TAU * 60.0 * t).sin() as f32;
                }
            }
        }
        out.into_iter().map(|v| [v, v]).collect()
    }

    #[test]
    fn finds_kicks_where_they_are() {
        let kicks: Vec<(f64, f32)> = (0..6).map(|i| (500.0 + i as f64 * 500.0, 0.8)).collect();
        let found = onsets(&track(4.0, &kicks), RATE);
        assert_eq!(found.len(), 6, "{found:?}");
        for (b, (ms, _)) in found.iter().zip(&kicks) {
            assert!((b.t - ms).abs() <= 25.0, "hit at {} for a kick at {ms}", b.t);
            assert!(b.strength > 0.5, "{b:?}");
        }
    }

    #[test]
    fn hiss_and_silence_have_no_hits() {
        assert!(onsets(&track(3.0, &[]), RATE).is_empty());
        assert!(onsets(&vec![[0.0, 0.0]; RATE as usize * 2], RATE).is_empty());
    }

    #[test]
    fn a_hit_that_barely_stands_out_is_weaker_and_sensitivity_decides_if_it_counts() {
        // Over loud hiss, a big kick stands out far more than a small one.
        let found = onsets(&track_over(3.0, &[(500.0, 0.8), (1500.0, 0.02)], 0.2), RATE);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[1].strength < found[0].strength, "{found:?}");
        assert!(found[1].strength < threshold(0.0), "a sharp-only setting skips it: {found:?}");
        assert!(found[1].strength >= threshold(1.0), "an everything setting keeps it: {found:?}");
        assert!(found[0].strength >= threshold(0.0));
        assert!(threshold(1.0) < threshold(0.5) && threshold(0.5) < threshold(0.0));
    }

    #[test]
    fn two_close_hits_count_once() {
        let found = onsets(&track(2.0, &[(500.0, 0.8), (580.0, 0.8)]), RATE);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_pulse_peaks_on_the_hit_and_fades() {
        let beats = [Beat { t: 1000.0, strength: 1.0 }, Beat { t: 2000.0, strength: 0.5 }];
        assert!(pulse(&beats, 900.0, 0.1).is_none(), "before the first hit");
        let on = pulse(&beats, 1000.0, 0.1).unwrap();
        assert!((on.amp - 1.0).abs() < 1e-9 && on.index == 0);
        let later = pulse(&beats, 1100.0, 0.1).unwrap();
        assert!(later.amp < on.amp && later.amp > 0.4);
        assert!(pulse(&beats, 1800.0, 0.1).is_none(), "faded out");
        let second = pulse(&beats, 2000.0, 0.1).unwrap();
        assert_eq!(second.index, 1);
        assert!((second.amp - 0.5).abs() < 1e-9);
        // A hit under the threshold is skipped, the one before it counts.
        assert_eq!(pulse(&beats, 2010.0, 0.8), None);
    }

    #[test]
    fn shake_is_deterministic_and_fades_with_the_hit() {
        let p = pulse(&[Beat { t: 100.0, strength: 1.0 }], 100.0, 0.1).unwrap();
        assert_eq!(shake_offset(p, 1.0, 1080.0), shake_offset(p, 1.0, 1080.0));
        let (dx, dy) = shake_offset(p, 1.0, 1080.0);
        assert!((dx.hypot(dy) - 0.012 * 1080.0).abs() < 1e-6, "a full hit moves 1.2 % of the height");
        assert_eq!(shake_offset(p, 0.0, 1080.0), (0.0, 0.0));
        let later = Pulse { amp: p.amp * 0.1, age: 300.0, index: p.index };
        assert!(shake_offset(later, 1.0, 1080.0).0.hypot(shake_offset(later, 1.0, 1080.0).1) < 2.0);
        // Different hits go in different directions.
        let other = Pulse { index: 3, ..p };
        assert_ne!(shake_offset(p, 1.0, 1080.0), shake_offset(other, 1.0, 1080.0));
    }

    #[test]
    fn pulse_scale_grows_with_the_hit_and_strength() {
        let p = Pulse { amp: 1.0, age: 0.0, index: 0 };
        assert!((pulse_scale(p, 1.0) - 1.04).abs() < 1e-9);
        assert!((pulse_scale(p, 0.0) - 1.0).abs() < 1e-9);
        assert!(pulse_scale(Pulse { amp: 0.2, ..p }, 1.0) < pulse_scale(p, 1.0));
    }

    #[test]
    fn edges_ease_in_and_out() {
        assert_eq!(edge_gain(1000.0, 1000.0, 5000.0, 100.0), 0.0);
        assert!((edge_gain(3000.0, 1000.0, 5000.0, 100.0) - 1.0).abs() < 1e-9);
        assert!(edge_gain(1050.0, 1000.0, 5000.0, 100.0) > 0.0 && edge_gain(1050.0, 1000.0, 5000.0, 100.0) < 1.0);
        assert_eq!(edge_gain(5000.0, 1000.0, 5000.0, 100.0), 0.0);
    }

    #[test]
    fn music_hits_repeat_and_recording_hits_do_not() {
        let music = Beats { beats: vec![], loop_ms: 4000.0 };
        assert!((music.time_at(9500.0, 123.0) - 1500.0).abs() < 1e-9);
        let rec = Beats::default();
        assert_eq!(rec.time_at(9500.0, 123.0), 123.0);
    }
}
