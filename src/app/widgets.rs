//! Small UI building blocks: sections, rows, sliders, switches and chips.

use egui::{Color32, CornerRadius, Rect, Response, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use std::ops::RangeInclusive;

use super::theme::{self, pal};

/// A titled inspector section.
pub fn section(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).font(theme::semibold(theme::BODY + 0.5)).color(pal(ui).text));
    ui.add_space(8.0);
    add(ui);
    ui.add_space(10.0);
    ui.separator();
}

/// Label on the left, control on the right.
pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// A labelled slider: the label and its value on one line, the track under
/// it. Returns true when changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f64, range: RangeInclusive<f64>, suffix: &str) -> bool {
    slider_stepped(ui, label, v, range, suffix, None, true)
}

/// `slider` that snaps to multiples of `step` and can be disabled.
pub fn slider_stepped(ui: &mut Ui, label: &str, v: &mut f64, range: RangeInclusive<f64>, suffix: &str, step: Option<f64>, enabled: bool) -> bool {
    let p = pal(ui);
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{}{suffix}", fmt_value(*v))).color(p.secondary));
            });
        });
        changed = track(ui, v, range, false, step).changed();
        ui.add_space(2.0);
    });
    changed
}

/// A value with at most two decimals and no trailing zeros.
fn fmt_value(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The slider track across the available width. `log` spaces the values
/// logarithmically (the range must be positive); `step` snaps to multiples.
pub fn track(ui: &mut Ui, v: &mut f64, range: RangeInclusive<f64>, log: bool, step: Option<f64>) -> Response {
    let width = ui.available_width().max(40.0);
    track_w(ui, width, v, range, log, step)
}

/// `track` with a given width.
pub fn track_w(ui: &mut Ui, width: f32, v: &mut f64, range: RangeInclusive<f64>, log: bool, step: Option<f64>) -> Response {
    const KNOB: f32 = 7.0;
    let (lo, hi) = (*range.start(), *range.end());
    let scale = |x: f64| if log { x.ln() } else { x };
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let (x0, x1) = (rect.left() + KNOB, rect.right() - KNOB);
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.is_pointer_button_down_on()) {
        let t = ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0) as f64;
        let mut nv = if log { (scale(lo) + t * (scale(hi) - scale(lo))).exp() } else { lo + t * (hi - lo) };
        if let Some(s) = step {
            nv = (nv / s).round() * s;
        }
        let nv = nv.clamp(lo, hi);
        if nv != *v {
            *v = nv;
            resp.mark_changed();
        }
    }
    let t = if hi > lo { ((scale(*v) - scale(lo)) / (scale(hi) - scale(lo))).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let knob = pos2(x0 + (x1 - x0) * t, rect.center().y);
    let fade = if ui.is_enabled() { 1.0 } else { 0.35 };
    let rail = Rect::from_center_size(rect.center(), vec2(rect.width(), 4.0));
    let painter = ui.painter();
    painter.rect_filled(rail, CornerRadius::same(2), Color32::from_rgb(0x29, 0x2b, 0x2f));
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rail.left_top(), theme::INDIGO),
        (pos2(knob.x, rail.top()), theme::VIOLET),
        (pos2(knob.x, rail.bottom()), theme::VIOLET),
        (rail.left_bottom(), theme::INDIGO),
    ] {
        mesh.colored_vertex(pos, color.gamma_multiply(fade));
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    painter.circle_filled(knob, if resp.hovered() || resp.dragged() { KNOB + 1.0 } else { KNOB }, theme::ACCENT.gamma_multiply(fade));
    resp
}

/// macOS-style switch; returns true when toggled.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(30.0, 18.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = pal(ui);
    let bg = lerp_color(Color32::from_rgb(0x2a, 0x2c, 0x31), theme::VIOLET, t);
    ui.painter().rect_filled(rect, CornerRadius::same(9), bg);
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    ui.painter().circle(egui::pos2(x, rect.center().y), 7.0, Color32::WHITE, Stroke::new(0.5, p.control_border));
    resp
}

