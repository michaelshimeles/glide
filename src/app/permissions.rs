//! The permissions checklist on the start page: what macOS lets Glide do,
//! what the current recording needs, and a button to fix each gap before
//! recording instead of an error halfway in.

use std::time::{Duration, Instant};

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};

use super::home::Settings;
use super::theme::{self, pal};
use super::widgets::{caption, primary};
use crate::capture::tcc::{self, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Screen,
    Mic,
    Camera,
    Keys,
    Accessibility,
}

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Kind::Screen => "Screen & System Audio Recording",
            Kind::Mic => "Microphone",
            Kind::Camera => "Camera",
            Kind::Keys => "Input Monitoring",
            Kind::Accessibility => "Accessibility",
        }
    }

    fn why(self, required: bool) -> &'static str {
        match (self, required) {
            (Kind::Screen, _) => "Shows your screen and the sound your Mac plays.",
            (Kind::Mic, true) => "Records your voice. Microphone is switched on for this recording.",
            (Kind::Mic, false) => "Only needed when you record your voice.",
            (Kind::Camera, true) => "Shows you in the camera bubble. Screen + Cam is selected.",
            (Kind::Camera, false) => "Only needed for Screen + Cam.",
            (Kind::Keys, _) => "Logs key presses so Glide can zoom while you type.",
            (Kind::Accessibility, _) => "Lets Glide resize the window you record.",
        }
    }

    /// The Privacy & Security pane in System Settings.
    fn pane(self) -> &'static str {
        match self {
            Kind::Screen => "Privacy_ScreenCapture",
            Kind::Mic => "Privacy_Microphone",
            Kind::Camera => "Privacy_Camera",
            Kind::Keys => "Privacy_ListenEvent",
            Kind::Accessibility => "Privacy_Accessibility",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: Kind,
    pub status: Status,
    /// Recording can't start without it.
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checklist {
    pub items: Vec<Item>,
}

impl Checklist {
    /// `status` is in the order of `Kind`'s variants. The microphone and
    /// camera only block recording when the current settings use them.
    pub fn new(status: [Status; 5], s: &Settings) -> Self {
        let kinds = [Kind::Screen, Kind::Mic, Kind::Camera, Kind::Keys, Kind::Accessibility];
        let items = kinds
            .into_iter()
            .zip(status)
            .map(|(kind, status)| Item {
                kind,
                status,
                required: match kind {
                    Kind::Screen => true,
                    Kind::Mic => s.mic,
                    Kind::Camera => s.camera,
                    Kind::Keys | Kind::Accessibility => false,
                },
            })
            .collect();
        Checklist { items }
    }

    pub fn read(s: &Settings) -> Self {
        let ax = if crate::capture::winsize::trusted(false) { Status::Granted } else { Status::Denied };
        Self::new([tcc::screen_status(), tcc::mic_status(), tcc::camera_status(), tcc::input_status(), ax], s)
    }

    pub fn ready(&self) -> bool {
        self.missing().next().is_none()
    }

    /// Required permissions that aren't granted.
    pub fn missing(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.required && i.status != Status::Granted)
    }

    pub fn status(&self, kind: Kind) -> Status {
        self.items.iter().find(|i| i.kind == kind).map_or(Status::Denied, |i| i.status)
    }
}

/// The checklist as the app shows it: re-read while the start page is up, so
/// a row turns green as soon as the toggle in System Settings flips.
pub struct State {
    pub list: Checklist,
    /// The checklist replaces the live preview.
    pub open: bool,
    polled: Instant,
}

impl State {
    pub fn new(s: &Settings) -> Self {
        let list = Checklist::read(s);
        State { open: !list.ready(), list, polled: Instant::now() }
    }

    /// A fixed checklist (headless snapshots), open when something's missing.
    pub fn fixed(list: Checklist) -> Self {
        State { open: !list.ready(), list, polled: Instant::now() }
    }

    /// Re-read at most once a second (or now, with `force`). Returns whether
    /// anything changed.
    pub fn refresh(&mut self, s: &Settings, force: bool) -> bool {
        if !force && self.polled.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.polled = Instant::now();
        let list = Checklist::read(s);
        let changed = list != self.list;
        self.list = list;
        changed
    }
}

/// What a click on the card asks for.
pub enum Event {
    Grant(Kind),
    Close,
}

/// Ask for `kind`; if macOS won't show a prompt (already answered, or a
/// permission that is only granted in System Settings), open its pane.
pub fn grant(kind: Kind) {
    let status = match kind {
        Kind::Mic => tcc::mic_status(),
        Kind::Camera => tcc::camera_status(),
        _ => Status::Denied,
    };
    std::thread::spawn(move || {
        match kind {
            Kind::Mic if status == Status::NotDetermined => tcc::request_av(false),
            Kind::Camera if status == Status::NotDetermined => tcc::request_av(true),
            Kind::Mic | Kind::Camera => tcc::open_settings(kind.pane()),
            Kind::Screen => {
                tcc::request_all(true, false, false, Duration::from_millis(0));
            }
            Kind::Keys => {
                tcc::request_input();
            }
            Kind::Accessibility => {
                crate::capture::winsize::trusted(true);
            }
        }
        // The system prompt is the answer for most first requests; if none
        // appeared and it's still not granted, point at the pane.
        if matches!(kind, Kind::Screen | Kind::Keys | Kind::Accessibility) {
            std::thread::sleep(Duration::from_millis(800));
            let granted = match kind {
                Kind::Screen => tcc::screen_status(),
                Kind::Keys => tcc::input_status(),
                _ => {
                    if crate::capture::winsize::trusted(false) { Status::Granted } else { Status::Denied }
                }
            } == Status::Granted;
            if !granted {
                tcc::open_settings(kind.pane());
            }
        }
    });
}

