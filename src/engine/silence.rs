//! Pauses: stretches of the recording where nobody is talking and nothing is
//! playing, found from the audio levels so the editor can cut them out.

/// Length of one level measurement.
const BLOCK_MS: f64 = 20.0;
/// Less than this much sound in the whole recording and there's no speech to
/// keep, so nothing is called a pause.
const MIN_SOUND_MS: f64 = 1000.0;

/// Level of each 20 ms block in dBFS (RMS of the two channels averaged).
pub fn levels(samples: &[[f32; 2]], rate: f64) -> Vec<f32> {
    let block = ((rate * BLOCK_MS / 1000.0) as usize).max(1);
    samples
        .chunks(block)
        .map(|b| {
            let ms = b.iter().map(|s| (((s[0] + s[1]) * 0.5) as f64).powi(2)).sum::<f64>() / b.len() as f64;
            (10.0 * ms.max(1e-12).log10()) as f32
        })
        .collect()
}

/// Per-block loudest level across several tracks. A block missing from a
/// shorter track counts as silent for that track.
pub fn loudest(tracks: &[Vec<f32>]) -> Vec<f32> {
    let len = tracks.iter().map(Vec::len).max().unwrap_or(0);
    (0..len).map(|i| tracks.iter().filter_map(|t| t.get(i)).fold(f32::NEG_INFINITY, |m, &l| m.max(l))).collect()
}

/// Level below which a block is quiet: 12 dB above the quietest tenth of the
/// recording (its noise floor), kept between -60 and -35 dBFS.
pub fn threshold(levels: &[f32]) -> f32 {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted.get(sorted.len() / 10).copied().unwrap_or(-60.0);
    (floor + 12.0).clamp(-60.0, -35.0)
}

/// Source-time ranges (ms) of quiet stretches at least `min_ms` long, each
/// shrunk by `keep_ms` on both sides so the cut doesn't clip a word. Empty
/// when the recording has almost no sound at all.
pub fn pauses(levels: &[f32], min_ms: f64, keep_ms: f64) -> Vec<(f64, f64)> {
    let limit = threshold(levels);
    let sound = levels.iter().filter(|&&l| l >= limit).count() as f64 * BLOCK_MS;
    if sound < MIN_SOUND_MS {
        return vec![];
    }
    let mut out = vec![];
    let mut run_start: Option<usize> = None;
    for i in 0..=levels.len() {
        let quiet = levels.get(i).is_some_and(|&l| l < limit);
        match (quiet, run_start) {
            (true, None) => run_start = Some(i),
            (false, Some(s)) => {
                run_start = None;
                let (start, end) = (s as f64 * BLOCK_MS, i as f64 * BLOCK_MS);
                if end - start >= min_ms && end - start > 2.0 * keep_ms {
                    out.push((start + keep_ms, end - keep_ms));
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEECH: f32 = -25.0;
    const QUIET: f32 = -70.0;

    /// `secs` seconds of blocks at `db`.
    fn blocks(db: f32, secs: f64) -> Vec<f32> {
        vec![db; (secs * 1000.0 / BLOCK_MS) as usize]
    }

    #[test]
    fn levels_measure_rms_in_db() {
        let tone: Vec<[f32; 2]> = (0..48_000)
            .map(|i| {
                let v = 0.1 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin();
                [v, v]
            })
            .collect();
        let l = levels(&tone, 48_000.0);
        assert_eq!(l.len(), 50);
        assert!((l[10] - -23.0).abs() < 0.5, "{}", l[10]);
        assert!(levels(&[[0.0, 0.0]; 960], 48_000.0)[0] < -100.0);
    }

    #[test]
    fn finds_a_long_pause_and_keeps_a_margin() {
        let mut l = blocks(SPEECH, 1.0);
        l.extend(blocks(QUIET, 2.0));
        l.extend(blocks(SPEECH, 1.0));
        assert_eq!(pauses(&l, 500.0, 150.0), vec![(1150.0, 2850.0)]);
    }

    #[test]
    fn ignores_short_pauses_and_silent_recordings() {
        let mut l = blocks(SPEECH, 1.0);
        l.extend(blocks(QUIET, 0.4));
        l.extend(blocks(SPEECH, 1.0));
        assert!(pauses(&l, 500.0, 150.0).is_empty());
        assert!(pauses(&blocks(QUIET, 10.0), 500.0, 150.0).is_empty());
    }

    #[test]
    fn a_pause_at_the_end_counts_and_loudest_merges_tracks() {
        let mut mic = blocks(SPEECH, 1.5);
        mic.extend(blocks(QUIET, 2.0));
        let system = blocks(QUIET, 3.5);
        let merged = loudest(&[mic.clone(), system]);
        assert_eq!(merged, mic);
        assert_eq!(pauses(&merged, 500.0, 150.0), vec![(1650.0, 3350.0)]);
        // Sound on the other track keeps the stretch.
        let mut system = blocks(QUIET, 1.5);
        system.extend(blocks(SPEECH, 2.0));
        assert!(pauses(&loudest(&[mic, system]), 500.0, 150.0).is_empty());
    }
}
