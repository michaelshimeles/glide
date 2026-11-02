//! Output geometry: how big the video is and where the window sits.
//!
//! Sizes are computed in "design" pixels at the recording's scale; export
//! scales the whole design uniformly to the requested height.

use super::device::{self, Margins};
use super::geom::{Rect, Vec2, v2};
use crate::project::model::Style;

/// Largest output (VideoToolbox HEVC's limit); bigger designs are scaled
/// down. Native Retina and 5K/6K recordings fit unscaled.
pub const MAX_OUTPUT: Vec2 = v2(8192.0, 4320.0);

/// Stacked layout: the camera takes this fraction of the frame's height, the
/// screen the rest.
pub const STACK_BAND: f64 = 1.0 / 3.0;
/// Stacked layout output aspect (w/h): 9:16.
pub const STACK_ASPECT: f64 = 9.0 / 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sizes {
    /// Recording (video) size.
    pub recording: Vec2,
    /// Crop in video pixels.
    pub crop: Rect,
    /// Inner border between window edge and content.
    pub inset: f64,
    /// Cropped screen plus inset on both sides.
    pub window: Vec2,
    /// Background padding on each side.
    pub pad: f64,
    /// Final design output size.
    pub output: Vec2,
    /// Extra centring space added by a forced aspect ratio.
    pub aspect_pad: Vec2,
    /// Where the screen may be drawn: the output minus padding, and minus
    /// the camera band when stacked.
    pub content: Vec2,
    /// Top-left of `content` in the output.
    pub origin: Vec2,
    /// Size of the output the screen shares with the camera band.
    pub view: Vec2,
    /// The camera band across the top of a stacked layout.
    pub band: Option<Rect>,
    /// Window scale at zoom 1.
    pub initial_zoom: f64,
    /// Space taken by a device frame around the window.
    pub frame: Margins,
}

impl Sizes {
    pub fn new(style: &Style, recording: Vec2) -> Self {
        let c = style.crop;
        let crop = Rect::new(c.x * recording.x, c.y * recording.y, c.w * recording.x, c.h * recording.y);
        let inset = style.inset.padding.max(0.0);
        let window = crop.size() + Vec2::splat(2.0 * inset);
        let frame = device::margins(style.device, window);
        let framed = window + frame.size();
        let pad = (window.avg() * style.padding.max(0.0) / 100.0).ceil();
        let natural = framed + Vec2::splat(2.0 * pad);
        let stacked = style.camera.stacked;
        let ratio = if stacked { Some(STACK_ASPECT) } else { style.aspect.ratio() };
        let output = match ratio {
            Some(r) if (natural.aspect() - r).abs() > 1e-3 => natural.expand_to_aspect(r),
            _ => natural,
        };
        let band_h = if stacked { output.y * STACK_BAND } else { 0.0 };
        let band = stacked.then(|| Rect::new(0.0, 0.0, output.x, band_h));
        let view = v2(output.x, output.y - band_h);
        let origin = v2(pad, band_h + pad);
        let content = view - Vec2::splat(2.0 * pad);
        let aspect_pad = (content - framed) / 2.0 + v2(frame.l, frame.t);
        let fill = content.div(framed);
        let initial_zoom = if stacked || output.y > output.x || pad <= 0.0 { fill.max_elem() } else { 1.0 };
        Sizes { recording, crop, inset, window, pad, output, aspect_pad, content, origin, view, band, initial_zoom, frame }
    }

    /// The part of the output the screen is clipped to: everything under the
    /// camera band when stacked.
    pub fn screen_clip(&self) -> Option<Rect> {
        self.band.map(|b| Rect::new(0.0, b.h, self.output.x, self.output.y - b.h))
    }

    /// Unzoomed window rect within the content area.
    pub fn default_body(&self) -> Rect {
        Rect::from_min_size(self.aspect_pad, self.window)
    }

    /// Even pixel dimensions for an encoder at a given resolution (0 =
    /// native). A resolution is the short side, so 1080p is 1920×1080 wide
    /// and 1080×1920 tall.
    pub fn render_size(&self, height: u32) -> (u32, u32) {
        let s = if height == 0 {
            self.native_height() as f64 / self.output.y
        } else if self.output.y > self.output.x {
            height as f64 / self.output.x
        } else {
            height as f64 / self.output.y
        };
        let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
        (even(self.output.x * s), even(self.output.y * s))
    }

