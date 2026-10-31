//! The editable project: style, timeline edits and export settings.
//! Stored as `project.json` inside a `.glide` package.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::engine::geom::{Rect, Vec2, v2};
use crate::engine::spring::SpringConfig;

pub const PROJECT_VERSION: u32 = 1;

/// sRGB colour with alpha, serialised as `#rrggbbaa`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub [u8; 4]);

impl Color {
    pub const BLACK: Color = Color([0, 0, 0, 255]);
    pub const WHITE: Color = Color([255, 255, 255, 255]);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color([r, g, b, 255])
    }
    pub fn to_f32(self) -> [f32; 4] {
        self.0.map(|c| c as f32 / 255.0)
    }
    pub fn hex(self) -> String {
        let [r, g, b, a] = self.0;
        if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Color([byte(0)?, byte(2)?, byte(4)?, 255])),
            8 => Some(Color([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}
impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s:?}")))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Background {
    /// One of the built-in generated wallpapers.
    Wallpaper { id: String },
    Gradient { angle: f64, stops: Vec<(Color, f64)> },
    Color { color: Color },
    Image { path: String },
}

impl Default for Background {
    fn default() -> Self {
        Background::Wallpaper { id: "aurora".into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub intensity: f64,
    /// Direction the light comes from, degrees (90 = from above).
    pub angle: f64,
    pub distance: f64,
    pub blur: f64,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow { intensity: 0.75, angle: 90.0, distance: 25.0, blur: 20.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Inset {
    /// Inner border thickness in recording pixels.
    pub padding: f64,
    pub color: Color,
    pub alpha: f64,
}

impl Default for Inset {
    fn default() -> Self {
        Inset { padding: 0.0, color: Color::BLACK, alpha: 0.5 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Aspect {
    /// Recording plus padding.
    #[default]
    Auto,
    Ratio { w: u32, h: u32 },
}

impl Aspect {
    pub const PRESETS: [(&'static str, Aspect); 7] = [
        ("Auto", Aspect::Auto),
        ("Wide 16:9", Aspect::Ratio { w: 16, h: 9 }),
        ("Square 1:1", Aspect::Ratio { w: 1, h: 1 }),
        ("Classic 4:3", Aspect::Ratio { w: 4, h: 3 }),
        ("Vertical 9:16", Aspect::Ratio { w: 9, h: 16 }),
        ("Tall 3:4", Aspect::Ratio { w: 3, h: 4 }),
        ("Portrait 4:5", Aspect::Ratio { w: 4, h: 5 }),
    ];
    pub fn ratio(self) -> Option<f64> {
        match self {
            Aspect::Auto => None,
            Aspect::Ratio { w, h } => Some(w as f64 / h as f64),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClickEffect {
    #[default]
    None,
    /// An expanding soft disc under the cursor.
    Circle,
    /// A shockwave that ripples the screen content.
    Ripple { strength: f64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorSkin {
    /// The cursor images recorded from the system.
    #[default]
    System,
    /// Crisp vector cursors drawn by the renderer at any size.
    Vector,
    /// A soft touch-style circle.
    Touch,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorStyle {
    pub size: f64,
    pub hide: bool,
    pub skin: CursorSkin,
    /// None = follow the raw mouse exactly.
    pub smoothing: Option<SpringConfig>,
    /// How much horizontal motion tilts the cursor.
    pub tilt: f64,
    pub base_rotation: f64,
    /// Hide after this long without movement.
    pub hide_idle_ms: Option<f64>,
    /// Glide back to the starting position in the last N ms (seamless loops).
    pub loop_ms: Option<f64>,
    /// Freeze in the last N ms.
    pub freeze_ms: f64,
    /// Glide onto upcoming click points this early, removing jitter.
    pub click_snap_ms: f64,
    pub click_effect: ClickEffect,
}

impl Default for CursorStyle {
    fn default() -> Self {
        CursorStyle {
            size: 1.5,
            hide: false,
            skin: CursorSkin::System,
            smoothing: Some(SpringConfig::CURSOR_SMOOTH),
            tilt: 0.5,
            base_rotation: 0.0,
            hide_idle_ms: None,
            loop_ms: None,
            freeze_ms: 0.0,
            click_snap_ms: 500.0,
            click_effect: ClickEffect::None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionBlur {
    pub amount: f64,
    pub cursor: f64,
    pub screen_move: f64,
    pub screen_zoom: f64,
}

impl Default for MotionBlur {
    fn default() -> Self {
        MotionBlur { amount: 1.0, cursor: 1.0, screen_move: 1.0, screen_zoom: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraAspect {
    #[default]
    Original,
    Square,
    Vertical,
    Horizontal,
}

/// The camera bubble's outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CameraShape {
    /// Rounded rectangle in the chosen aspect (radius from roundness).
    #[default]
    Rounded,
    /// Circle (centre square crop).
    Circle,
    /// Square, rounded by roundness (centre square crop).
    Square,
    /// Capsule in the chosen aspect, fully round ends.
    Pill,
    /// Superellipse square (continuous corners); roundness sets the curve.
    Squircle,
}

impl CameraShape {
    pub const ALL: [CameraShape; 5] = [CameraShape::Circle, CameraShape::Rounded, CameraShape::Square, CameraShape::Pill, CameraShape::Squircle];

    pub fn label(self) -> &'static str {
        match self {
            CameraShape::Rounded => "Rounded",
            CameraShape::Circle => "Circle",
            CameraShape::Square => "Square",
            CameraShape::Pill => "Pill",
            CameraShape::Squircle => "Squircle",
        }
    }

    /// Crops the camera to a centred square.
    pub fn is_square(self) -> bool {
        matches!(self, CameraShape::Circle | CameraShape::Square | CameraShape::Squircle)
    }

    /// The roundness setting applies.
    pub fn uses_roundness(self) -> bool {
        matches!(self, CameraShape::Rounded | CameraShape::Square | CameraShape::Squircle)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraStyle {
    pub hide: bool,
    pub mirror: bool,
    pub shape: CameraShape,
    /// Border width in pixels at 1080p (scaled with the output height).
    pub border: f64,
    pub border_color: Color,
    /// Soft drop shadow under the bubble.
    pub shadow: bool,
    /// Fraction of the safe frame the bubble may occupy.
    pub size: f64,
    /// Corner radius as a fraction of the bubble's short side (0.5 = round).
    pub roundness: f64,
    /// Position inside the safe frame, (0,0) top-left to (1,1) bottom-right.
    pub position: Vec2,
    pub aspect: CameraAspect,
    /// Bubble scale while the screen is zoomed in.
    pub zoom_scale: f64,
}

impl CameraStyle {
    /// Corner radius in pixels for a `w`x`h` bubble.
    pub fn radius(&self, w: f64, h: f64) -> f64 {
        let short = w.min(h);
        match self.shape {
            CameraShape::Circle | CameraShape::Pill => short / 2.0,
            CameraShape::Rounded | CameraShape::Square => self.roundness.clamp(0.0, 0.5) * short,
            CameraShape::Squircle => short / 2.0,
        }
    }

    /// Superellipse exponent (0 = not a squircle): 2 (a circle) at the
    /// roundest, 4 (the classic squircle) halfway, 10 (nearly square) at 0.
    pub fn squircle_exponent(&self) -> f64 {
        if self.shape != CameraShape::Squircle {
            return 0.0;
        }
        let r = (self.roundness / 0.5).clamp(0.0, 1.0);
        2.0 + 8.0 * (1.0 - r).powi(2)
    }

    /// Border width in pixels for an output `out_h` pixels tall.
    pub fn border_px(&self, out_h: f64) -> f64 {
        (self.border.max(0.0) * out_h / 1080.0).round()
    }
}

impl Default for CameraStyle {
    fn default() -> Self {
        CameraStyle {
            hide: false,
            mirror: false,
            shape: CameraShape::Rounded,
            border: 0.0,
            border_color: Color::WHITE,
            shadow: true,
            size: 0.35,
            roundness: 0.25,
            position: v2(1.0, 1.0),
            aspect: CameraAspect::Original,
            zoom_scale: 0.7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutStyle {
    pub show: bool,
    pub single_keys: bool,
    pub size: f64,
}

impl Default for ShortcutStyle {
    fn default() -> Self {
        ShortcutStyle { show: false, single_keys: false, size: 1.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptionStyle {
    pub show: bool,
    pub size: f64,
}

impl Default for CaptionStyle {
    fn default() -> Self {
        CaptionStyle { show: true, size: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioStyle {
    pub volume: f64,
    pub mute_mic: bool,
    pub mute_system: bool,
    /// Gentle denoise + level for the microphone track.
    pub enhance_mic: bool,
    /// Background music: `assets/...` inside the package, or an absolute path in older projects.
    pub music: Option<String>,
    pub music_volume: f64,
    pub click_sounds: bool,
    pub click_volume: f64,
    /// Lower the recorded audio while a voice-over plays.
    pub duck_under_voiceover: bool,
    /// Level of the recorded audio under a voice-over (0..1).
    pub duck_level: f64,
}

impl Default for AudioStyle {
    fn default() -> Self {
        AudioStyle {
            volume: 1.0,
            mute_mic: false,
            mute_system: false,
            enhance_mic: true,
            music: None,
            music_volume: 0.05,
            click_sounds: false,
            click_volume: 0.25,
            duck_under_voiceover: true,
            duck_level: 0.15,
        }
    }
}

/// A device mockup drawn around the screen (generated, not artwork).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceFrame {
    #[default]
    None,
    Laptop,
    Phone,
    Tablet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub background: Background,
    pub background_blur: f64,
    /// Space around the window, percent of the mean screen dimension.
    pub padding: f64,
    /// Window corner radius in recording pixels.
    pub corner_radius: f64,
    pub shadow: Shadow,
    pub inset: Inset,
    pub aspect: Aspect,
    /// Normalised crop of the recording.
    pub crop: Rect,
    pub cursor: CursorStyle,
    pub screen_spring: SpringConfig,
    pub motion_blur: MotionBlur,
    pub camera: CameraStyle,
    pub shortcuts: ShortcutStyle,
    pub captions: CaptionStyle,
    pub audio: AudioStyle,
    /// Zoom level used for new automatic zooms.
    pub default_zoom: f64,
    pub device: DeviceFrame,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            background: Background::default(),
            background_blur: 0.0,
            padding: 10.0,
            corner_radius: 12.0,
            shadow: Shadow::default(),
            inset: Inset::default(),
            aspect: Aspect::Auto,
            crop: Rect::UNIT,
            cursor: CursorStyle::default(),
            screen_spring: SpringConfig::SCREEN_FOCUSED,
            motion_blur: MotionBlur::default(),
            camera: CameraStyle::default(),
            shortcuts: ShortcutStyle::default(),
            captions: CaptionStyle::default(),
            audio: AudioStyle::default(),
            default_zoom: 2.0,
            device: DeviceFrame::None,
        }
    }
}

impl Style {
    /// Padding used to bring a hidden background into view.
    pub const REVEAL_PADDING: f64 = 8.0;

    /// With 0 % padding the background (and shadow) can't be seen. If an
    /// edit since `before` changed the background, its blur or the shadow,
    /// add padding (and soft corners) so the change shows. Returns whether
    /// it did.
    pub fn reveal_background(&mut self, before: &Style) -> bool {
        let touched = self.background != before.background || self.background_blur != before.background_blur || self.shadow != before.shadow;
        if !touched || self.padding > 0.0 || self.device != DeviceFrame::None {
            return false;
        }
        self.padding = Style::REVEAL_PADDING;
        if self.corner_radius == 0.0 {
            self.corner_radius = 12.0;
        }
        true
    }
}

/// A kept piece of the recording. Gaps between slices are cuts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slice {
    pub id: u64,
    /// Source (recording) time range, ms.
    pub start: f64,
    pub end: f64,
    pub speed: f64,
    pub mic_volume: f64,
    pub system_volume: f64,
    pub hide_cursor: bool,
    /// Follow the raw mouse in this slice.
    pub no_smoothing: bool,
}

impl Default for Slice {
    fn default() -> Self {
        Slice {
            id: 0,
            start: 0.0,
            end: 0.0,
            speed: 1.0,
            mic_volume: 1.0,
            system_volume: 1.0,
            hide_cursor: false,
            no_smoothing: false,
        }
    }
}

impl Slice {
    pub fn source_len(&self) -> f64 {
        self.end - self.start
    }
    pub fn playback_len(&self) -> f64 {
        self.source_len() / self.speed
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ZoomMode {
    /// Follow clusters of clicks and activity.
    #[default]
    Auto,
    /// Zoom on a fixed point (normalised in the cropped screen).
    Manual { target: Vec2 },
    /// Keep the live cursor in view.
    FollowMouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Zoom {
    pub id: u64,
    /// Source time range, ms.
    pub start: f64,
    pub end: f64,
    pub scale: f64,
    pub mode: ZoomMode,
    /// How strongly targets near an edge push the view to that edge.
    pub snap_edges: f64,
    /// Cut in/out instead of animating.
    pub instant: bool,
    pub disabled: bool,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom {
            id: 0,
            start: 0.0,
            end: 0.0,
            scale: 2.0,
            mode: ZoomMode::Auto,
            snap_edges: 0.25,
            instant: false,
            disabled: false,
        }
    }
}

impl Zoom {
    /// Shortest a zoom can be made by trimming or splitting.
    pub const MIN_LEN_MS: f64 = 500.0;
    /// Length of a zoom added at the playhead.
    pub const DEFAULT_LEN_MS: f64 = 2500.0;

    /// Move the start edge to `t`. The zoom keeps at least `MIN_LEN_MS` (one
    /// already shorter than that can only grow) and stays inside the recording.
    pub fn trim_start(&mut self, t: f64) {
        let hi = (self.end - Self::MIN_LEN_MS).max(self.start).min(self.end);
        self.start = t.clamp(0.0, hi.max(0.0));
    }

    /// Move the end edge to `t`, for a recording `duration` ms long.
    pub fn trim_end(&mut self, t: f64, duration: f64) {
        let lo = (self.start + Self::MIN_LEN_MS).min(self.end);
        self.end = t.clamp(lo, duration.max(lo));
    }

    /// Move the whole zoom so it starts at `t`, keeping its length.
    pub fn move_to(&mut self, t: f64, duration: f64) {
        let len = self.end - self.start;
        self.start = t.clamp(0.0, (duration - len).max(0.0));
        self.end = self.start + len;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutKind {
    /// Screen with the camera bubble.
    #[default]
    Both,
    ScreenOnly,
    CameraOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutRange {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub kind: LayoutKind,
    pub camera_size: Option<f64>,
    pub camera_position: Option<Vec2>,
}

impl Default for LayoutRange {
    fn default() -> Self {
        LayoutRange { id: 0, start: 0.0, end: 0.0, kind: LayoutKind::Both, camera_size: None, camera_position: None }
    }
}
impl LayoutRange {
    /// Length of a layout added at the playhead.
    pub const DEFAULT_LEN_MS: f64 = 3000.0;
    pub const MIN_LEN_MS: f64 = 300.0;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MaskKind {
    /// Soft blur inside the rect (hide private details).
    #[default]
    Blur,
    /// Mosaic inside the rect.
    Pixelate,
    /// Dim everything outside the rect (spotlight).
    Highlight,
}

/// A region effect over a time range (source ms).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub id: u64,
    pub start: f64,
    pub end: f64,
    pub kind: MaskKind,
    /// Region normalised to the recording (0..1, top-left origin).
    pub rect: Rect,
    /// Blur/pixel size in output px at 1080p (Blur, Pixelate) or dim
    /// opacity 0..1 (Highlight).
    pub amount: f64,
    /// Corner radius in output px at 1080p.
    pub radius: f64,
}

impl Default for Mask {
    fn default() -> Self {
        Mask { id: 0, start: 0.0, end: 0.0, kind: MaskKind::Blur, rect: Rect::new(0.35, 0.35, 0.3, 0.3), amount: 16.0, radius: 10.0 }
    }
}

impl Mask {
    /// Fade in/out time at the ends of the range.
    pub const FADE_MS: f64 = 150.0;
    pub const MIN_LEN_MS: f64 = 300.0;
    /// Length of a mask added at the playhead.
    pub const DEFAULT_LEN_MS: f64 = 3000.0;

    pub fn default_amount(kind: MaskKind) -> f64 {
        match kind {
            MaskKind::Blur => 16.0,
            MaskKind::Pixelate => 14.0,
            MaskKind::Highlight => 0.6,
        }
    }

    /// Opacity at a source time (0 outside the range, eased at the ends).
    /// No fade where the range touches the start or end (`duration`) of the
    /// recording, so a mask covering the first frame is there from frame one.
    pub fn alpha_at(&self, t: f64, duration: f64) -> f64 {
        if t < self.start || t > self.end {
            return 0.0;
        }
        let from_start = if self.start <= 1.0 { f64::INFINITY } else { t - self.start };
        let to_end = if self.end >= duration - 1.0 { f64::INFINITY } else { self.end - t };
        let edge = (from_start.min(to_end) / Self::FADE_MS).clamp(0.0, 1.0);
        edge * edge * (3.0 - 2.0 * edge)
    }
}

/// Narration recorded in the editor, played at normal speed from a point on
/// the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceOver {
    pub id: u64,
    /// Audio file inside the package's recording/ folder.
    pub file: String,
    /// Where it starts (source ms).
    pub start: f64,
    pub duration: f64,
    pub volume: f64,
}

impl Default for VoiceOver {
    fn default() -> Self {
        VoiceOver { id: 0, file: String::new(), start: 0.0, duration: 0.0, volume: 1.0 }
    }
}

/// Where a `len` ms range added at `at` goes in a `duration` ms recording:
/// inside it, and at least `min` long (it starts earlier when `at` is too
/// close to the end). A recording shorter than `len` gets all of it.
pub fn span_at(at: f64, len: f64, min: f64, duration: f64) -> (f64, f64) {
    let duration = duration.max(0.0);
    let (len, min) = (len.min(duration), min.min(duration));
    let start = at.clamp(0.0, (duration - min).max(0.0));
    (start, (start + len).min(duration))
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeline {
    pub slices: Vec<Slice>,
    pub zooms: Vec<Zoom>,
    pub layouts: Vec<LayoutRange>,
    pub masks: Vec<Mask>,
    pub voiceovers: Vec<VoiceOver>,
    next_id: u64,
}

impl Timeline {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Add a zoom starting at source time `at`, `Zoom::DEFAULT_LEN_MS` long
    /// but never past the end of a `duration` ms recording or shorter than
    /// `Zoom::MIN_LEN_MS` (it starts earlier when `at` is too close to the end).
    pub fn add_zoom(&mut self, at: f64, scale: f64, duration: f64) -> u64 {
        let id = self.new_id();
        let (start, end) = span_at(at, Zoom::DEFAULT_LEN_MS, Zoom::MIN_LEN_MS, duration);
        self.zooms.push(Zoom { id, start, end, scale, ..Default::default() });
        id
    }
    /// Add a mask at source time `at`, placed like `add_zoom`.
    pub fn add_mask(&mut self, at: f64, duration: f64) -> u64 {
        let id = self.new_id();
        let (start, end) = span_at(at, Mask::DEFAULT_LEN_MS, Mask::MIN_LEN_MS, duration);
        self.masks.push(Mask { id, start, end, ..Default::default() });
        id
    }
    /// Add a camera-only layout at source time `at`, placed like `add_zoom`.
    pub fn add_layout(&mut self, at: f64, duration: f64) -> u64 {
        let id = self.new_id();
        let (start, end) = span_at(at, LayoutRange::DEFAULT_LEN_MS, LayoutRange::MIN_LEN_MS, duration);
        self.layouts.push(LayoutRange { id, start, end, kind: LayoutKind::CameraOnly, ..Default::default() });
        id
    }
    /// Split zoom `id` at source time `t` into two that share its settings.
    /// Each half keeps at least `Zoom::MIN_LEN_MS`. Returns the id of the new
    /// second half, or None when `t` is too close to an end (or no such zoom).
    pub fn split_zoom(&mut self, id: u64, t: f64) -> Option<u64> {
        let i = self.zooms.iter().position(|z| z.id == id)?;
        let z = self.zooms[i];
        if t < z.start + Zoom::MIN_LEN_MS || t > z.end - Zoom::MIN_LEN_MS {
            return None;
        }
        let new = self.new_id();
        self.zooms[i].end = t;
        self.zooms.insert(i + 1, Zoom { id: new, start: t, ..z });
        Some(new)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    #[default]
    Mp4,
    Gif,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quality {
    Studio,
    #[default]
    Social,
    Web,
    WebLow,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Studio, Quality::Social, Quality::Web, Quality::WebLow];

    /// VideoToolbox constant-quality setting (0..1): the encoder spends what
    /// each frame needs, so fine text stays crisp through motion while static
    /// screens stay small.
    pub fn encoder_quality(self) -> f64 {
        match self {
            Quality::Studio => 0.95,
            Quality::Social => 0.85,
            Quality::Web => 0.7,
            Quality::WebLow => 0.55,
        }
    }

    /// Bits per pixel per frame, for size estimates (and GIF effort).
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            Quality::Studio => 0.3,
            Quality::Social => 0.05,
            Quality::Web => 0.0175,
            Quality::WebLow => 0.0075,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Quality::Studio => "Studio",
            Quality::Social => "Social media",
            Quality::Web => "Web",
            Quality::WebLow => "Web (low)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    #[default]
    H264,
    Hevc,
    /// Apple ProRes 4444 in a .mov: visually lossless, full 4:4:4 colour,
    /// for masters and further editing (large files).
    Prores,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub format: ExportFormat,
    /// Output height in pixels; width follows the project's aspect ratio.
    /// 0 = native: the recording's own pixels, unscaled.
    pub height: u32,
    pub fps: u32,
    pub quality: Quality,
    pub codec: Codec,
    pub gif_loop: bool,
    /// MP4 only: pick the bitrate so the file comes out at about this many MB.
    pub target_mb: Option<u32>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            format: ExportFormat::Mp4,
            height: Self::NATIVE,
            // Light mode (older Macs): half the frames to render and encode.
            fps: if crate::hw::light() { 30 } else { 60 },
            quality: Quality::Social,
            codec: Codec::H264,
            gif_loop: true,
            target_mb: None,
        }
    }
}

/// The last settings used for each export format, so switching MP4 to GIF
/// and back brings each side back as it was left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatMemory {
    mp4: ExportSettings,
    gif: ExportSettings,
}
impl FormatMemory {
    /// Remember `current` for its format; the other starts at its defaults.
    pub fn new(current: ExportSettings) -> Self {
        let mut m = FormatMemory { mp4: ExportSettings::default(), gif: ExportSettings::gif_default() };
        m.remember(current);
        m
    }
    pub fn remember(&mut self, s: ExportSettings) {
        match s.format {
            ExportFormat::Mp4 => self.mp4 = s,
            ExportFormat::Gif => self.gif = s,
        }
    }
    pub fn recall(&self, format: ExportFormat) -> ExportSettings {
        match format {
            ExportFormat::Mp4 => self.mp4,
            ExportFormat::Gif => self.gif,
        }
    }
    /// Leave the format of `leaving` (remembering it) for `to`.
    pub fn switch(&mut self, leaving: ExportSettings, to: ExportFormat) -> ExportSettings {
        self.remember(leaving);
        self.recall(to)
    }
}
impl ExportSettings {
    pub const MP4_FPS: [u32; 7] = [60, 50, 30, 25, 24, 20, 10];
    pub const GIF_FPS: [u32; 6] = [50, 30, 25, 20, 15, 10];
    /// `height` for the recording's native resolution.
    pub const NATIVE: u32 = 0;
    pub const MIN_FPS: u32 = 1;
    pub const MAX_FPS: u32 = 120;
    /// The tallest export Glide accepts (8K is 4320; this leaves room for portrait aspects).
    pub const MAX_HEIGHT: u32 = 8640;
    /// These settings with values the encoder can't take brought into range: a frame rate
    /// outside 1..=120, a height above [`Self::MAX_HEIGHT`] and a target size of 0 MB. A
    /// hand-edited `project.json` can hold any of them.
    pub fn sanitized(self) -> Self {
        ExportSettings {
            fps: self.fps.clamp(Self::MIN_FPS, Self::MAX_FPS),
            height: self.height.min(Self::MAX_HEIGHT),
            target_mb: self.target_mb.filter(|mb| *mb > 0),
            ..self
        }
    }
    pub const MP4_HEIGHTS: [u32; 5] = [Self::NATIVE, 2160, 1440, 1080, 720];
    pub const GIF_HEIGHTS: [u32; 3] = [480, 720, 1080];

    pub fn gif_default() -> Self {
        ExportSettings { format: ExportFormat::Gif, height: 480, fps: 15, quality: Quality::Studio, ..Default::default() }
    }

    /// Whether "fit to file size" applies (not to GIF or ProRes).
    pub fn fits_size(&self) -> bool {
        self.format == ExportFormat::Mp4 && self.codec != Codec::Prores
    }

    /// The codec actually used at a size: VideoToolbox's H.264 encoder stops
    /// at 4096x2304, so bigger (e.g. native Retina) exports use HEVC.
    pub fn codec_for(&self, width: u32, height: u32) -> Codec {
        let (long, short) = (width.max(height), width.min(height));
        if self.codec == Codec::H264 && (long > 4096 || short > 2304) { Codec::Hevc } else { self.codec }
    }

    /// Audio bitrate assumed for size estimates (AAC stereo).
    pub const AUDIO_BPS: f64 = 192_000.0;

    /// Target video bitrate (bits/s) for an output size and duration. With a
    /// target size the bitrate fills that size (minus audio and container
    /// overhead); otherwise it follows the quality preset.
    pub fn bitrate(&self, width: u32, height: u32, secs: f64) -> u64 {
        if let Some(mb) = self.target_mb.filter(|_| self.fits_size())
            && secs > 0.0
        {
            let total = mb as f64 * 8e6 * 0.96 / secs;
            return (total - Self::AUDIO_BPS).max(250_000.0) as u64;
        }
        // ProRes 4444 is fixed-quality intra-frame (~2-3 bits/pixel on screens).
        let bpp = if self.codec == Codec::Prores { 2.5 } else { self.quality.bits_per_pixel() };
        let base = width as f64 * height as f64 * self.fps as f64 * bpp;
        // HEVC reaches the same quality with ~40% fewer bits.
        let codec = if self.codec_for(width, height) == Codec::Hevc { 0.6 } else { 1.0 };
        (base * codec).max(250_000.0) as u64
    }

    /// Rough output size in MB.
    pub fn estimate_mb(&self, width: u32, height: u32, secs: f64) -> f64 {
        match self.format {
            ExportFormat::Mp4 => (self.bitrate(width, height, secs) as f64 + Self::AUDIO_BPS) * secs / 8e6,
            // Delta-frame GIFs of screen content average ~0.035 bytes per pixel per frame.
            ExportFormat::Gif => (width * height) as f64 * secs * self.fps as f64 * 0.035 / 1e6,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub style: Style,
    pub timeline: Timeline,
    pub export: ExportSettings,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: PROJECT_VERSION,
            id: String::new(),
            name: "Untitled".into(),
            created_ms: 0,
            updated_ms: 0,
            style: Style::default(),
            timeline: Timeline::default(),
            export: ExportSettings::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_round_trip() {
        let c = Color([1, 0xab, 0xff, 0x80]);
        assert_eq!(Color::parse(&c.hex()), Some(c));
        assert_eq!(Color::parse("#ffffff"), Some(Color::WHITE));
    }

    #[test]
    fn project_round_trips_through_json() {
        let mut p = Project::default();
        p.style.background = Background::Gradient { angle: 135.0, stops: vec![(Color::BLACK, 0.0), (Color::WHITE, 1.0)] };
        p.timeline.zooms.push(Zoom { mode: ZoomMode::Manual { target: v2(0.2, 0.3) }, ..Default::default() });
        let s = serde_json::to_string(&p).unwrap();
        let q: Project = serde_json::from_str(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let p: Project = serde_json::from_str(r#"{"name":"x","style":{"padding":3}}"#).unwrap();
        assert_eq!(p.style.padding, 3.0);
        assert_eq!(p.style.corner_radius, 12.0);
        assert_eq!(p.export.fps, ExportSettings::default().fps);
        assert_eq!(p.export.height, ExportSettings::NATIVE);
    }

    #[test]
    fn bitrate_formula() {
        let e = ExportSettings { height: 1080, fps: 60, ..Default::default() };
        assert_eq!(e.bitrate(1920, 1080, 10.0), (1920.0 * 1080.0 * 60.0 * 0.05) as u64);
        // Fit to 10 MB over 20 s: ~3.84 Mbit/s total minus audio.
        let t = ExportSettings { target_mb: Some(10), ..e };
        assert_eq!(t.bitrate(1920, 1080, 20.0), (10.0 * 8e6 * 0.96 / 20.0 - ExportSettings::AUDIO_BPS) as u64);
        assert!((t.estimate_mb(1920, 1080, 20.0) - 9.6).abs() < 0.01);
    }

    #[test]
    fn big_exports_switch_to_hevc() {
        let e = ExportSettings::default();
        assert_eq!(e.codec_for(3840, 2160), Codec::H264);
        assert_eq!(e.codec_for(4654, 3200), Codec::Hevc);
        assert_eq!(e.codec_for(2160, 3840), Codec::H264);
        assert_eq!(ExportSettings { codec: Codec::Hevc, ..e }.codec_for(1280, 720), Codec::Hevc);
    }

    #[test]
    fn sanitized_export_settings_stay_inside_what_the_encoder_takes() {
        let bad = ExportSettings { fps: 0, height: 999_999, target_mb: Some(0), ..Default::default() };
        let ok = bad.sanitized();
        assert_eq!((ok.fps, ok.height, ok.target_mb), (ExportSettings::MIN_FPS, ExportSettings::MAX_HEIGHT, None));
        let high = ExportSettings { fps: 500, ..Default::default() }.sanitized();
        assert_eq!(high.fps, ExportSettings::MAX_FPS);
    }
    #[test]
    fn sanitized_export_settings_keep_valid_values() {
        for fps in ExportSettings::MP4_FPS.into_iter().chain(ExportSettings::GIF_FPS) {
            let e = ExportSettings { fps, height: ExportSettings::NATIVE, target_mb: Some(10), ..Default::default() };
            assert_eq!(e.sanitized(), e, "fps {fps}");
        }
        for height in ExportSettings::MP4_HEIGHTS.into_iter().chain(ExportSettings::GIF_HEIGHTS) {
            let e = ExportSettings { height, ..Default::default() };
            assert_eq!(e.sanitized(), e, "height {height}");
        }
    }
    fn zoom(start: f64, end: f64) -> Zoom {
        Zoom { id: 1, start, end, ..Default::default() }
    }

    #[test]
    fn trimming_a_zoom_keeps_it_inside_the_recording_and_above_the_minimum() {
        let mut z = zoom(2000.0, 4000.0);
        z.trim_start(-500.0);
        assert_eq!(z.start, 0.0);
        z.trim_start(9000.0);
        assert_eq!(z.start, 4000.0 - Zoom::MIN_LEN_MS);
        z.trim_end(0.0, 10_000.0);
        assert_eq!(z.end, z.start + Zoom::MIN_LEN_MS);
        z.trim_end(99_000.0, 10_000.0);
        assert_eq!(z.end, 10_000.0);
        assert!(z.end - z.start >= Zoom::MIN_LEN_MS);
    }

    #[test]
    fn a_zoom_shorter_than_the_minimum_can_grow_but_not_shrink() {
        let mut z = zoom(1000.0, 1200.0);
        z.trim_start(1150.0);
        assert_eq!(z.start, 1000.0);
        z.trim_end(1050.0, 5000.0);
        assert_eq!(z.end, 1200.0);
        z.trim_start(200.0);
        z.trim_end(3000.0, 5000.0);
        assert_eq!((z.start, z.end), (200.0, 3000.0));
        // One past the end of a short recording is pulled back in, never inverted.
        let mut z = zoom(100.0, 400.0);
        z.trim_end(900.0, 300.0);
        assert!(z.end >= z.start);
    }

    #[test]
    fn moving_a_zoom_keeps_its_length_and_stays_inside() {
        let mut z = zoom(2000.0, 4000.0);
        z.move_to(-300.0, 10_000.0);
        assert_eq!((z.start, z.end), (0.0, 2000.0));
        z.move_to(9500.0, 10_000.0);
        assert_eq!((z.start, z.end), (8000.0, 10_000.0));
        // Longer than the recording: pinned at the start rather than panicking.
        let mut z = zoom(0.0, 3000.0);
        z.move_to(500.0, 2000.0);
        assert_eq!(z.start, 0.0);
    }

    #[test]
    fn switching_format_and_back_keeps_each_sides_settings() {
        let mp4 = ExportSettings { height: 1080, fps: 30, quality: Quality::Web, codec: Codec::Hevc, target_mb: Some(20), ..Default::default() };
        let mut mem = FormatMemory::new(mp4);
        let gif = mem.switch(mp4, ExportFormat::Gif);
        assert_eq!(gif, ExportSettings::gif_default());
        let edited = ExportSettings { fps: 10, height: 360, gif_loop: false, ..gif };
        assert_eq!(mem.switch(edited, ExportFormat::Mp4), mp4);
        assert_eq!(mem.switch(mp4, ExportFormat::Gif), edited);
    }
    #[test]
    fn a_new_memory_offers_the_other_formats_defaults() {
        let mem = FormatMemory::new(ExportSettings::default());
        assert_eq!(mem.recall(ExportFormat::Gif), ExportSettings::gif_default());
        let gif = ExportSettings { fps: 10, ..ExportSettings::gif_default() };
        let mem = FormatMemory::new(gif);
        assert_eq!(mem.recall(ExportFormat::Gif), gif);
        assert_eq!(mem.recall(ExportFormat::Mp4), ExportSettings::default());
    }
    #[test]
    fn span_at_start_of_the_recording() {
        assert_eq!(span_at(0.0, 3000.0, 300.0, 10_000.0), (0.0, 3000.0));
    }
    #[test]
    fn span_at_the_middle() {
        assert_eq!(span_at(4000.0, 3000.0, 300.0, 10_000.0), (4000.0, 7000.0));
    }
    #[test]
    fn span_at_exactly_the_end() {
        assert_eq!(span_at(10_000.0, 3000.0, 300.0, 10_000.0), (9700.0, 10_000.0));
    }
    #[test]
    fn span_past_the_end_is_pulled_back() {
        assert_eq!(span_at(15_000.0, 3000.0, 300.0, 10_000.0), (9700.0, 10_000.0));
    }
    #[test]
    fn span_before_the_start_is_pushed_forward() {
        assert_eq!(span_at(-50.0, 3000.0, 300.0, 10_000.0), (0.0, 3000.0));
    }
    #[test]
    fn a_recording_shorter_than_the_default_length_gets_the_whole_recording() {
        assert_eq!(span_at(0.0, 3000.0, 300.0, 1000.0), (0.0, 1000.0));
        assert_eq!(span_at(100.0, 3000.0, 300.0, 200.0), (0.0, 200.0));
    }
    #[test]
    fn added_masks_and_layouts_are_never_zero_length_at_the_end() {
        let mut tl = Timeline::default();
        let m = tl.add_mask(10_000.0, 10_000.0);
        let l = tl.add_layout(15_000.0, 10_000.0);
        let mask = *tl.masks.iter().find(|x| x.id == m).unwrap();
        let layout = *tl.layouts.iter().find(|x| x.id == l).unwrap();
        assert_eq!((mask.start, mask.end), (10_000.0 - Mask::MIN_LEN_MS, 10_000.0));
        assert_eq!((layout.start, layout.end), (10_000.0 - LayoutRange::MIN_LEN_MS, 10_000.0));
        assert_eq!(layout.kind, LayoutKind::CameraOnly);
        let m = tl.add_mask(2000.0, 10_000.0);
        let mask = *tl.masks.iter().find(|x| x.id == m).unwrap();
        assert_eq!((mask.start, mask.end), (2000.0, 5000.0));
    }
    #[test]
    fn added_zooms_never_run_past_the_end_or_below_the_minimum() {
        let mut tl = Timeline::default();
        let a = tl.add_zoom(1000.0, 2.0, 10_000.0);
        let b = tl.add_zoom(9800.0, 2.0, 10_000.0);
        let c = tl.add_zoom(10_000.0, 2.0, 10_000.0);
        let get = |id| *tl.zooms.iter().find(|z| z.id == id).unwrap();
        assert_eq!((get(a).start, get(a).end), (1000.0, 3500.0));
        assert_eq!((get(b).start, get(b).end), (10_000.0 - Zoom::MIN_LEN_MS, 10_000.0));
        assert_eq!(get(c).start, get(b).start);
        assert!(tl.zooms.iter().all(|z| z.end - z.start >= Zoom::MIN_LEN_MS && z.end <= 10_000.0));
        // A recording shorter than the minimum still gets a zoom inside it.
        let mut tl = Timeline::default();
        let id = tl.add_zoom(100.0, 2.0, 300.0);
        assert_eq!((tl.zooms[0].id, tl.zooms[0].start, tl.zooms[0].end), (id, 0.0, 300.0));
    }

    #[test]
    fn splitting_a_zoom_makes_two_that_share_its_settings() {
        let mut tl = Timeline::default();
        let id = tl.new_id();
        let z = Zoom { id, start: 2000.0, end: 5000.0, scale: 3.0, instant: true, ..Default::default() };
        tl.zooms.push(z);
        let new = tl.split_zoom(id, 3500.0).unwrap();
        assert_ne!(new, id);
        assert_eq!(tl.zooms.len(), 2);
        assert_eq!((tl.zooms[0].id, tl.zooms[0].start, tl.zooms[0].end), (id, 2000.0, 3500.0));
        assert_eq!((tl.zooms[1].id, tl.zooms[1].start, tl.zooms[1].end), (new, 3500.0, 5000.0));
        assert!(tl.zooms.iter().all(|z| z.scale == 3.0 && z.instant));
    }

    #[test]
    fn a_zoom_wont_split_closer_to_an_end_than_the_minimum() {
        let mut tl = Timeline::default();
        let id = tl.new_id();
        tl.zooms.push(Zoom { id, start: 2000.0, end: 4000.0, ..Default::default() });
        let before = tl.clone();
        for t in [1000.0, 2000.0, 2000.0 + Zoom::MIN_LEN_MS - 1.0, 4000.0 - Zoom::MIN_LEN_MS + 1.0, 4000.0, 9000.0] {
            assert_eq!(tl.split_zoom(id, t), None, "{t}");
        }
        assert_eq!(tl.split_zoom(99, 3000.0), None);
        assert_eq!(tl, before);
        // The boundary itself is allowed: both halves are exactly the minimum.
        let mut tl = before;
        assert!(tl.split_zoom(id, 2000.0 + Zoom::MIN_LEN_MS).is_some());
    }
}
