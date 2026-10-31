//! Animated GIF export: frames are quantised to 256 colours in parallel
//! (NeuQuant) and written in order. After the first frame only the region
//! that changed is encoded (unchanged pixels inside it are transparent), and
//! frames with no visible change merge into the previous one.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use anyhow::{Context, Result, bail};

use super::{FramePump, Inputs, Job, ProgressFn};
use crate::engine::scene::Scene;
use crate::project::model::Quality;

#[allow(clippy::too_many_arguments)]
pub(super) fn encode(
    job: &Job,
    out: &std::path::Path,
    scene: &mut Scene,
    pump: &mut FramePump,
    inputs: &mut Inputs,
    total: u64,
    progress: ProgressFn,
    cancel: &AtomicBool,
) -> Result<u64> {
    let s = job.settings;
    let (w, h) = (pump.w, pump.h);
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        bail!("GIF too large");
    }
    let file = File::create(out).with_context(|| format!("creating {}", out.display()))?;
    let mut enc = gif::Encoder::new(BufWriter::new(file), w as u16, h as u16, &[])?;
    enc.set_repeat(if s.gif_loop { gif::Repeat::Infinite } else { gif::Repeat::Finite(0) })?;
    // Lower is slower and better; Studio spends more time per frame.
    let speed = if s.quality == Quality::Studio { 4 } else { 10 };
    // Light mode: two quantizers, leaving the Mac usable while it exports.
    let workers = if crate::hw::light() { 2 } else { std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).max(2) };

    let (job_tx, job_rx) = mpsc::sync_channel::<(u64, Option<Delta>)>(workers * 2);
    let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
    let (out_tx, out_rx) = mpsc::channel::<(u64, Option<gif::Frame<'static>>)>();
    let mut threads = vec![];
    for _ in 0..workers {
        let (rx, tx) = (job_rx.clone(), out_tx.clone());
        threads.push(std::thread::spawn(move || {
            loop {
                let next = rx.lock().unwrap().recv();
                let Ok((i, delta)) = next else { break };
                let frame = delta.map(|d| quantize(&d, speed));
                if tx.send((i, frame)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(out_tx);

    // Frame timing in centiseconds, carrying rounding error forward.
    let frame_cs = 100.0 / s.fps as f64;
    let mut written = 0u64;
    let mut ready: BTreeMap<u64, Option<gif::Frame<'static>>> = BTreeMap::new();
    let mut held: Option<(gif::Frame<'static>, f64)> = None;
    let mut clock = 0.0f64;
    let mut emitted_cs = 0u64;
    let mut flush = |ready: &mut BTreeMap<u64, Option<gif::Frame<'static>>>,
                     held: &mut Option<(gif::Frame<'static>, f64)>,
                     enc: &mut gif::Encoder<BufWriter<File>>,
                     written: &mut u64|
     -> Result<()> {
        while let Some(f) = ready.remove(written) {
            *written += 1;
            match (f, held.as_mut()) {
                // No visible change: the previous frame stays up longer.
                (None, Some((_, dur))) => *dur += frame_cs,
                (None, None) => {}
                (Some(f), _) => {
                    if let Some((mut prev, dur)) = held.take() {
                        clock += dur;
                        let end = clock.round() as u64;
                        prev.delay = (end - emitted_cs).max(2) as u16;
                        emitted_cs += prev.delay as u64;
                        enc.write_frame(&prev)?;
                    }
                    *held = Some((f, frame_cs));
                }
            }
        }
        Ok(())
    };

    let mut submitted = 0u64;
    // What viewers currently see (RGBA), to diff each new frame against.
    let mut shown: Option<Vec<u8>> = None;
    let mut send_oldest = |pump: &mut FramePump, submitted: &mut u64| {
        if let Some(rgba) = pump.take(|rows, stride| {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h as usize {
                for px in rows[y * stride..y * stride + w as usize * 4].chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            out
        }) {
            let delta = match &mut shown {
                None => {
                    shown = Some(rgba.clone());
                    Some(Delta { x: 0, y: 0, w: w as u16, h: h as u16, rgba })
                }
                Some(prev) => delta(prev, &rgba, w as usize, h as usize),
            };
            let _ = job_tx.send((*submitted, delta));
            *submitted += 1;
        }
    };
    for i in 0..total {
        if cancel.load(Ordering::SeqCst) {
            drop(job_tx);
            bail!("export cancelled");
        }
        let frame = scene.frame(i as f64 * 1000.0 / s.fps as f64, h as f64);
        inputs.feed(&mut pump.renderer, frame.source_ms, None);
        pump.submit(&frame, &job.project.style);
        if pump.full() {
            send_oldest(pump, &mut submitted);
        }
        while let Ok((idx, f)) = out_rx.try_recv() {
            ready.insert(idx, f);
        }
        flush(&mut ready, &mut held, &mut enc, &mut written)?;
        progress(i as f64 / total as f64 * 0.95);
    }
    while !pump.pending.is_empty() {
        send_oldest(pump, &mut submitted);
    }
    drop(job_tx);
    for (idx, f) in out_rx.iter() {
        ready.insert(idx, f);
        flush(&mut ready, &mut held, &mut enc, &mut written)?;
    }
    for t in threads {
        let _ = t.join();
    }
    if let Some((mut prev, dur)) = held.take() {
        clock += dur;
        prev.delay = ((clock.round() as u64).saturating_sub(emitted_cs)).max(2) as u16;
        enc.write_frame(&prev)?;
    }
    Ok(written)
}

/// The changed part of a frame: a sub-rectangle with unchanged pixels transparent.
struct Delta {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    rgba: Vec<u8>,
}

/// Palette index reserved for "keep what's there".
const TRANSPARENT: u8 = 255;

/// Quantise a delta to a GIF frame. The palette is learned from the drawn
/// (opaque) pixels only — transparent ones would otherwise dominate it — and
/// is exact when they use at most 255 colours.
fn quantize(d: &Delta, speed: i32) -> gif::Frame<'static> {
    use std::collections::HashMap;
    let opaque: Vec<u8> = d.rgba.chunks_exact(4).filter(|p| p[3] != 0).flatten().copied().collect();
    let mut exact: HashMap<[u8; 3], u8> = HashMap::new();
    let mut overflow = false;
    for p in opaque.chunks_exact(4) {
        let c = [p[0], p[1], p[2]];
        if !exact.contains_key(&c) {
            if exact.len() == TRANSPARENT as usize {
                overflow = true;
                break;
            }
            let n = exact.len() as u8;
            exact.insert(c, n);
        }
    }
    let mut palette = vec![0u8; 256 * 3];
    let buffer: Vec<u8> = if !overflow {
        for (c, &i) in &exact {
            palette[i as usize * 3..i as usize * 3 + 3].copy_from_slice(c);
        }
        d.rgba.chunks_exact(4).map(|p| if p[3] == 0 { TRANSPARENT } else { exact[&[p[0], p[1], p[2]]] }).collect()
    } else {
        let nq = color_quant::NeuQuant::new(speed, TRANSPARENT as usize, &opaque);
        let map = nq.color_map_rgb();
        palette[..map.len()].copy_from_slice(&map);
        d.rgba.chunks_exact(4).map(|p| if p[3] == 0 { TRANSPARENT } else { nq.index_of(p) as u8 }).collect()
    };
    gif::Frame {
        left: d.x,
        top: d.y,
        width: d.w,
        height: d.h,
        buffer: std::borrow::Cow::Owned(buffer),
        palette: Some(palette),
        transparent: Some(TRANSPARENT),
        dispose: gif::DisposalMethod::Keep,
        ..Default::default()
    }
}

/// Per-channel difference below which a pixel counts as unchanged.
const THRESHOLD: u8 = 2;

fn changed(a: &[u8], b: &[u8]) -> bool {
    a[0].abs_diff(b[0]) > THRESHOLD || a[1].abs_diff(b[1]) > THRESHOLD || a[2].abs_diff(b[2]) > THRESHOLD
}

/// Tile size for change detection: a tile that changed is redrawn whole (so
/// no stale quantisation error lingers inside it); untouched tiles inside the
/// changed box are transparent.
const TILE: usize = 32;

/// Diff `next` against what is shown, update `shown` with what will be drawn,
/// and return it (None when nothing visibly changed). Comparing against
/// what's on screen, not the previous frame, keeps slow changes from
/// accumulating unseen.
fn delta(shown: &mut [u8], next: &[u8], w: usize, h: usize) -> Option<Delta> {
    let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let mut dirty = vec![false; tw * th];
    for y in 0..h {
        let row = y * w * 4;
        let ty = y / TILE;
        for x in 0..w {
            let t = ty * tw + x / TILE;
            if dirty[t] {
                continue;
            }
            let i = row + x * 4;
            if changed(&shown[i..i + 4], &next[i..i + 4]) {
                dirty[t] = true;
            }
        }
    }
    let (mut tx0, mut ty0, mut tx1, mut ty1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for ty in 0..th {
        for tx in 0..tw {
            if dirty[ty * tw + tx] {
                tx0 = tx0.min(tx);
                tx1 = tx1.max(tx);
                ty0 = ty0.min(ty);
                ty1 = ty1.max(ty);
            }
        }
    }
    if tx0 == usize::MAX {
        return None;
    }
    let (x0, y0) = (tx0 * TILE, ty0 * TILE);
    let (x1, y1) = (((tx1 + 1) * TILE).min(w), ((ty1 + 1) * TILE).min(h));
    let (dw, dh) = (x1 - x0, y1 - y0);
    let mut rgba = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = (x0 + x, y0 + y);
            if !dirty[(sy / TILE) * tw + sx / TILE] {
                continue;
            }
            let i = (sy * w + sx) * 4;
            let o = (y * dw + x) * 4;
            rgba[o..o + 3].copy_from_slice(&next[i..i + 3]);
            rgba[o + 3] = 255;
            shown[i..i + 3].copy_from_slice(&next[i..i + 3]);
        }
    }
    Some(Delta { x: x0 as u16, y: y0 as u16, w: dw as u16, h: dh as u16, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_keeps_exact_colours_and_transparency() {
        let mut rgba = vec![0u8; 3 * 4];
        rgba[0..4].copy_from_slice(&[10, 20, 30, 255]);
        rgba[8..12].copy_from_slice(&[200, 100, 50, 255]);
        let f = quantize(&Delta { x: 1, y: 2, w: 3, h: 1, rgba }, 10);
        assert_eq!(f.buffer[1], TRANSPARENT);
        let pal = f.palette.as_ref().unwrap();
        let c0 = f.buffer[0] as usize * 3;
        assert_eq!(&pal[c0..c0 + 3], &[10, 20, 30]);
        let c2 = f.buffer[2] as usize * 3;
        assert_eq!(&pal[c2..c2 + 3], &[200, 100, 50]);
        assert_eq!((f.left, f.top), (1, 2));
    }

    #[test]
    fn delta_redraws_changed_tiles_and_tracks_what_is_shown() {
        let (w, h) = (100usize, 70usize);
        let mut shown = vec![100u8; w * h * 4];
        let mut next = shown.clone();
        assert!(delta(&mut shown, &next, w, h).is_none());
        // Two changes in tiles (0,0) and (2,1); a tiny change elsewhere is ignored.
        for (x, y) in [(5usize, 5usize), (70, 40)] {
            next[(y * w + x) * 4] = 200;
        }
        next[(60 * w + 90) * 4] = 101;
        let d = delta(&mut shown, &next, w, h).unwrap();
        assert_eq!((d.x, d.y, d.w, d.h), (0, 0, 96, 64));
        // Changed tiles are opaque throughout; untouched tiles are transparent.
        let alpha = |x: usize, y: usize| d.rgba[(y * 96 + x) * 4 + 3];
        assert_eq!(alpha(31, 31), 255);
        assert_eq!(alpha(64, 32), 255);
        assert_eq!(alpha(40, 5), 0);
        // Shown now matches; the same frame again is no change.
        assert!(delta(&mut shown, &next, w, h).is_none());
        // Slow drift is caught once it exceeds the threshold against what's shown.
        let mut drift = next.clone();
        drift[(60 * w + 90) * 4] = 104;
        assert!(delta(&mut shown, &drift, w, h).is_some());
    }
}
