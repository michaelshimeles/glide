//! Zoom: automatic zoom generation from clicks, and where the camera looks
//! while zoomed.

use super::events::Events;
use super::geom::{Rect, Vec2, remap, v2};
use super::sizes::Sizes;
use crate::project::model::{Timeline, Zoom, ZoomMode};
use crate::project::recording::MouseEvent;

/// A zoom starts this long before the click that triggers it…
const AUTO_LEAD_MS: f64 = 300.0;
/// …and holds this long after it.
const AUTO_HOLD_MS: f64 = 2500.0;
/// Zooms closer than this are merged.
const AUTO_MERGE_GAP_MS: f64 = 2500.0;
/// No zooms for clicks in the last second, and none reaching the last 0.8 s.
const AUTO_END_GUARD_MS: f64 = 1000.0;
const AUTO_END_MARGIN_MS: f64 = 800.0;
/// Instant zooms cut within this distance of their edges.
const INSTANT_EDGE_MS: f64 = 100.0;
/// Snap-to-edge ratio used while following the mouse between zooms.
const FOLLOW_SNAP: f64 = 0.5;

/// Generate zoom ranges from clicks.
pub fn auto_zooms(clicks: &[MouseEvent], duration_ms: f64, scale: f64, timeline: &mut Timeline) -> Vec<Zoom> {
    let mut spans: Vec<(f64, f64)> = clicks
        .iter()
        .filter(|c| c.t < duration_ms - AUTO_END_GUARD_MS)
        .map(|c| {
            let t = c.t.floor();
            ((t - AUTO_LEAD_MS).max(1.0), (t + AUTO_HOLD_MS).min(duration_ms - AUTO_END_MARGIN_MS))
        })
        .filter(|(a, b)| b > a)
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = vec![];
    for (a, b) in spans {
        match merged.last_mut() {
            Some(last) if last.1 + AUTO_MERGE_GAP_MS >= a => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    merged
        .into_iter()
        .map(|(a, b)| Zoom { id: timeline.new_id(), start: a.floor(), end: b.floor(), scale, ..Default::default() })
        .collect()
}

/// The enabled zoom covering source time `t`.
pub fn active(zooms: &[Zoom], t: f64) -> Option<&Zoom> {
    zooms.iter().filter(|z| !z.disabled && t >= z.start && t <= z.end).max_by(|a, b| a.start.total_cmp(&b.start))
}

/// Whether animation should be skipped at `t` (edge of an instant zoom).
pub fn is_instant_edge(zooms: &[Zoom], t: f64) -> bool {
    zooms
        .iter()
        .filter(|z| !z.disabled && z.instant)
        .any(|z| (t - z.start).abs() < INSTANT_EDGE_MS || (t - z.end).abs() < INSTANT_EDGE_MS)
}

/// A group of events that fit together in the zoomed view.
struct Cluster {
    first_t: f64,
    min: Vec2,
    max: Vec2,
}

impl Cluster {
    fn center(&self) -> Vec2 {
        (self.min + self.max) / 2.0
    }
}

/// Greedy time-ordered clustering: an event joins the current cluster while
/// the cluster's bounding box still fits within `area`.
fn clusters(events: &[MouseEvent], area: Vec2) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = vec![];
    for e in events {
        let p = v2(e.x, e.y);
        if let Some(c) = out.last_mut() {
            let (lo, hi) = (v2(c.min.x.min(p.x), c.min.y.min(p.y)), v2(c.max.x.max(p.x), c.max.y.max(p.y)));
            if hi.x - lo.x <= area.x && hi.y - lo.y <= area.y {
                c.min = lo;
                c.max = hi;
                continue;
            }
        }
        out.push(Cluster { first_t: e.t, min: p, max: p });
    }
    out
}

/// What the camera should frame at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Focus {
    /// Point of interest in video pixels.
    pub point: Vec2,
    pub zoom: f64,
    pub snap: f64,
    /// Put `point` in the middle of the view (stopping at the screen's
    /// edges) instead of leaving it as far across the view as it is across
    /// the screen. `snap` is ignored.
    pub centered: bool,
}

