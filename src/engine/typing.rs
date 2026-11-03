//! Typing speed-up suggestions: find stretches of the recording where the
//! user was typing (plain key presses, no ⌘/⌃ shortcuts, short gaps) so the
//! editor can offer to play them faster.

use crate::project::model::{Slice, Timeline};
use crate::project::recording::{KeyEvent, mods};

/// Gap between presses that still counts as one typing run.
pub const MAX_GAP_MS: f64 = 1500.0;
/// Shortest run worth speeding up.
pub const MIN_LEN_MS: f64 = 2500.0;
pub const MIN_KEYS: usize = 8;
/// Leave a little real-time context around each run.
pub const MARGIN_MS: f64 = 150.0;

/// Source-time ranges of typing runs.
pub fn ranges(keys: &[KeyEvent]) -> Vec<(f64, f64)> {
    let mut out = vec![];
    let mut run: Option<(f64, f64, usize)> = None;
    let typing = keys.iter().filter(|k| k.down && k.mods & (mods::CMD | mods::CTRL) == 0);
    for k in typing {
        run = match run {
            Some((s, e, n)) if k.t - e <= MAX_GAP_MS => Some((s, k.t, n + 1)),
            Some((s, e, n)) => {
                if e - s >= MIN_LEN_MS && n >= MIN_KEYS {
                    out.push((s, e));
                }
                Some((k.t, k.t, 1))
            }
            None => Some((k.t, k.t, 1)),
        };
    }
    if let Some((s, e, n)) = run
        && e - s >= MIN_LEN_MS
        && n >= MIN_KEYS
    {
        out.push((s, e));
    }
    out.into_iter().map(|(s, e)| (s + MARGIN_MS, e - MARGIN_MS)).filter(|(s, e)| e > s).collect()
}

/// Split slices at a source time (at least 100 ms from slice edges).
/// Returns false when no slice could be split there.
pub fn split_at(tl: &mut Timeline, t: f64) -> bool {
    let Some(i) = tl.slices.iter().position(|s| t > s.start + 100.0 && t < s.end - 100.0) else {
        return false;
    };
    let id = tl.new_id();
    let right = Slice { id, start: t, ..tl.slices[i] };
    tl.slices[i].end = t;
    tl.slices.insert(i + 1, right);
    true
}

/// Drop the part of the slice around source time `t` before it (`keep_after`)
/// or after it. `t` must be at least 100 ms inside the slice, as for
/// `split_at`. `duration` is the recording length, used when the edit has no
/// slices yet. Returns whether anything was trimmed.
pub fn trim_at(tl: &mut Timeline, duration: f64, t: f64, keep_after: bool) -> bool {
    let (start, end) = match tl.slices.iter().find(|s| t >= s.start && t < s.end) {
        Some(s) => (s.start, s.end),
        None if tl.slices.is_empty() => (0.0, duration),
        None => return false,
    };
    if t <= start + 100.0 || t >= end - 100.0 {
        return false;
    }
    if keep_after { cut(tl, duration, start, t) } else { cut(tl, duration, t, end) }
}

/// Play the given source ranges at `speed`: split at their edges and set the
/// speed of every slice inside. Returns how many slices changed.
pub fn speed_up(tl: &mut Timeline, ranges: &[(f64, f64)], speed: f64) -> usize {
    for &(s, e) in ranges {
        split_at(tl, s);
        split_at(tl, e);
    }
    let mut n = 0;
    for slice in &mut tl.slices {
        let mid = (slice.start + slice.end) * 0.5;
        if ranges.iter().any(|&(s, e)| mid > s && mid < e && slice.start >= s - 1.0 && slice.end <= e + 1.0) {
            slice.speed = speed;
            n += 1;
        }
    }
    n
}

