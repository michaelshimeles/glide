//! Timeline: ruler, clip track (slices), zoom track, mask track, effect track
//! and camera layout track.
//! Everything is drawn in playback time; edits are stored in source time.

use egui::{Color32, CornerRadius, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::editor::{Editor, Selection};
use super::theme::{self, pal};
use crate::engine::beats;
use crate::project::model::{Effect, EffectKind, LayoutKind, Mask, MaskKind, Zoom, ZoomMode};

const RULER_H: f32 = 18.0;
const TRACK_H: f32 = 40.0;
const GAP: f32 = 6.0;
const EDGE: f32 = 7.0;
/// Row of "data may be visible" warnings above the masks (only when any).
const WARN_H: f32 = 20.0;

/// Ranges where content under a mask moved, in playback ms (cut parts
/// don't show).
fn warnings(ed: &Editor) -> Vec<(crate::exposure::Exposure, f64, f64)> {
    let map = &ed.preview.scene.map;
    ed.mask_watch
        .found
        .iter()
        .map(|e| (*e, map.to_playback_nearest(e.start), map.to_playback_nearest(e.end)))
        .filter(|(_, a, b)| b - a >= 1.0)
        .collect()
}

/// Extra height the timeline needs right now.
pub fn extra_height(ed: &Editor) -> f32 {
    if warnings(ed).is_empty() { 0.0 } else { WARN_H + GAP }
}

#[derive(Clone, Copy)]
enum Drag {
    /// `orig` is the edge's source time and `grab_x` the pointer's x when the
    /// drag started.
    SliceEdge { id: u64, left: bool, orig: f64, grab_x: f32 },
    Zoom { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Layout { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Mask { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Effect { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    /// An effect being painted: it starts at `start` and its end follows the
    /// playhead while the video plays, else the pointer.
    NewEffect { id: u64, start: f64 },
    Playhead,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Body,
    Left,
    Right,
}

fn zoom_color() -> Color32 {
    theme::VIOLET
}
fn clip_color() -> Color32 {
    Color32::from_rgb(0xc6, 0x82, 0x0e)
}
/// The zoom track's background, a deep indigo.
fn zoom_track_bg() -> Color32 {
    Color32::from_rgb(0x11, 0x10, 0x25)
}
fn layout_color() -> Color32 {
    Color32::from_rgb(0x10, 0xb9, 0x81)
}
fn mask_color() -> Color32 {
    Color32::from_rgb(0xec, 0x48, 0x99)
}
fn effect_color() -> Color32 {
    Color32::from_rgb(0xf9, 0x73, 0x60)
}

/// `v.clamp(lo, hi)`, or `keep` when the range is empty (an item already
/// shorter than the minimum length can't be resized further; `clamp` would
/// panic).
fn clamp_or(v: f64, lo: f64, hi: f64, keep: f64) -> f64 {
    if lo <= hi { v.clamp(lo, hi) } else { keep }
}

/// Source time of a clip edge grabbed at `orig` once the pointer has moved
/// `dx_px` since. It depends only on the grab and the pointer, never on where
/// the edge is now: trimming ripples the timeline, so the edge's own position
/// on screen doesn't follow the pointer, and measuring from it would add the
/// same distance again every frame.
fn edge_after_drag(orig: f64, dx_px: f32, px_per_ms: f64, speed: f64) -> f64 {
    let speed = if speed > 0.0 { speed } else { 1.0 };
    orig + dx_px as f64 / px_per_ms * speed
}

/// Where an effect that is being painted from `start` ends when the playhead
/// or pointer is at `now` (source ms) in a recording `duration` long: never
/// shorter than the minimum, never past the end, and never before `start`.
fn painted_end(start: f64, now: f64, duration: f64) -> f64 {
    now.max(start + Effect::MIN_LEN_MS).min(duration).max(start)
}

/// Greedy lane assignment so overlapping ranges don't cover each other.
fn lanes(ranges: &[(f64, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by(|&a, &b| ranges[a].0.total_cmp(&ranges[b].0));
    let mut ends: Vec<f64> = vec![];
    let mut out = vec![0; ranges.len()];
    for i in order {
        let (s, e) = ranges[i];
        let lane = ends.iter().position(|&end| end <= s).unwrap_or_else(|| {
            ends.push(f64::MIN);
            ends.len() - 1
        });
        ends[lane] = e;
        out[i] = lane;
    }
    out
}

/// The timeline before anything is recorded: the transport and tracks,
/// disabled and empty.
pub fn empty(ui: &mut Ui) {
    let p = pal(ui);
    ui.add_enabled_ui(false, |ui| {
        ui.horizontal(|ui| {
            let _ = ui.add(egui::Button::new(RichText::new("▶").size(14.0)).min_size(vec2(32.0, 24.0)));
            ui.label(RichText::new("0:00.0 / 0:00.0").monospace());
            let _ = ui.selectable_label(false, "Loop");
            ui.separator();
            for label in ["✂ Split", "+ Zoom", "+ Mask", "Delete"] {
                let _ = ui.button(label);
            }
        });
    });
    ui.add_space(4.0);
    let width = ui.available_width() - 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, RULER_H + 4.0 * (TRACK_H + GAP) + 4.0), Sense::hover());
    let painter = ui.painter_at(rect);
    for (i, name) in ["Clips", "Zoom", "Masks", "Effects"].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left(), rect.top() + RULER_H + 2.0 + i as f32 * (TRACK_H + GAP)), vec2(width, TRACK_H));
        painter.rect_filled(r, CornerRadius::same(10), if i == 1 { zoom_track_bg() } else { Color32::from_white_alpha(8) });
        painter.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, name, egui::FontId::proportional(10.0), p.tertiary);
        if i == 0 {
            painter.text(r.center(), egui::Align2::CENTER_CENTER, "Your recording opens here when you stop", egui::FontId::proportional(12.0), p.secondary);
        }
    }
}

pub fn ui(ui: &mut Ui, ed: &mut Editor) {
    let p = pal(ui);
    let has_cam = ed.rec.meta.camera.is_some();
    let tracks = if has_cam { 5 } else { 4 };
    let warns = warnings(ed);
    let extra = if warns.is_empty() { 0.0 } else { WARN_H + GAP };
    let height = RULER_H + tracks as f32 * (TRACK_H + GAP) + 4.0 + extra;
    let dur = ed.duration().max(1.0);
    let width = (dur as f32 / 1000.0 * ed.px_per_sec).max(ui.available_width() - 4.0);
    let drag_id = ui.id().with("timeline-drag");
    egui::ScrollArea::horizontal().id_salt("timeline-scroll").auto_shrink(false).show(ui, |ui| {
        let (rect, bg) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
        let px_per_ms = ed.px_per_sec / 1000.0;
        let x_of = |pb: f64| rect.left() + pb as f32 * px_per_ms;
        let pb_of = |x: f32| ((x - rect.left()) / px_per_ms) as f64;
        let painter = ui.painter_at(rect);
        let map = ed.preview.scene.map.clone();

        // Ruler.
        let ruler = Rect::from_min_size(rect.min, vec2(width, RULER_H));
        let step_s = [0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0].into_iter().find(|s| *s * ed.px_per_sec as f64 >= 60.0).unwrap_or(120.0);
        let mut t = 0.0;
        while t * 1000.0 <= dur {
            let x = x_of(t * 1000.0);
            painter.line_segment([pos2(x, ruler.bottom() - 5.0), pos2(x, ruler.bottom())], Stroke::new(1.0, p.tertiary));
            painter.text(pos2(x + 3.0, ruler.top() + 1.0), egui::Align2::LEFT_TOP, super::widgets::fmt_time(t * 1000.0), egui::FontId::proportional(10.0), p.tertiary);
            t += step_s;
        }

        let track = |i: usize| {
            let below_warnings = if i >= 2 { extra } else { 0.0 };
            Rect::from_min_size(pos2(rect.left(), rect.top() + RULER_H + 2.0 + i as f32 * (TRACK_H + GAP) + below_warnings), vec2(width, TRACK_H))
        };
        let clip_row = track(0);
        let zoom_row = track(1);
        let mask_row = track(2);
        let effect_row = track(3);
        let layout_row = track(4);
        for r in [clip_row, zoom_row, mask_row, effect_row].into_iter().chain(has_cam.then_some(layout_row)) {
            painter.rect_filled(r, CornerRadius::same(10), if r == zoom_row { zoom_track_bg() } else { Color32::from_white_alpha(8) });
        }

        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
        let mut hover_cursor = None;

        // Clip track: one block per kept slice.
        for l in &map.links {
            let r = Rect::from_min_max(pos2(x_of(l.pb_start) + 1.0, clip_row.top()), pos2(x_of(l.pb_end) - 1.0, clip_row.bottom()));
            let slice = ed.project.timeline.slices.get(l.slice).copied();
            let id = slice.map_or(0, |s| s.id);
            let selected = ed.selection == Selection::Slice(id);
            let c = clip_color();
            painter.rect_filled(r, CornerRadius::same(10), c.gamma_multiply(if selected { 1.0 } else { 0.92 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let mut label = String::from("Clip");
            if (l.speed - 1.0).abs() > 1e-3 {
                label = format!("{:.2}×", l.speed).replace(".00", "");
            }
            if slice.is_some_and(|s| s.hide_cursor) {
                label.push_str(" · cursor hidden");
            }
            painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
                && slice.is_some()
            {
                let near_l = (pos.x - r.left()).abs() < EDGE;
                let near_r = (pos.x - r.right()).abs() < EDGE;
                if near_l || near_r {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started()
                    && drag.is_none()
                    && (near_l || near_r)
                    && let Some(s) = slice
                {
                    let grab_x = ui.input(|i| i.pointer.press_origin()).map_or(pos.x, |p| p.x);
                    drag = Some(Drag::SliceEdge { id, left: near_l, orig: if near_l { s.start } else { s.end }, grab_x });
                } else if bg.clicked() {
                    ed.selection = Selection::Slice(id);
                }
            }
        }

        // Voice-overs: thin bars along the bottom of the clip track.
        for vo in &ed.project.timeline.voiceovers {
            let a = map.to_playback_nearest(vo.start);
            let b = (a + vo.duration).min(map.duration());
            if b > a {
                let r = Rect::from_min_max(pos2(x_of(a) + 2.0, clip_row.bottom() - 7.0), pos2(x_of(b) - 2.0, clip_row.bottom() - 3.0));
                painter.rect_filled(r, CornerRadius::same(2), Color32::from_rgb(0x06, 0xb6, 0xd4));
            }
        }

        // Zoom track.
        let zooms = ed.project.timeline.zooms.clone();
        for z in &zooms {
            let (a, b) = (map.to_playback_nearest(z.start), map.to_playback_nearest(z.end));
            if b <= a {
                continue;
            }
            let r = Rect::from_min_max(pos2(x_of(a) + 1.0, zoom_row.top()), pos2(x_of(b) - 1.0, zoom_row.bottom()));
            let selected = ed.selection == Selection::Zoom(z.id);
            let c = if z.disabled { p.tertiary } else { zoom_color() };
            painter.rect_filled(r, CornerRadius::same(6), c.gamma_multiply(if selected { 0.95 } else { 0.7 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let mode = match z.mode {
                ZoomMode::Auto => "Auto",
                ZoomMode::Manual { .. } => "Manual",
                ZoomMode::FollowMouse => "Follow",
            };
            if r.width() > 40.0 {
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, format!("{:.1}× {mode}", z.scale), theme::semibold(11.0), Color32::WHITE);
            }
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
            {
                let part = if (pos.x - r.left()).abs() < EDGE {
                    Part::Left
                } else if (pos.x - r.right()).abs() < EDGE {
                    Part::Right
                } else {
                    Part::Body
                };
                if part != Part::Body {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() {
                    drag = Some(Drag::Zoom { id: z.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (z.start, z.end) });
                    ed.selection = Selection::Zoom(z.id);
                } else if bg.clicked() {
                    ed.selection = Selection::Zoom(z.id);
                }
            }
        }
        // Double-click empty zoom track to add a zoom there.
        if bg.double_clicked()
            && let Some(pos) = pointer
            && zoom_row.contains(pos)
            && !zooms.iter().any(|z| {
                let (a, b) = (map.to_playback_nearest(z.start), map.to_playback_nearest(z.end));
                pos.x >= x_of(a) && pos.x <= x_of(b)
            })
        {
            let src = map.to_source(pb_of(pos.x));
            let scale = ed.project.style.default_zoom;
            let end_limit = ed.rec.meta.duration_ms;
            ed.edit(|p| {
                let id = p.timeline.new_id();
                p.timeline.zooms.push(Zoom { id, start: src, end: (src + 2500.0).min(end_limit), scale, ..Default::default() });
            });
        }

        // Mask track: overlapping masks stack in lanes.
        let masks = ed.project.timeline.masks.clone();
        let mask_lanes = lanes(&masks.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>());
        let lane_count = mask_lanes.iter().copied().max().map_or(1, |n| n + 1) as f32;
        for (m, lane) in masks.iter().zip(&mask_lanes) {
            let (a, b) = (map.to_playback_nearest(m.start), map.to_playback_nearest(m.end));
            if b <= a {
                continue;
            }
            let lane_h = mask_row.height() / lane_count;
            let top = mask_row.top() + *lane as f32 * lane_h;
            let r = Rect::from_min_max(pos2(x_of(a) + 1.0, top + 0.5), pos2(x_of(b) - 1.0, top + lane_h - 0.5));
            let selected = ed.selection == Selection::Mask(m.id);
            painter.rect_filled(r, CornerRadius::same(6), mask_color().gamma_multiply(if selected { 0.95 } else { 0.7 }));
            if selected {
                painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
            }
            let label = match m.kind {
                MaskKind::Blur => "Blur",
                MaskKind::Pixelate => "Pixelate",
                MaskKind::Highlight => "Highlight",
            };
            if r.width() > 40.0 {
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
            }
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
            {
                let part = if (pos.x - r.left()).abs() < EDGE {
                    Part::Left
                } else if (pos.x - r.right()).abs() < EDGE {
                    Part::Right
                } else {
                    Part::Body
                };
                if part != Part::Body {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() {
                    drag = Some(Drag::Mask { id: m.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (m.start, m.end) });
                    ed.selection = Selection::Mask(m.id);
                } else if bg.clicked() {
                    ed.selection = Selection::Mask(m.id);
                }
            }
        }
        // "Data may be visible": content under a blur/pixelate mask moved.
        let warn_row = Rect::from_min_size(pos2(rect.left(), zoom_row.bottom() + GAP), vec2(width, WARN_H));
        let mut warning_clicked = None;
        for (i, (e, a, b)) in warns.iter().enumerate() {
            let r = Rect::from_min_max(pos2(x_of(*a) + 1.0, warn_row.top()), pos2((x_of(*b) - 1.0).max(x_of(*a) + 20.0), warn_row.bottom()));
            let resp = ui.interact(r, ui.id().with(("exposure", i)), Sense::click()).on_hover_text(
                "The content under this mask moved here, so what it was hiding may show. Click to cut this part.",
            );
            let amber = Color32::from_rgb(0xff, 0x9f, 0x0a);
            painter.rect_filled(r, CornerRadius::same(5), amber.gamma_multiply(if resp.hovered() { 0.95 } else { 0.75 }));
            // Warning sign: a triangle with "!".
            let c = pos2(r.left() + 11.0, r.center().y);
            let tri = vec![pos2(c.x, c.y - 6.5), pos2(c.x + 7.0, c.y + 5.5), pos2(c.x - 7.0, c.y + 5.5)];
            painter.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
            painter.text(pos2(c.x, c.y + 1.5), egui::Align2::CENTER_CENTER, "!", theme::semibold(9.0), amber);
            if r.width() >= 128.0 {
                painter.text(pos2(r.left() + 22.0, r.center().y), egui::Align2::LEFT_CENTER, "Data may be visible", theme::semibold(11.0), Color32::WHITE);
            }
            if resp.clicked() {
                warning_clicked = Some(*e);
            }
            if resp.hovered() {
                hover_cursor = Some(egui::CursorIcon::PointingHand);
            }
        }
        if let Some(e) = warning_clicked {
            ed.cut_exposure(e);
        }

        // Double-click empty mask track to add a mask there.
        if bg.double_clicked()
            && let Some(pos) = pointer
            && mask_row.contains(pos)
            && !masks.iter().any(|m| {
                let (a, b) = (map.to_playback_nearest(m.start), map.to_playback_nearest(m.end));
                pos.x >= x_of(a) && pos.x <= x_of(b)
            })
        {
            let src = map.to_source(pb_of(pos.x));
            let end_limit = ed.rec.meta.duration_ms;
            let mut new_id = 0;
            ed.edit(|p| {
                let id = p.timeline.new_id();
                new_id = id;
                p.timeline.masks.push(Mask { id, start: src, end: (src + 3000.0).min(end_limit), ..Default::default() });
            });
            ed.selection = Selection::Mask(new_id);
        }

        // Effect track: the hits in the audio as bars, and the effects over them.
        let dur_src = ed.rec.meta.duration_ms;
        {
            let on_hit = beats::threshold(0.5);
            let heard = &ed.rec.beats;
            let (mid, tall) = (effect_row.center().y, effect_row.height() - 12.0);
            let visible = ui.clip_rect();
            let bars = ui.painter_at(effect_row);
            let bar = |pb: f64, strength: f64| {
                let x = x_of(pb);
                if x < visible.left() - 2.0 || x > visible.right() + 2.0 {
                    return;
                }
                let h = tall * strength.clamp(0.1, 1.0) as f32;
                let color = theme::ACCENT.gamma_multiply(if strength >= on_hit { 0.55 } else { 0.2 });
                bars.line_segment([pos2(x, mid - h / 2.0), pos2(x, mid + h / 2.0)], Stroke::new(1.5, color));
            };
            for b in &heard.beats {
                if heard.loop_ms > 0.0 {
                    let mut t = b.t;
                    while t <= map.duration() {
                        bar(t, b.strength);
                        t += heard.loop_ms;
                    }
                } else if let Some(pb) = map.to_playback(b.t) {
                    bar(pb, b.strength);
                }
            }
            if ed.project.timeline.effects.is_empty() {
                painter.text(effect_row.center(), egui::Align2::CENTER_CENTER, "Press and hold here while it plays to add an effect", egui::FontId::proportional(11.0), p.tertiary);
            }
        }
        let effects = ed.project.timeline.effects.clone();
        let effect_lanes = lanes(&effects.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>());
        let effect_lane_count = effect_lanes.iter().copied().max().map_or(1, |n| n + 1) as f32;
        for (e, lane) in effects.iter().zip(&effect_lanes) {
            let (a, b) = (map.to_playback_nearest(e.start), map.to_playback_nearest(e.end));
            if b <= a {
                continue;
            }
            let lane_h = effect_row.height() / effect_lane_count;
            let top = effect_row.top() + *lane as f32 * lane_h;
            let r = Rect::from_min_max(pos2(x_of(a) + 1.0, top + 0.5), pos2(x_of(b) - 1.0, top + lane_h - 0.5));
            let selected = ed.selection == Selection::Effect(e.id);
            painter.rect_filled(r, CornerRadius::same(6), effect_color().gamma_multiply(if selected { 0.6 } else { 0.4 }));
            painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { Color32::WHITE } else { effect_color() }), egui::StrokeKind::Inside);
            let label = EffectKind::ALL.iter().find(|(k, _)| *k == e.kind).map_or("", |(_, n)| *n);
            if r.width() > 40.0 {
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
            }
            if let Some(pos) = pointer
                && r.expand(2.0).contains(pos)
            {
                let part = if (pos.x - r.left()).abs() < EDGE {
                    Part::Left
                } else if (pos.x - r.right()).abs() < EDGE {
                    Part::Right
                } else {
                    Part::Body
                };
                if part != Part::Body {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() {
                    drag = Some(Drag::Effect { id: e.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (e.start, e.end) });
                    ed.selection = Selection::Effect(e.id);
                } else if bg.clicked() {
                    ed.selection = Selection::Effect(e.id);
                }
            }
        }
        // Pressing and dragging on empty space in the track paints a new
        // effect: from the playhead while the video plays, from the pointer
        // while it is paused.
        if bg.drag_started()
            && drag.is_none()
            && let Some(pos) = pointer
            && effect_row.contains(pos)
        {
            let at = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let start = if ed.is_playing() { ed.source_at_playhead() } else { map.to_source(pb_of(at.x)) }.clamp(0.0, dur_src);
            let mut draft = ed.project.clone();
            let id = draft.timeline.new_id();
            draft.timeline.effects.push(Effect { id, start, end: painted_end(start, start, dur_src), ..Default::default() });
            ed.commit(draft);
            ed.selection = Selection::Effect(id);
            drag = Some(Drag::NewEffect { id, start });
        }

        // Layout track.
        if has_cam {
            let layouts = ed.project.timeline.layouts.clone();
            for l in &layouts {
                let (a, b) = (map.to_playback_nearest(l.start), map.to_playback_nearest(l.end));
                if b <= a {
                    continue;
                }
                let r = Rect::from_min_max(pos2(x_of(a) + 1.0, layout_row.top()), pos2(x_of(b) - 1.0, layout_row.bottom()));
                let selected = ed.selection == Selection::Layout(l.id);
                painter.rect_filled(r, CornerRadius::same(6), layout_color().gamma_multiply(if selected { 0.95 } else { 0.7 }));
                if selected {
                    painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(2.0, Color32::WHITE), egui::StrokeKind::Inside);
                }
                let label = match l.kind {
                    LayoutKind::Both => "Screen + camera",
                    LayoutKind::ScreenOnly => "Screen only",
                    LayoutKind::CameraOnly => "Camera only",
                };
                painter.text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, label, theme::semibold(11.0), Color32::WHITE);
                if let Some(pos) = pointer
                    && r.expand(2.0).contains(pos)
                {
                    let part = if (pos.x - r.left()).abs() < EDGE {
                        Part::Left
                    } else if (pos.x - r.right()).abs() < EDGE {
                        Part::Right
                    } else {
                        Part::Body
                    };
                    if bg.drag_started() && drag.is_none() {
                        drag = Some(Drag::Layout { id: l.id, part, grab_src: map.to_source(pb_of(pos.x)), orig: (l.start, l.end) });
                        ed.selection = Selection::Layout(l.id);
                    } else if bg.clicked() {
                        ed.selection = Selection::Layout(l.id);
                    }
                }
            }
        }

        // Clicking the ruler or empty space moves the playhead.
        if (bg.clicked() || bg.drag_started())
            && drag.is_none()
            && let Some(pos) = pointer
        {
            let on_item = clip_row.contains(pos) || ((zoom_row.contains(pos) || mask_row.contains(pos) || effect_row.contains(pos)) && ed.selection != Selection::None);
            if ruler.contains(pos) || !on_item {
                ed.seek(pb_of(pos.x));
                if bg.drag_started() {
                    drag = Some(Drag::Playhead);
                }
                if !ruler.contains(pos) && bg.clicked() && !zoom_row.contains(pos) && !mask_row.contains(pos) && !effect_row.contains(pos) && !clip_row.contains(pos) {
                    ed.selection = Selection::None;
                }
            }
        }

        // Apply an ongoing drag.
        if let (Some(d), Some(pos)) = (drag, pointer)
            && bg.dragged()
        {
            let src = map.to_source(pb_of(pos.x).clamp(0.0, map.duration()));
            match d {
                Drag::Playhead => ed.seek(pb_of(pos.x)),
                Drag::SliceEdge { id, left, orig, grab_x } => {
                    let mut draft = ed.project.clone();
                    let slices = &mut draft.timeline.slices;
                    if let Some(i) = slices.iter().position(|s| s.id == id) {
                        let lo = if i > 0 { slices[i - 1].end } else { 0.0 };
                        let hi = if i + 1 < slices.len() { slices[i + 1].start } else { dur_src };
                        let s = &mut slices[i];
                        let target = edge_after_drag(orig, pos.x - grab_x, px_per_ms as f64, s.speed);
                        if left {
                            s.start = clamp_or(target, lo, s.end - 200.0, s.start);
                        } else {
                            s.end = clamp_or(target, s.start + 200.0, hi, s.end);
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Zoom { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(z) = draft.timeline.zooms.iter_mut().find(|z| z.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                z.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                z.end = z.start + len;
                            }
                            Part::Left => z.start = clamp_or(orig.0 + delta, 0.0, orig.1 - Zoom::MIN_LEN_MS / 2.0, orig.0),
                            Part::Right => z.end = clamp_or(orig.1 + delta, orig.0 + Zoom::MIN_LEN_MS / 2.0, dur_src, orig.1),
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Mask { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(m) = draft.timeline.masks.iter_mut().find(|m| m.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                m.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                m.end = m.start + len;
                            }
                            Part::Left => m.start = clamp_or(orig.0 + delta, 0.0, orig.1 - Mask::MIN_LEN_MS, orig.0),
                            Part::Right => m.end = clamp_or(orig.1 + delta, orig.0 + Mask::MIN_LEN_MS, dur_src, orig.1),
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Effect { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(e) = draft.timeline.effects.iter_mut().find(|e| e.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                e.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                e.end = e.start + len;
                            }
                            Part::Left => e.start = clamp_or(orig.0 + delta, 0.0, orig.1 - Effect::MIN_LEN_MS, orig.0),
                            Part::Right => e.end = clamp_or(orig.1 + delta, orig.0 + Effect::MIN_LEN_MS, dur_src, orig.1),
                        }
                    }
                    ed.commit(draft);
                }
                Drag::NewEffect { id, start } => {
                    let end = if ed.is_playing() { ed.source_at_playhead() } else { src };
                    let mut draft = ed.project.clone();
                    if let Some(e) = draft.timeline.effects.iter_mut().find(|e| e.id == id) {
                        e.end = painted_end(start, end, dur_src);
                    }
                    ed.commit(draft);
                }
                Drag::Layout { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(l) = draft.timeline.layouts.iter_mut().find(|l| l.id == id) {
                        match part {
                            Part::Body => {
                                let len = orig.1 - orig.0;
                                l.start = (orig.0 + delta).clamp(0.0, (dur_src - len).max(0.0));
                                l.end = l.start + len;
                            }
                            Part::Left => l.start = clamp_or(orig.0 + delta, 0.0, orig.1 - 300.0, orig.0),
                            Part::Right => l.end = clamp_or(orig.1 + delta, orig.0 + 300.0, dur_src, orig.1),
                        }
                    }
                    ed.commit(draft);
                }
            }
        }
        if bg.drag_stopped() {
            drag = None;
        }
        ui.data_mut(|d| match drag {
            Some(v) => {
                d.insert_temp(drag_id, v);
            }
            None => {
                d.remove::<Drag>(drag_id);
            }
        });
        if let Some(c) = hover_cursor {
            ui.ctx().set_cursor_icon(c);
        }

        // Track labels and playhead.
        for (r, name) in [(clip_row, "Clips"), (zoom_row, "Zoom"), (mask_row, "Masks"), (effect_row, "Effects")].into_iter().chain(has_cam.then_some((layout_row, "Camera"))) {
            if map.links.is_empty() || name != "Clips" {
                painter.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, name, egui::FontId::proportional(10.0), p.tertiary);
            }
        }
        let x = x_of(ed.playhead);
        // A thin line with a round handle, in the same blue-grey as the reference.
        let (line, handle) = (Color32::from_rgb(0x3c, 0x40, 0x50), Color32::from_rgb(0x29, 0x2c, 0x38));
        painter.line_segment([pos2(x, rect.top() + 6.0), pos2(x, rect.bottom())], Stroke::new(2.0, line));
        painter.circle(pos2(x, rect.top() + 6.0), 6.0, handle, Stroke::new(1.0, line));
        if ed.is_playing() {
            ui.scroll_to_rect(Rect::from_center_size(pos2(x, rect.center().y), vec2(200.0, 10.0)), None);
        }
    });
    let _ = RichText::new("");
}

#[cfg(test)]
mod tests {
    #[test]
    fn clamp_or_never_panics() {
        assert_eq!(super::clamp_or(5.0, 0.0, 3.0, 9.0), 3.0);
        assert_eq!(super::clamp_or(5.0, 4.0, 3.0, 9.0), 9.0);
    }

    #[test]
    fn a_painted_effect_follows_the_playhead_within_the_recording() {
        // Starts at its minimum length, then grows as the playhead moves on.
        assert_eq!(super::painted_end(1000.0, 1000.0, 10_000.0), 1200.0);
        assert_eq!(super::painted_end(1000.0, 4000.0, 10_000.0), 4000.0);
        // Dragging back doesn't make it shorter than the minimum or end before it starts.
        assert_eq!(super::painted_end(1000.0, 500.0, 10_000.0), 1200.0);
        // Never past the end of the recording, and at the very end it is as long as it can be.
        assert_eq!(super::painted_end(1000.0, 99_000.0, 10_000.0), 10_000.0);
        assert_eq!(super::painted_end(9_950.0, 9_950.0, 10_000.0), 10_000.0);
        assert_eq!(super::painted_end(10_000.0, 10_000.0, 10_000.0), 10_000.0);
    }

    #[test]
    fn an_edge_drag_depends_only_on_the_grab_and_the_pointer() {
        let near = |a: f64, b: f64| (a - b).abs() < 1e-6;
        // 40 px at 0.1 px/ms is 400 ms, however many times it is evaluated.
        let once = super::edge_after_drag(2000.0, 40.0, 0.1, 1.0);
        assert!(near(once, 2400.0));
        assert!(near(super::edge_after_drag(2000.0, 40.0, 0.1, 1.0), once));
        // Left of the grab moves the edge earlier.
        assert!(near(super::edge_after_drag(2000.0, -10.0, 0.1, 1.0), 1900.0));
        // A clip at 2x covers twice the source time per pixel.
        assert!(near(super::edge_after_drag(2000.0, 10.0, 0.1, 2.0), 2200.0));
        // No movement leaves the edge where it was grabbed; a bad speed counts as 1x.
        assert!(near(super::edge_after_drag(2000.0, 0.0, 0.1, 1.0), 2000.0));
        assert!(near(super::edge_after_drag(2000.0, 10.0, 0.1, 0.0), 2100.0));
    }

    #[test]
    fn overlapping_ranges_get_separate_lanes() {
        assert_eq!(super::lanes(&[(0.0, 5.0), (1.0, 2.0), (5.0, 6.0), (1.5, 7.0)]), vec![0, 1, 0, 2]);
    }
}