/// Focus for an auto zoom: the latest click/activity cluster that started.
pub fn auto_focus(z: &Zoom, events: &Events, sizes: &Sizes, t: f64) -> Option<Vec2> {
    let mut inside = Events::range(&events.all, z.start, z.end).to_vec();
    if inside.is_empty()
        && let Some(e) = Events::last_at(&events.all, z.start).or(events.all.first())
    {
        inside.push(*e);
    }
    let fill = sizes.content.div(sizes.window);
    let area = sizes.crop.size().div(v2(fill.y, fill.x)) / z.scale.max(0.01);
    let cs = clusters(&inside, area.mul(v2(0.5, 0.7)));
    let pick = cs.iter().rev().find(|c| c.first_t <= t).or(cs.first())?;
    Some(pick.center())
}

/// Window placement for a focus: (scale, offset) relative to the unzoomed
/// window, in design pixels.
pub fn place(sizes: &Sizes, focus: Focus) -> (f64, Vec2) {
    let scale = focus.zoom * sizes.initial_zoom;
    let crop = sizes.crop;
    // Focus relative to the cropped screen, normalised.
    let s = crop.scalar_of(focus.point).clamp(0.0, 1.0);
    // Push targets near edges fully to the edge.
    let body = sizes.default_body();
    let visible = sizes.output / sizes.initial_zoom / scale;
    let max_snap = visible.div(sizes.content);
    let frame = Rect::from_min_size(Vec2::ZERO, sizes.content);
    let size = body.size() * scale;
    let fit = v2(size.x.min(frame.w), size.y.min(frame.h));
    // Centring a point needs half the visible share of the screen on each side.
    let r = if focus.centered {
        v2(fit.x / size.x / 2.0, fit.y / size.y / 2.0)
    } else {
        v2(focus.snap.clamp(0.0, max_snap.x), focus.snap.clamp(0.0, max_snap.y))
    };
    let s = v2(remap(s.x, r.x, 1.0 - r.x + 1e-4, 0.0, 1.0), remap(s.y, r.y, 1.0 - r.y + 1e-4, 0.0, 1.0));
    // Interpolate between edge-aligned placements so the zoomed window
    // never exposes the background inside the content frame.
    let lo = (frame.size() - fit) / 2.0;
    let left = lo;
    let right = lo + fit - size;
    let pos = v2(left.x + (right.x - left.x) * s.x, left.y + (right.y - left.y) * s.y);
    (scale, pos - body.min())
}

