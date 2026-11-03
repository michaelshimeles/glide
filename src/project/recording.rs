//! What the recorder captured: media files plus input events. Stored in the
//! `recording/` folder of a `.glide` package and never modified by editing.
//!
//! All positions are in screen-video pixels (top-left origin) and all times
//! are milliseconds since the start of the recording, with pauses removed.

use serde::{Deserialize, Serialize};

use crate::engine::geom::Rect;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Display,
    Window,
    Area,
    /// A USB-connected iPhone or iPad screen.
    Device,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoTrack {
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Recording time of this file's first frame (camera files start a
    /// little after the screen).
    pub offset_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioTrack {
    pub file: String,
    /// Index of the audio track within the file.
    pub track: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingMeta {
    pub version: u32,
    pub kind: SourceKind,
    pub duration_ms: f64,
    pub screen: VideoTrack,
    /// Video pixels per screen point (2 on Retina).
    pub scale: f64,
    /// Captured region in global screen points, at the start.
    pub bounds: Rect,
    pub camera: Option<VideoTrack>,
    pub mic: Option<AudioTrack>,
    pub system_audio: Option<AudioTrack>,
    pub app_name: Option<String>,
    pub created_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseKind {
    Move,
    Drag,
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MouseEvent {
    pub t: f64,
    pub x: f64,
    pub y: f64,
    pub kind: MouseKind,
    /// 0 = left, 1 = right, 2 = other.
    #[serde(default)]
    pub button: u8,
    /// Index into the cursor image table.
    #[serde(default)]
    pub cursor: u32,
}

pub mod mods {
    pub const CMD: u8 = 1;
    pub const SHIFT: u8 = 2;
    pub const OPT: u8 = 4;
    pub const CTRL: u8 = 8;
    pub const FN: u8 = 16;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub t: f64,
    /// Virtual key code.
    pub code: u16,
    /// Printable label ("A", "Return", "→" …).
    pub key: String,
    pub mods: u8,
    pub down: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorImage {
    pub id: u32,
    /// PNG file in `recording/cursors/`, if an image was captured.
    pub file: Option<String>,
    /// Hotspot, normalised to the image size.
    pub hotspot: (f64, f64),
    /// Size in screen points.
    pub size: (f64, f64),
    /// Known system cursor name ("arrow", "ibeam", "pointing-hand" …).
    pub system: Option<String>,
}

/// Bounds of a recorded window over time (window recordings only).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoundsChange {
    pub t: f64,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Everything loaded from `recording/`.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub meta: RecordingMeta,
    pub mouse: Vec<MouseEvent>,
    pub keys: Vec<KeyEvent>,
    pub cursors: Vec<CursorImage>,
    pub transcript: Vec<Word>,
    /// The hits in the audio that effects follow (see `project::beats`).
    pub beats: crate::engine::beats::Beats,
}

impl Recording {
    pub fn screen_size(&self) -> (f64, f64) {
        (self.meta.screen.width as f64, self.meta.screen.height as f64)
    }
}