    /// Native render height: design pixels, one per recorded pixel (capped
    /// to MAX_OUTPUT).
    pub fn native_height(&self) -> u32 {
        let cap = (MAX_OUTPUT.x / self.output.x).min(MAX_OUTPUT.y / self.output.y).min(1.0);
        (self.output.y * cap).round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::model::Aspect;

    #[test]
    fn padding_and_auto_aspect() {
        let style = Style::default();
        let s = Sizes::new(&style, v2(2000.0, 1000.0));
        assert_eq!(s.pad, 150.0);
        assert_eq!(s.output, v2(2300.0, 1300.0));
        assert_eq!(s.initial_zoom, 1.0);
        assert_eq!(s.default_body(), Rect::new(0.0, 0.0, 2000.0, 1000.0));
    }

    #[test]
    fn forced_aspect_centres() {
        let mut style = Style::default();
        style.aspect = Aspect::Ratio { w: 1, h: 1 };
        let s = Sizes::new(&style, v2(2000.0, 1000.0));
        assert_eq!(s.output, v2(2300.0, 2300.0));
        assert_eq!(s.aspect_pad, v2(0.0, 500.0));
    }

    #[test]
    fn device_frame_adds_room() {
        let mut style = Style::default();
        style.device = crate::project::model::DeviceFrame::Laptop;
        let s = Sizes::new(&style, v2(2000.0, 1000.0));
        assert!(s.output.x > 2300.0 && s.output.y > 1300.0);
        let body = s.default_body();
        assert!(body.x >= s.frame.l - 1e-9 && body.y >= s.frame.t - 1e-9);
        assert!((body.w - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn stacked_splits_the_frame_in_thirds() {
        let mut style = Style::default();
        style.camera.stacked = true;
        style.padding = 0.0;
        let s = Sizes::new(&style, v2(1920.0, 1080.0));
        assert!((s.output.aspect() - 9.0 / 16.0).abs() < 1e-9);
        let band = s.band.unwrap();
        assert_eq!(band, Rect::new(0.0, 0.0, s.output.x, s.output.y / 3.0));
        assert_eq!(s.origin, v2(0.0, band.h));
        assert!((s.content.y - s.output.y * 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(s.screen_clip(), Some(Rect::new(0.0, band.h, s.output.x, s.output.y - band.h)));
        // The screen covers its area.
        assert!(s.default_body().size().x * s.initial_zoom >= s.content.x - 1e-9);
        assert!(s.default_body().size().y * s.initial_zoom >= s.content.y - 1e-9);
    }

    #[test]
    fn stacked_padding_keeps_screen_clear_of_the_band() {
        let mut style = Style::default();
        style.camera.stacked = true;
        let s = Sizes::new(&style, v2(1920.0, 1080.0));
        assert_eq!(s.origin, v2(s.pad, s.band.unwrap().h + s.pad));
        assert_eq!(s.content, s.view - Vec2::splat(2.0 * s.pad));
    }

    #[test]
    fn unstacked_has_no_band() {
        let s = Sizes::new(&Style::default(), v2(2000.0, 1000.0));
        assert_eq!(s.band, None);
        assert_eq!(s.screen_clip(), None);
        assert_eq!(s.origin, Vec2::splat(s.pad));
        assert_eq!(s.view, s.output);
    }

    #[test]
    fn portrait_resolution_is_the_short_side() {
        let mut style = Style::default();
        style.camera.stacked = true;
        let s = Sizes::new(&style, v2(1920.0, 1080.0));
        assert_eq!(s.render_size(1080), (1080, 1920));
        assert_eq!(s.render_size(720), (720, 1280));
    }

    #[test]
    fn no_padding_fills() {
        let mut style = Style::default();
        style.padding = 0.0;
        style.aspect = Aspect::Ratio { w: 16, h: 9 };
        let s = Sizes::new(&style, v2(1920.0, 1200.0));
        assert!(s.initial_zoom > 1.0);
        assert_eq!(s.render_size(1080), (1920, 1080));
    }
}