/// Label + switch on one row.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    row(ui, label, |ui| changed = switch(ui, on).changed());
    changed
}

/// `over` drawn on `base`.
fn blend_over(base: Color32, over: Color32) -> Color32 {
    theme::blend(base, over)
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Segmented control. Returns true when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    let p = pal(ui);
    egui::Frame::new()
        .fill(p.control)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding.x = 7.0;
            // Inside right-aligned rows items flow right to left; add them in
            // reverse so they still read left to right.
            let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
            ui.horizontal(|ui| {
                let order: Vec<&(T, &str)> = if rtl { options.iter().rev().collect() } else { options.iter().collect() };
                for (v, label) in order {
                    let sel = *value == *v;
                    let text = RichText::new(*label).size(theme::SECONDARY + 1.0).color(p.text);
                    let b = egui::Button::new(text)
                        .fill(if sel { p.selected } else { Color32::TRANSPARENT })
                        .stroke(if sel { Stroke::new(1.0, Color32::from_white_alpha(28)) } else { Stroke::NONE })
                        .corner_radius(CornerRadius::same(8))
                        .min_size(vec2(0.0, 24.0));
                    if ui.add(b).clicked() && !sel {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Primary (accent) button.
pub fn primary(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).font(theme::semibold(theme::BODY)))
            .fill(theme::VIOLET)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(9))
            .min_size(vec2(0.0, 28.0)),
    )
}

/// A square icon button for rails and toolbars. A selected one is lavender.
pub fn glyph_button(ui: &mut Ui, g: super::glyph::Glyph, size: f32, selected: bool, enabled: bool) -> Response {
    let side = size + 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(side, side), if enabled { Sense::click() } else { Sense::hover() });
    let p = pal(ui);
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(10), if resp.is_pointer_button_down_on() { p.pressed } else { p.hover });
    }
    let color = if !enabled { p.tertiary } else if selected { theme::ACCENT } else { p.text };
    super::glyph::paint(ui.painter(), rect.center(), size, g, color);
    resp
}

/// A pill-shaped toolbar button: an optional icon, a label and an optional
/// chevron. A selected one has a lavender label and a border.
pub fn pill(ui: &mut Ui, icon: Option<super::glyph::Glyph>, text: &str, chevron: bool, selected: bool) -> Response {
    let p = pal(ui);
    let color = if selected { Color32::from_rgb(0xa3, 0x98, 0xe5) } else { p.text };
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(theme::BODY + 0.5), color);
    let icon_w = if icon.is_some() { 24.0 } else { 0.0 };
    let chevron_w = if chevron { 18.0 } else { 0.0 };
    let size = vec2(24.0 + icon_w + galley.size().x + chevron_w, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(10), Color32::from_rgb(0x12, 0x14, 0x16));
        ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, Color32::from_white_alpha(30)), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(10), p.hover);
    }
    let mut x = rect.left() + 12.0;
    if let Some(g) = icon {
        super::glyph::paint(ui.painter(), egui::pos2(x + 8.0, rect.center().y), 16.0, g, color);
        x += icon_w;
    }
    let text_pos = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    let end = x + galley.size().x;
    ui.painter().galley(text_pos, galley, color);
    if chevron {
        super::glyph::paint(ui.painter(), egui::pos2(end + 10.0, rect.center().y), 14.0, super::glyph::Glyph::Chevron, p.secondary);
    }
    resp
}

/// Big round record button.
pub fn record_button(ui: &mut Ui, size: f32, recording: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hover = resp.hovered();
    let c = rect.center();
    ui.painter().circle_filled(c, size / 2.0, if hover { Color32::from_rgb(255, 80, 70) } else { Color32::from_rgb(255, 59, 48) });
    if recording {
        let s = size * 0.28;
        ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(s, s)), CornerRadius::same(3), Color32::WHITE);
    } else {
        ui.painter().circle_filled(c, size * 0.18, Color32::WHITE);
    }
    resp
}