/// Zoom focus at source time `t`; gaps between zooms gently follow the
/// cursor at zoom 1.
pub fn focus_at(timeline: &Timeline, events: &Events, sizes: &Sizes, t: f64, cursor: Option<Vec2>) -> Focus {
    let center = sizes.crop.center();
    match active(&timeline.zooms, t) {
        Some(z) => {
            let point = match z.mode {
                ZoomMode::Manual { target } => sizes.crop.at(target),
                ZoomMode::FollowMouse => cursor.unwrap_or(center),
                ZoomMode::Auto => auto_focus(z, events, sizes, t).unwrap_or(center),
            };
            let centered = matches!(z.mode, ZoomMode::Manual { .. });
            Focus { point, zoom: z.scale.max(1.0), snap: z.snap_edges, centered }
        }
        None => Focus { point: cursor.unwrap_or(center), zoom: 1.0, snap: FOLLOW_SNAP, centered: false },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::model::Style;
    use crate::project::recording::MouseKind;

    fn click(t: f64, x: f64, y: f64) -> MouseEvent {
        MouseEvent { t, x, y, kind: MouseKind::Down, button: 0, cursor: 0 }
    }

    #[test]
    fn auto_zoom_merges_nearby_clicks() {
        let mut tl = Timeline::default();
        let clicks = [click(1000.0, 0.0, 0.0), click(4000.0, 0.0, 0.0), click(12_000.0, 0.0, 0.0)];
        let z = auto_zooms(&clicks, 20_000.0, 2.0, &mut tl);
        assert_eq!(z.len(), 2);
        assert_eq!((z[0].start, z[0].end), (700.0, 6500.0));
        assert_eq!((z[1].start, z[1].end), (11_700.0, 14_500.0));
        // Clicks in the final second never zoom.
        assert!(auto_zooms(&[click(19_500.0, 0.0, 0.0)], 20_000.0, 2.0, &mut tl).is_empty());
    }

    #[test]
    fn unzoomed_window_does_not_move() {
        let sizes = Sizes::new(&Style::default(), v2(1920.0, 1080.0));
        let (scale, off) = place(&sizes, Focus { point: v2(10.0, 10.0), zoom: 1.0, snap: 0.5, centered: false });
        assert_eq!(scale, 1.0);
        assert!(off.length() < 1e-9);
    }

    #[test]
    fn zoom_on_corner_aligns_window_edge() {
        let mut style = Style::default();
        style.padding = 0.0;
        let sizes = Sizes::new(&style, v2(1920.0, 1080.0));
        // Top-left focus: zoomed window's top-left stays at the frame's top-left.
        let (scale, off) = place(&sizes, Focus { point: v2(0.0, 0.0), zoom: 2.0, snap: 0.25, centered: false });
        assert_eq!(scale, 2.0);
        assert!(off.length() < 1e-6, "{off:?}");
        // Bottom-right focus: the window is shifted left/up by its overflow.
        let (_, off) = place(&sizes, Focus { point: v2(1920.0, 1080.0), zoom: 2.0, snap: 0.25, centered: false });
        assert!((off.x + 1920.0).abs() < 1.0 && (off.y + 1080.0).abs() < 1.0, "{off:?}");
        // Centre focus: overflow split evenly.
        let (_, off) = place(&sizes, Focus { point: v2(960.0, 540.0), zoom: 2.0, snap: 0.25, centered: false });
        assert!((off.x + 960.0).abs() < 1.0, "{off:?}");
    }

    /// Where a screen point (video pixels) is drawn in the content area, with
    /// the window placed for `focus`.
    fn drawn_at(sizes: &Sizes, focus: Focus, p: Vec2) -> Vec2 {
        let (scale, off) = place(sizes, focus);
        sizes.default_body().min() + off + (p - sizes.crop.min()).mul(v2(scale, scale))
    }

    #[test]
    fn a_manual_zoom_puts_its_point_in_the_middle_of_the_view() {
        let sizes = Sizes::new(&Style::default(), v2(1920.0, 1080.0));
        let mid = sizes.content / 2.0;
        for zoom in [1.5, 2.0, 3.0] {
            for s in [v2(0.5, 0.5), v2(0.4, 0.45), v2(0.6, 0.55)] {
                let point = sizes.crop.at(s);
                let at = drawn_at(&sizes, Focus { point, zoom, snap: 0.0, centered: true }, point);
                assert!((at.x - mid.x).abs() < 0.5 && (at.y - mid.y).abs() < 0.5, "zoom {zoom}, point {s:?} drawn at {at:?}, middle is {mid:?}");
            }
        }
    }

    #[test]
    fn a_centred_zoom_near_an_edge_stops_at_the_screen_edge() {
        let sizes = Sizes::new(&Style::default(), v2(1920.0, 1080.0));
        let focus = |s: Vec2| Focus { point: sizes.crop.at(s), zoom: 2.0, snap: 0.0, centered: true };
        // Top-left corner: the window's corner sits at the content's corner.
        let (_, off) = place(&sizes, focus(v2(0.02, 0.03)));
        assert!((sizes.default_body().min() + off).length() < 1e-6, "{off:?}");
        // Bottom-right corner: its far corner sits at the content's far corner.
        let (scale, off) = place(&sizes, focus(v2(0.99, 0.98)));
        let far = sizes.default_body().min() + off + sizes.window * scale;
        assert!((far - sizes.content).length() < 1e-6, "{far:?}");
    }

    #[test]
    fn clusters_split_far_apart_clicks() {
        let events = Events::new(vec![click(100.0, 100.0, 100.0), click(200.0, 120.0, 110.0), click(900.0, 1800.0, 900.0)]);
        let sizes = Sizes::new(&Style::default(), v2(1920.0, 1080.0));
        let z = Zoom { start: 0.0, end: 3000.0, ..Default::default() };
        let early = auto_focus(&z, &events, &sizes, 300.0).unwrap();
        assert_eq!(early, v2(110.0, 105.0));
        let late = auto_focus(&z, &events, &sizes, 1000.0).unwrap();
        assert_eq!(late, v2(1800.0, 900.0));
    }
}
