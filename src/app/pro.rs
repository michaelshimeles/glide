//! "DaisyCap Pro": a tiny badge that marks a Pro setting. Hovering or
//! clicking it shows a toolbox with a link to the subscription page. The
//! link lives here, so other pages can point at the same URL.

use egui::{Color32, CornerRadius, Key, PopupCloseBehavior, Response, RichText, Sense, Stroke, StrokeKind, Ui, vec2};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSURL;

use super::theme::{self, pal};
use super::widgets;
use crate::util::ns;

/// Where to subscribe.
pub const SUBSCRIBE_URL: &str = "https://jaeswift.xyz/daisycap/subscription";

/// Open the subscription page in the default browser.
pub fn open_subscribe() {
    if let Some(url) = NSURL::URLWithString(&ns(SUBSCRIBE_URL)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// A small "PRO" chip. The toolbox shows while it is hovered or after a click.
pub fn badge(ui: &mut Ui) -> Response {
    let font = theme::semibold(9.0);
    let accent = theme::accent();
    let galley = ui.painter().layout_no_wrap("PRO".into(), font, accent);
    let size = vec2(galley.size().x + 9.0, 14.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(7), theme::accent().gamma_multiply(0.22));
    } else {
        ui.painter().rect_filled(rect, CornerRadius::same(7), Color32::TRANSPARENT);
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(0.75, accent.gamma_multiply(0.7)), StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, accent);
    resp
}

/// A row with a Pro badge after the label; returns the badge's response.
pub fn labelled(ui: &mut Ui, label: &str, pro: bool) -> Option<Response> {
    ui.label(label);
    if pro {
        let r = badge(ui);
        toolbox(ui, &r);
        return Some(r);
    }
    None
}

/// The toolbox: what Pro means and where to subscribe.
fn toolbox(ui: &mut Ui, badge: &Response) {
    let id = badge.id.with("pro-toolbox");
    let mut open = ui.ctx().memory(|m| m.data.get_temp::<bool>(id)).unwrap_or(false);
    if badge.hovered() || badge.clicked() {
        open = true;
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        open = false;
    }
    if open {
        let inner = egui::Popup::from_response(badge)
            .open(true)
            .gap(0.0)
            .width(238.0)
            .close_behavior(PopupCloseBehavior::IgnoreClicks)
            .show(|ui| content(ui));
        // Close once the pointer is off both the badge and the toolbox.
        if !badge.hovered() && !inner.is_some_and(|i| i.response.contains_pointer()) {
            open = false;
        }
    }
    ui.ctx().memory_mut(|m| m.data.insert_temp(id, open));
}

fn content(ui: &mut Ui) {
    ui.label(RichText::new("DaisyCap Pro").font(theme::semibold(theme::BODY)));
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new("This is a Pro feature. Subscribe by clicking ").size(theme::SECONDARY).color(pal(ui).secondary));
        if ui.link(RichText::new("here").size(theme::SECONDARY)).clicked() {
            open_subscribe();
        }
        ui.label(RichText::new(".").size(theme::SECONDARY).color(pal(ui).secondary));
    });
}

/// A settings row with a Pro badge: label + badge on the left, switch right.
pub fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        labelled(ui, label, true);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = widgets::switch(ui, on).changed();
        });
    });
    changed
}