/// sRGB colour picker on a project colour.
pub fn color_edit(ui: &mut Ui, c: &mut crate::project::model::Color) -> bool {
    let mut col = Color32::from_rgba_unmultiplied(c.0[0], c.0[1], c.0[2], c.0[3]);
    let changed = egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed();
    if changed {
        let [r, g, b, a] = col.to_srgba_unmultiplied();
        c.0 = [r, g, b, a];
    }
    changed
}

/// Small secondary caption text.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(theme::SECONDARY).color(pal(ui).secondary));
}

/// Format milliseconds as m:ss.t
pub fn fmt_time(ms: f64) -> String {
    let s = (ms / 1000.0).max(0.0);
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Undo,
    Redo,
    Restart,
    Close,
    Pause,
    Play,
}

/// Small borderless toolbar button with a vector icon (Inter has no arrows).
pub fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = pal(ui);
    if enabled && resp.hovered() {
        let fill = if resp.is_pointer_button_down_on() { p.pressed } else { p.hover };
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    }
    let color = if enabled { p.text } else { p.tertiary };
    let stroke = Stroke::new(1.5, color);
    let c = rect.center();
    let painter = ui.painter();
    let arc = |from: f32, to: f32, r: f32, cy: f32| -> Vec<egui::Pos2> {
        (0..=16)
            .map(|i| {
                let a = from + (to - from) * i as f32 / 16.0;
                egui::pos2(c.x + r * a.cos(), cy - r * a.sin())
            })
            .collect()
    };
    use std::f32::consts::PI;
    match icon {
        Icon::Undo | Icon::Redo => {
            let flip = if icon == Icon::Redo { -1.0 } else { 1.0 };
            let mut pts = vec![egui::pos2(c.x + flip * 5.5, c.y + 5.0)];
            pts.extend(arc(0.0, PI, 5.5, c.y + 1.5).into_iter().map(|q| egui::pos2(c.x + (q.x - c.x) * flip, q.y)));
            let end = *pts.last().unwrap();
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([end, egui::pos2(end.x - 3.0, end.y - 3.0)], stroke);
            painter.line_segment([end, egui::pos2(end.x + 3.0, end.y - 3.0)], stroke);
        }
        Icon::Restart => {
            let pts = arc(PI * 0.35, PI * 2.15, 6.0, c.y);
            let end = pts[0];
            painter.add(egui::Shape::line(pts, stroke));
            painter.line_segment([end, egui::pos2(end.x - 3.5, end.y - 0.5)], stroke);
            painter.line_segment([end, egui::pos2(end.x + 0.5, end.y + 3.5)], stroke);
        }
        Icon::Close => {
            let d = 4.5;
            painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
            painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
        }
        Icon::Pause => {
            for dx in [-3.0, 3.0] {
                painter.rect_filled(egui::Rect::from_center_size(c + vec2(dx, 0.0), vec2(3.0, 11.0)), CornerRadius::same(1), color);
            }
        }
        Icon::Play => {
            let pts = vec![c + vec2(-4.0, -6.0), c + vec2(6.0, 0.0), c + vec2(-4.0, 6.0)];
            painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
    resp
}

/// Outline of a camera bubble filling `rect`, clockwise: a rounded rect with
/// corner radius `radius`, or a superellipse when `n` > 0.
pub fn bubble_outline(rect: egui::Rect, radius: f32, n: f32) -> Vec<egui::Pos2> {
    let c = rect.center();
    let (hw, hh) = (rect.width() / 2.0, rect.height() / 2.0);
    if n > 0.0 {
        return (0..128)
            .map(|i| {
                let t = i as f32 / 128.0 * std::f32::consts::TAU;
                let (ct, st) = (t.cos(), t.sin());
                c + vec2(ct.signum() * ct.abs().powf(2.0 / n) * hw, st.signum() * st.abs().powf(2.0 / n) * hh)
            })
            .collect();
    }
    let r = radius.clamp(0.0, hw.min(hh));
    let mut pts = Vec::with_capacity(4 * 17);
    // Corner centres, starting top right and going clockwise (y down).
    for (k, (sx, sy)) in [(1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0)].into_iter().enumerate() {
        let cc = c + vec2(sx * (hw - r), sy * (hh - r));
        let a0 = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::FRAC_PI_2;
        for j in 0..=16 {
            let a = a0 + j as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
            pts.push(cc + vec2(a.cos(), a.sin()) * r);
        }
    }
    pts
}

/// A small icon of a camera shape (with the current roundness).
fn shape_icon(ui: &Ui, rect: egui::Rect, c: &crate::project::model::CameraStyle, color: Color32) {
    let (w, h) = if c.shape.is_square() { (16.0, 16.0) } else { (22.0, 13.0) };
    let r = egui::Rect::from_center_size(rect.center(), vec2(w, h));
    let pts = bubble_outline(r, c.radius(w as f64, h as f64) as f32, c.squircle_exponent() as f32);
    ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.5, color)));
}

