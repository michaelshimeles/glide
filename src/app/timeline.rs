//! Timeline: ruler, clip track (slices), zoom track, mask track and camera
//! layout track.
//! Everything is drawn in playback time; edits are stored in source time.

use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use super::editor::{Editor, Selection};
use super::theme::{self, pal};
use crate::project::model::{LayoutKind, LayoutRange, Mask, MaskKind, Zoom, ZoomMode};

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
    SliceEdge { id: u64, left: bool, orig: (f64, f64), grab_pb: f64 },
    Zoom { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Layout { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Mask { id: u64, part: Part, grab_src: f64, orig: (f64, f64) },
    Playhead,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Body,
    Left,
    Right,
}

fn zoom_color() -> Color32 {
    Color32::from_rgb(0x8b, 0x5c, 0xf6)
}
fn clip_color() -> Color32 {
    Color32::from_rgb(0xf5, 0x9e, 0x0b)
}
fn layout_color() -> Color32 {
    Color32::from_rgb(0x10, 0xb9, 0x81)
}
fn mask_color() -> Color32 {
    Color32::from_rgb(0xec, 0x48, 0x99)
}

/// Which part of the block spanning `left..right` the pointer at `x` is on.
/// The edge zones are `EDGE` wide but never more than a third of the block,
/// so a short block keeps a body to grab.
fn part_at(x: f32, left: f32, right: f32) -> Part {
    let edge = EDGE.min((right - left) / 3.0);
    if (x - left).abs() < edge {
        Part::Left
    } else if (x - right).abs() < edge {
        Part::Right
    } else {
        Part::Body
    }
}

/// `v.clamp(lo, hi)`, or `keep` when the range is empty (an item already
/// shorter than the minimum length can't be resized further; `clamp` would
/// panic).
fn clamp_or(v: f64, lo: f64, hi: f64, keep: f64) -> f64 {
    if lo <= hi { v.clamp(lo, hi) } else { keep }
}

/// Shortest slice, in source ms.
const MIN_SLICE_MS: f64 = 200.0;

/// Where a slice edge sits after a drag. The edge moves by the pointer's travel
/// since the grab (playback ms, scaled by the slice speed to source ms) from
/// where it was when the drag began, so repeating the same pointer position
/// gives the same edge. `orig` is the slice's (start, end) at that moment, and
/// `lo`/`hi` are the neighbouring slices' bounds. Dragging outward extends
/// into the cut region.
fn dragged_edge(left: bool, orig: (f64, f64), speed: f64, grab_pb: f64, pointer_pb: f64, lo: f64, hi: f64) -> f64 {
    let delta = (pointer_pb - grab_pb) * speed;
    if left {
        clamp_or(orig.0 + delta, lo, orig.1 - MIN_SLICE_MS, orig.0)
    } else {
        clamp_or(orig.1 + delta, orig.0 + MIN_SLICE_MS, hi, orig.1)
    }
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
    let (rect, _) = ui.allocate_exact_size(vec2(width, RULER_H + 3.0 * (TRACK_H + GAP) + 4.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let track_bg = if ui.visuals().dark_mode { Color32::from_white_alpha(8) } else { Color32::from_black_alpha(8) };
    for (i, name) in ["Clips", "Zoom", "Masks"].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left(), rect.top() + RULER_H + 2.0 + i as f32 * (TRACK_H + GAP)), vec2(width, TRACK_H));
        painter.rect_filled(r, CornerRadius::same(6), track_bg);
        painter.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, name, egui::FontId::proportional(10.0), p.tertiary);
        if i == 0 {
            painter.text(r.center(), egui::Align2::CENTER_CENTER, "Your recording opens here when you stop", egui::FontId::proportional(12.0), p.secondary);
        }
    }
}