const GREEN: Color32 = Color32::from_rgb(0x34, 0xc7, 0x59);
const ORANGE: Color32 = Color32::from_rgb(0xff, 0x9f, 0x0a);

fn badge(ui: &mut Ui, status: Status, required: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    match status {
        Status::Granted => {
            p.circle_filled(c, 10.0, GREEN);
            let st = Stroke::new(2.0, Color32::WHITE);
            let a = c + vec2(-4.0, 0.5);
            let b = c + vec2(-1.2, 3.5);
            p.line_segment([a, b], st);
            p.line_segment([b, c + vec2(4.5, -3.5)], st);
        }
        _ if required => {
            p.circle_filled(c, 10.0, ORANGE);
            p.text(c, egui::Align2::CENTER_CENTER, "!", theme::semibold(13.0), Color32::WHITE);
        }
        _ => {
            p.circle_stroke(c, 9.5, Stroke::new(1.5, pal(ui).tertiary));
        }
    }
}

/// The checklist card, centred in `ui`. Returns what was clicked.
pub fn card(ui: &mut Ui, list: &Checklist) -> Option<Event> {
    let p = pal(ui);
    let mut event = None;
    let ready = list.ready();
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.5 - 250.0).max(8.0));
        ui.allocate_ui(vec2(460.0_f32.min(ui.available_width()), 0.0), |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(if ready { "You're all set" } else { "Let's get Glide ready" }).font(theme::semibold(22.0)).color(p.text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(if ready {
                        "Every permission this recording needs is on. Nothing will interrupt it."
                    } else {
                        "Allow these once, so nothing interrupts a recording halfway through."
                    })
                    .size(theme::BODY)
                    .color(p.secondary),
                );
                ui.add_space(14.0);
                for (title, required) in [("NEEDED FOR THIS RECORDING", true), ("OPTIONAL", false)] {
                    let rows: Vec<&Item> = list.items.iter().filter(|i| i.required == required).collect();
                    if rows.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(title).font(theme::semibold(theme::SECONDARY)).color(p.secondary));
                    ui.add_space(4.0);
                    for item in rows {
                        if let Some(k) = row(ui, item) {
                            event = Some(Event::Grant(k));
                        }
                        ui.add_space(6.0);
                    }
                    ui.add_space(6.0);
                }
                if list.status(Kind::Screen) != Status::Granted {
                    caption(ui, "After you allow Screen Recording, macOS asks you to reopen Glide.");
                    ui.add_space(8.0);
                }
                if ready {
                    if primary(ui, "Start recording setup").clicked() {
                        event = Some(Event::Close);
                    }
                } else if ui.button("Skip for now").on_hover_text("Recording stays blocked until the needed permissions are on").clicked() {
                    event = Some(Event::Close);
                }
            });
        });
    });
    event
}

fn row(ui: &mut Ui, item: &Item) -> Option<Kind> {
    let p = pal(ui);
    let mut clicked = None;
    egui::Frame::new()
        .fill(p.sidebar)
        .stroke(Stroke::new(1.0, p.separator))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                badge(ui, item.status, item.required);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    // Room for the button on the right: the text wraps before it.
                    ui.set_max_width((ui.available_width() - 130.0).max(120.0));
                    ui.label(RichText::new(item.kind.title()).font(theme::semibold(theme::BODY)).color(p.text));
                    ui.label(RichText::new(item.kind.why(item.required)).size(theme::SECONDARY + 1.0).color(p.secondary));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match item.status {
                    Status::Granted => {
                        ui.label(RichText::new("Allowed").size(theme::SECONDARY + 1.0).color(GREEN));
                    }
                    Status::Restricted => {
                        ui.label(RichText::new("Restricted by your Mac").size(theme::SECONDARY + 1.0).color(p.secondary));
                    }
                    Status::NotDetermined => {
                        if primary(ui, "Allow").clicked() {
                            clicked = Some(item.kind);
                        }
                    }
                    Status::Denied => {
                        let label = if matches!(item.kind, Kind::Mic | Kind::Camera) { "Open Settings" } else { "Allow" };
                        if ui.button(label).clicked() {
                            clicked = Some(item.kind);
                        }
                    }
                });
            });
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    fn settings(mic: bool, camera: bool) -> Settings {
        Settings { mic, camera, ..Default::default() }
    }

    #[test]
    fn screen_recording_always_blocks() {
        let l = Checklist::new([Denied, Granted, Granted, Granted, Granted], &settings(false, false));
        assert!(!l.ready());
        assert_eq!(l.missing().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Screen]);
    }

    #[test]
    fn mic_and_camera_block_only_when_used() {
        let status = [Granted, Denied, NotDetermined, Denied, Denied];
        assert!(Checklist::new(status, &settings(false, false)).ready());
        let l = Checklist::new(status, &settings(true, true));
        assert_eq!(l.missing().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Mic, Kind::Camera]);
    }

    #[test]
    fn keys_and_accessibility_never_block() {
        let l = Checklist::new([Granted, Granted, Granted, Denied, Denied], &settings(true, true));
        assert!(l.ready());
        assert_eq!(l.status(Kind::Keys), Denied);
    }
}