/// The camera bubble's look and position, as in Rec: shape, size,
/// roundness, border, corner, shadow and mirror. Returns true when changed.
pub fn camera_look(ui: &mut Ui, c: &mut crate::project::model::CameraStyle) -> bool {
    use crate::project::model::{CameraShape, CameraStyle};
    let before = *c;
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let bw = ((ui.available_width() - 16.0) / 5.0).max(30.0);
        for shape in CameraShape::ALL {
            let sel = c.shape == shape;
            let (rect, resp) = ui.allocate_exact_size(vec2(bw, 30.0), Sense::click());
            let fill = if sel { p.selected } else if resp.hovered() { blend_over(p.control, p.hover) } else { p.control };
            ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
            if sel {
                ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
            }
            shape_icon(ui, rect, &CameraStyle { shape, ..*c }, if sel { theme::ACCENT } else { p.text });
            if resp.on_hover_text(shape.label()).clicked() {
                c.shape = shape;
            }
        }
    });
    ui.add_space(4.0);
    let pct = |ui: &mut Ui, label: &str, v: &mut f64, range: RangeInclusive<f64>, scale: f64, enabled: bool| {
        let mut x = *v * scale;
        let r = (*range.start() * scale)..=(*range.end() * scale);
        if slider_stepped(ui, label, &mut x, r, " %", Some(1.0), enabled) {
            *v = x / scale;
        }
    };
    pct(ui, "Size", &mut c.size, 0.1..=0.8, 100.0, true);
    // Roundness 0..0.5 of the short side shows as 0..100 %.
    pct(ui, "Roundness", &mut c.roundness, 0.0..=0.5, 200.0, c.shape.uses_roundness());
    slider_stepped(ui, "Border", &mut c.border, 0.0..=20.0, " px", Some(1.0), true);
    row(ui, "Border colour", |ui| {
        ui.add_enabled_ui(c.border > 0.0, |ui| color_edit(ui, &mut c.border_color));
    });
    row(ui, "Position", |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        // Right to left: the last corner is drawn first.
        for (label, tip, pos) in [("↘", "Bottom right", (1.0, 1.0)), ("↙", "Bottom left", (0.0, 1.0)), ("↗", "Top right", (1.0, 0.0)), ("↖", "Top left", (0.0, 0.0))] {
            let sel = (c.position.x, c.position.y) == pos;
            if ui.add(egui::Button::selectable(sel, label).min_size(vec2(26.0, 22.0))).on_hover_text(tip).clicked() {
                c.position = crate::engine::geom::v2(pos.0, pos.1);
            }
        }
    });
    toggle_row(ui, "Shadow", &mut c.shadow);
    toggle_row(ui, "Mirror", &mut c.mirror);
    *c != before
}