pub fn ui(ui: &mut Ui, ed: &mut Editor) {
    let p = pal(ui);
    let has_cam = ed.rec.meta.camera.is_some();
    let tracks = if has_cam { 4 } else { 3 };
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
            painter.text(pos2(x + 3.0, ruler.top() + 1.0), egui::Align2::LEFT_TOP, super::widgets::fmt_time(t * 1000.0), egui::FontId::proportional(10.0), p.secondary);
            t += step_s;
        }

        let track = |i: usize| {
            let below_warnings = if i >= 2 { extra } else { 0.0 };
            Rect::from_min_size(pos2(rect.left(), rect.top() + RULER_H + 2.0 + i as f32 * (TRACK_H + GAP) + below_warnings), vec2(width, TRACK_H))
        };
        let clip_row = track(0);
        let zoom_row = track(1);
        let mask_row = track(2);
        let layout_row = track(3);
        let track_bg = if ui.visuals().dark_mode { Color32::from_white_alpha(8) } else { Color32::from_black_alpha(8) };
        for r in [clip_row, zoom_row, mask_row].into_iter().chain(has_cam.then_some(layout_row)) {
            painter.rect_filled(r, CornerRadius::same(6), track_bg);
        }

        let pointer = ui.input(|i| i.pointer.interact_pos());
        let press_origin = ui.input(|i| i.pointer.press_origin());
        let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
        let mut hover_cursor = None;

        // Clip track: one block per kept slice.
        for l in &map.links {
            let r = Rect::from_min_max(pos2(x_of(l.pb_start) + 1.0, clip_row.top()), pos2(x_of(l.pb_end) - 1.0, clip_row.bottom()));
            let slice = ed.project.timeline.slices.get(l.slice).copied();
            let id = slice.map_or(0, |s| s.id);
            let selected = ed.selection == Selection::Slice(id);
            let c = clip_color();
            painter.rect_filled(r, CornerRadius::same(6), c.gamma_multiply(if selected { 0.95 } else { 0.7 }));
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
            // egui reports a drag only after the pointer has travelled a few
            // pixels, so the drag starts from where the press happened.
            let at = if bg.drag_started() { press_origin.or(pointer) } else { pointer };
            if let Some(pos) = at
                && r.expand(2.0).contains(pos)
                && let Some(s) = slice
            {
                let near_l = (pos.x - r.left()).abs() < EDGE;
                let near_r = (pos.x - r.right()).abs() < EDGE;
                if near_l || near_r {
                    hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                }
                if bg.drag_started() && drag.is_none() && (near_l || near_r) {
                    drag = Some(Drag::SliceEdge { id, left: near_l, orig: (s.start, s.end), grab_pb: pb_of(pos.x) });
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
            // egui reports a drag only after the pointer has travelled a few
            // pixels, so where it started decides which part was grabbed (by
            // then the pointer has left a narrow edge zone).
            let at = if bg.drag_started() { press_origin.or(pointer) } else { pointer };
            if let Some(pos) = at
                && r.expand(2.0).contains(pos)
            {
                let part = part_at(pos.x, r.left(), r.right());
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
            let duration = ed.rec.meta.duration_ms;
            ed.edit(|p| {
                p.timeline.add_zoom(src, scale, duration);
            });
        }

        // Mask track: overlapping masks stack in lanes.
        let masks = ed.project.timeline.masks.clone();
        let lanes = lanes(&masks.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>());
        let lane_count = lanes.iter().copied().max().map_or(1, |n| n + 1) as f32;
        for (m, lane) in masks.iter().zip(&lanes) {
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
            let duration = ed.rec.meta.duration_ms;
            let mut new_id = 0;
            ed.edit(|p| new_id = p.timeline.add_mask(src, duration));
            ed.selection = Selection::Mask(new_id);
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
            let on_item = clip_row.contains(pos) || ((zoom_row.contains(pos) || mask_row.contains(pos)) && ed.selection != Selection::None);
            if ruler.contains(pos) || !on_item {
                ed.seek(pb_of(pos.x));
                if bg.drag_started() {
                    drag = Some(Drag::Playhead);
                }
                if !ruler.contains(pos) && bg.clicked() && !zoom_row.contains(pos) && !mask_row.contains(pos) && !clip_row.contains(pos) {
                    ed.selection = Selection::None;
                }
            }
        }

        // Apply an ongoing drag.
        if let (Some(d), Some(pos)) = (drag, pointer)
            && bg.dragged()
        {
            let src = map.to_source(pb_of(pos.x).clamp(0.0, map.duration()));
            let dur_src = ed.rec.meta.duration_ms;
            match d {
                Drag::Playhead => ed.seek(pb_of(pos.x)),
                Drag::SliceEdge { id, left, orig, grab_pb } => {
                    let mut draft = ed.project.clone();
                    let slices = &mut draft.timeline.slices;
                    if let Some(i) = slices.iter().position(|s| s.id == id) {
                        let lo = if i > 0 { slices[i - 1].end } else { 0.0 };
                        let hi = if i + 1 < slices.len() { slices[i + 1].start } else { dur_src };
                        let s = &mut slices[i];
                        let edge = dragged_edge(left, orig, s.speed, grab_pb, pb_of(pos.x), lo, hi);
                        if left {
                            s.start = edge;
                        } else {
                            s.end = edge;
                        }
                    }
                    ed.commit(draft);
                }
                Drag::Zoom { id, part, grab_src, orig } => {
                    let delta = src - grab_src;
                    let mut draft = ed.project.clone();
                    if let Some(z) = draft.timeline.zooms.iter_mut().find(|z| z.id == id) {
                        if part != Part::Body {
                            hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
                        }
                        // Always resize from where the drag began, not from the last frame.
                        let mut edited = Zoom { start: orig.0, end: orig.1, ..*z };
                        match part {
                            Part::Body => edited.move_to(orig.0 + delta, dur_src),
                            Part::Left => edited.trim_start(orig.0 + delta),
                            Part::Right => edited.trim_end(orig.1 + delta, dur_src),
                        }
                        (z.start, z.end) = (edited.start, edited.end);
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
                            Part::Left => l.start = clamp_or(orig.0 + delta, 0.0, orig.1 - LayoutRange::MIN_LEN_MS, orig.0),
                            Part::Right => l.end = clamp_or(orig.1 + delta, orig.0 + LayoutRange::MIN_LEN_MS, dur_src, orig.1),
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
        for (r, name) in [(clip_row, "Clips"), (zoom_row, "Zoom"), (mask_row, "Masks")].into_iter().chain(has_cam.then_some((layout_row, "Camera"))) {
            if map.links.is_empty() || name != "Clips" {
                painter.text(r.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, name, egui::FontId::proportional(10.0), p.tertiary);
            }
        }
        let x = x_of(ed.playhead);
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(2.0, theme::accent()));
        painter.add(egui::Shape::convex_polygon(
            vec![Pos2::new(x - 6.0, rect.top()), Pos2::new(x + 6.0, rect.top()), Pos2::new(x, rect.top() + 8.0)],
            theme::accent(),
            Stroke::NONE,
        ));
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
    fn edge_zones_shrink_on_short_blocks_so_the_body_stays_grabbable() {
        use super::Part::{Body, Left, Right};
        // Wide block: the full 7 px at each edge.
        assert!(matches!(super::part_at(103.0, 100.0, 300.0), Left));
        assert!(matches!(super::part_at(297.0, 100.0, 300.0), Right));
        assert!(matches!(super::part_at(200.0, 100.0, 300.0), Body));
        assert!(matches!(super::part_at(108.0, 100.0, 300.0), Body));
        // 12 px block: edge zones are 4 px, the middle is still the body.
        assert!(matches!(super::part_at(103.0, 100.0, 112.0), Left));
        assert!(matches!(super::part_at(106.0, 100.0, 112.0), Body));
        assert!(matches!(super::part_at(109.0, 100.0, 112.0), Right));
    }

    #[test]
    fn overlapping_ranges_get_separate_lanes() {
        assert_eq!(super::lanes(&[(0.0, 5.0), (1.0, 2.0), (5.0, 6.0), (1.5, 7.0)]), vec![0, 1, 0, 2]);
    }

    const ORIG: (f64, f64) = (1000.0, 5000.0);

    #[test]
    fn a_held_pointer_gives_the_same_edge_every_frame() {
        let first = super::dragged_edge(true, ORIG, 1.0, 1000.0, 1300.0, 0.0, 9000.0);
        for _ in 0..5 {
            assert_eq!(super::dragged_edge(true, ORIG, 1.0, 1000.0, 1300.0, 0.0, 9000.0), first);
        }
        assert_eq!(first, 1300.0);
    }

    #[test]
    fn the_left_edge_trims_in_and_extends_out() {
        // Inward (+) shortens the slice.
        assert_eq!(super::dragged_edge(true, ORIG, 1.0, 1000.0, 1500.0, 0.0, 9000.0), 1500.0);
        // Outward (-) lengthens it, down to the previous slice's end.
        assert_eq!(super::dragged_edge(true, ORIG, 1.0, 1000.0, 700.0, 0.0, 9000.0), 700.0);
        assert_eq!(super::dragged_edge(true, ORIG, 1.0, 1000.0, -5000.0, 400.0, 9000.0), 400.0);
        // Never closer than the minimum length to the end.
        assert_eq!(super::dragged_edge(true, ORIG, 1.0, 1000.0, 9000.0, 0.0, 9000.0), 5000.0 - super::MIN_SLICE_MS);
    }

    #[test]
    fn the_right_edge_trims_in_and_extends_out() {
        assert_eq!(super::dragged_edge(false, ORIG, 1.0, 5000.0, 4500.0, 0.0, 9000.0), 4500.0);
        assert_eq!(super::dragged_edge(false, ORIG, 1.0, 5000.0, 5600.0, 0.0, 9000.0), 5600.0);
        assert_eq!(super::dragged_edge(false, ORIG, 1.0, 5000.0, 99_000.0, 0.0, 6000.0), 6000.0);
        assert_eq!(super::dragged_edge(false, ORIG, 1.0, 5000.0, -9000.0, 0.0, 9000.0), 1000.0 + super::MIN_SLICE_MS);
    }

    #[test]
    fn the_slice_speed_scales_the_pointer_travel() {
        // A 2x slice covers twice the source time per playback ms.
        assert_eq!(super::dragged_edge(true, ORIG, 2.0, 1000.0, 1100.0, 0.0, 9000.0), 1200.0);
        assert_eq!(super::dragged_edge(false, ORIG, 0.5, 5000.0, 4800.0, 0.0, 9000.0), 4900.0);
    }

    #[test]
    fn an_empty_clamp_range_keeps_the_original_edge() {
        // A slice shorter than the minimum, squeezed by its neighbour, can't move that edge.
        let short = (1000.0, 1100.0);
        assert_eq!(super::dragged_edge(true, short, 1.0, 1000.0, 1050.0, 950.0, 9000.0), 1000.0);
        assert_eq!(super::dragged_edge(false, short, 1.0, 1100.0, 1050.0, 0.0, 1150.0), 1100.0);
        // Same for a long slice whose room to the neighbour is under the minimum.
        assert_eq!(super::dragged_edge(false, ORIG, 1.0, 5000.0, 5100.0, 0.0, 1100.0), 5000.0);
    }
}