/// Remove the source range `s..e` from the edit, trimming or splitting the
/// slices it touches (leftover pieces under 100 ms go too). `duration` is
/// the recording length, used when the edit has no slices yet. Never
/// removes everything; returns whether anything was cut.
pub fn cut(tl: &mut Timeline, duration: f64, s: f64, e: f64) -> bool {
    const MIN_PIECE_MS: f64 = 100.0;
    if tl.slices.is_empty() {
        let id = tl.new_id();
        tl.slices.push(Slice { id, start: 0.0, end: duration, ..Default::default() });
    }
    let mut out = Vec::with_capacity(tl.slices.len() + 1);
    let mut changed = false;
    for sl in tl.slices.clone() {
        if sl.end <= s || sl.start >= e {
            out.push(sl);
            continue;
        }
        changed = true;
        let left = Slice { end: s, ..sl };
        let keep_left = left.end - left.start >= MIN_PIECE_MS;
        if keep_left {
            out.push(left);
        }
        let right = Slice { start: e, ..sl };
        if right.end - right.start >= MIN_PIECE_MS {
            let id = if keep_left { tl.new_id() } else { sl.id };
            out.push(Slice { id, ..right });
        }
    }
    if !changed || out.is_empty() {
        return false;
    }
    tl.slices = out;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(t: f64, m: u8) -> KeyEvent {
        KeyEvent { t, code: 0, key: "A".into(), mods: m, down: true }
    }

    #[test]
    fn finds_typing_runs_and_ignores_shortcuts() {
        let mut keys: Vec<KeyEvent> = (0..20).map(|i| key(1000.0 + i as f64 * 200.0, 0)).collect();
        // A shortcut burst and an isolated press are not typing.
        keys.extend((0..10).map(|i| key(10_000.0 + i as f64 * 100.0, mods::CMD)));
        keys.push(key(20_000.0, 0));
        let r = ranges(&keys);
        assert_eq!(r.len(), 1);
        assert!((r[0].0 - 1150.0).abs() < 1e-9 && (r[0].1 - 4650.0).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn short_bursts_are_ignored() {
        let keys: Vec<KeyEvent> = (0..5).map(|i| key(i as f64 * 100.0, 0)).collect();
        assert!(ranges(&keys).is_empty());
    }

    #[test]
    fn speed_up_splits_and_sets_speed() {
        let mut tl = Timeline::default();
        let id = tl.new_id();
        tl.slices.push(Slice { id, start: 0.0, end: 10_000.0, speed: 1.0, ..Default::default() });
        let n = speed_up(&mut tl, &[(2000.0, 5000.0)], 3.0);
        assert_eq!(n, 1);
        let spans: Vec<_> = tl.slices.iter().map(|s| (s.start, s.end, s.speed)).collect();
        assert_eq!(spans, vec![(0.0, 2000.0, 1.0), (2000.0, 5000.0, 3.0), (5000.0, 10_000.0, 1.0)]);
    }

    #[test]
    fn trim_at_drops_one_side_of_the_slice() {
        let mut tl = Timeline::default();
        assert!(trim_at(&mut tl, 10_000.0, 3000.0, true));
        let spans: Vec<_> = tl.slices.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(spans, vec![(3000.0, 10_000.0)]);
        assert!(trim_at(&mut tl, 10_000.0, 8000.0, false));
        let spans: Vec<_> = tl.slices.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(spans, vec![(3000.0, 8000.0)]);
        // Too close to an edge, or outside every slice: nothing changes.
        assert!(!trim_at(&mut tl, 10_000.0, 3050.0, true));
        assert!(!trim_at(&mut tl, 10_000.0, 1000.0, true));
        assert_eq!(tl.slices.len(), 1);
    }

    #[test]
    fn cut_removes_a_range() {
        let mut tl = Timeline::default();
        assert!(cut(&mut tl, 10_000.0, 1400.0, 2600.0));
        let spans: Vec<_> = tl.slices.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(spans, vec![(0.0, 1400.0), (2600.0, 10_000.0)]);
        // Overlapping an edge trims; a range ending past the next slice's
        // start trims that one too.
        assert!(cut(&mut tl, 10_000.0, 1350.0, 3000.0));
        let spans: Vec<_> = tl.slices.iter().map(|s| (s.start, s.end)).collect();
        assert_eq!(spans, vec![(0.0, 1350.0), (3000.0, 10_000.0)]);
        // Never cuts everything.
        assert!(!cut(&mut tl, 10_000.0, 0.0, 10_000.0));
        assert_eq!(tl.slices.len(), 2);
    }
}
