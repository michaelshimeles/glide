//! The app's look: Inter and one dark palette. The colours were measured from
//! a reference screenshot of the editor, not picked by eye.

use egui::{
    Color32, CornerRadius, FontData, FontFamily, FontId, Shadow, Stroke,
    vec2,
};
use std::sync::Arc;

pub static INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// Lavender: selected icons and tabs, links, the slider knob.
pub const ACCENT: Color32 = Color32::from_rgb(0xa1, 0x8f, 0xf9);
/// Violet: filled controls (the slider's filled part, the main button).
pub const VIOLET: Color32 = Color32::from_rgb(0x4e, 0x35, 0xdc);
/// The darker end of the slider's filled part.
pub const INDIGO: Color32 = Color32::from_rgb(0x2b, 0x1b, 0x87);

/// Body text and secondary text, in points.
pub const BODY: f32 = 12.0;
pub const SECONDARY: f32 = 10.5;
/// Height of the unified toolbar.
pub const TOOLBAR_H: f32 = 40.0;
/// Traffic lights: left inset and the height of the bar they are centred in (points).
pub const TRAFFIC_LIGHTS: (f64, f64) = (14.0, TOOLBAR_H as f64);
/// Horizontal room the traffic lights need from the window's left edge.
pub const TRAFFIC_LIGHTS_W: f32 = 88.0;

/// Whether the main window is in full screen, where AppKit hides the traffic
/// lights (set from redraw).
pub static FULLSCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The display the main window is full screen on (0 when it isn't).
pub static FULLSCREEN_DISPLAY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Left inset of the toolbar's content: past the traffic lights, or a plain
/// margin in full screen, where there are none.
pub fn toolbar_left() -> i8 {
    if FULLSCREEN.load(std::sync::atomic::Ordering::Relaxed) { 16 } else { TRAFFIC_LIGHTS_W as i8 }
}

pub fn accent() -> Color32 {
    ACCENT
}

/// The app's colours.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Behind everything: the window and the preview.
    pub window: Color32,
    /// The timeline.
    pub sidebar: Color32,
    /// Panels that float over the window (the inspector, the tool rail).
    pub inspector: Color32,
    /// Buttons, chips and other controls on a panel.
    pub control: Color32,
    /// A selected control.
    pub selected: Color32,
    pub control_border: Color32,
    pub field: Color32,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub separator: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub popover: Color32,
}

pub fn palette() -> Palette {
    Palette {
        window: Color32::from_rgb(0x08, 0x0a, 0x0b),
        sidebar: Color32::from_rgb(0x08, 0x0a, 0x0b),
        inspector: Color32::from_rgb(0x13, 0x15, 0x1b),
        control: Color32::from_rgb(0x18, 0x1a, 0x1e),
        selected: Color32::from_rgb(0x25, 0x26, 0x2c),
        control_border: Color32::from_white_alpha(12),
        field: Color32::from_rgb(0x0d, 0x0f, 0x12),
        text: Color32::WHITE,
        secondary: Color32::from_rgb(0x99, 0x99, 0xab),
        tertiary: Color32::from_rgb(0x56, 0x58, 0x59),
        separator: Color32::from_rgb(0x1e, 0x20, 0x26),
        hover: Color32::from_white_alpha(14),
        pressed: Color32::from_white_alpha(26),
        popover: Color32::from_rgb(0x1b, 0x1d, 0x23),
    }
}

pub fn pal(_ui: &egui::Ui) -> Palette {
    palette()
}

/// Inter (Regular/Medium/SemiBold) as the proportional family, egui's fonts as fallbacks.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("Inter", INTER_REGULAR),
        ("Inter-Medium", INTER_MEDIUM),
        ("Inter-SemiBold", INTER_SEMIBOLD),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallbacks: Vec<String> = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let with = |first: &str| {
        let mut v = vec![first.to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, with("Inter"));
    fonts
        .families
        .insert(FontFamily::Name("medium".into()), with("Inter-Medium"));
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), with("Inter-SemiBold"));
    if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
        mono.push("Inter".into());
    }
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn style(s: &mut egui::Style) {
    use egui::TextStyle as T;
    let p = palette();
    s.visuals = egui::Visuals::dark();
    s.text_styles = [
        (T::Small, FontId::proportional(SECONDARY)),
        (T::Body, FontId::proportional(BODY)),
        (T::Button, FontId::proportional(BODY)),
        (T::Heading, semibold(BODY)),
        (T::Monospace, FontId::monospace(11.0)),
    ]
    .into();
    s.spacing.item_spacing = vec2(6.0, 5.0);
    s.spacing.button_padding = vec2(10.0, 4.0);
    s.spacing.interact_size = vec2(24.0, 24.0);
    s.spacing.menu_margin = egui::Margin::same(6);
    s.spacing.window_margin = egui::Margin::same(14);
    s.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        handle_min_length: 24.0,
        ..egui::style::ScrollStyle::floating()
    };
    s.interaction.selectable_labels = false;

    let v = &mut s.visuals;
    v.override_text_color = None;
    v.weak_text_color = Some(p.secondary);
    v.hyperlink_color = ACCENT;
    v.panel_fill = p.inspector;
    v.window_fill = p.popover;
    v.faint_bg_color = Color32::from_white_alpha(6);
    v.extreme_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.window_stroke = Stroke::new(1.0, p.control_border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(150) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(130) };
    v.selection.bg_fill = VIOLET;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.indent_has_left_vline = false;
    v.striped = false;
    v.interact_cursor = None;
    v.collapsing_header_frame = false;

    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.inspector;
    w.noninteractive.weak_bg_fill = p.inspector;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.separator);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.control;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;
    w.hovered.bg_fill = p.control;
    w.hovered.weak_bg_fill = blend(p.control, p.hover);
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = blend(p.control, p.pressed);
    w.active.weak_bg_fill = blend(p.control, p.pressed);
    w.active.bg_stroke = Stroke::NONE;
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;
    w.open = w.active;
}

/// `over` composited on `base`.
pub fn blend(base: Color32, over: Color32) -> Color32 {
    let a = over.a() as f32 / 255.0;
    let [or, og, ob, _] = over.to_srgba_unmultiplied();
    let mix = |b: u8, o: u8| (b as f32 * (1.0 - a) + o as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), or), mix(base.g(), og), mix(base.b(), ob))
}

/// Fonts and the dark style, whatever the system appearance is.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.options_mut(|o| {
        o.theme_preference = egui::ThemePreference::Dark;
        o.fallback_theme = egui::Theme::Dark;
    });
    ctx.style_mut_of(egui::Theme::Light, style);
    ctx.style_mut_of(egui::Theme::Dark, style);
}
