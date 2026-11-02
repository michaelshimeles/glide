//! The "Reels / TikTok" guide: an overlay on the editor preview that shows
//! the 9:16 frame a phone feed displays and the part of it the apps' own
//! buttons, captions and profile bar leave free. Nothing here reaches the
//! export.

use egui::{Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

/// Margins of a 1080x1920 Reel or TikTok that the apps cover, as fractions of
/// the frame: the stricter of the two apps on each side, so a frame that
/// fits stays clear in both. The apps move their interface around between
/// versions, so treat these as a guide and not a promise.
const TOP: f32 = 270.0 / 1920.0;
const BOTTOM: f32 = 670.0 / 1920.0;
const LEFT: f32 = 65.0 / 1080.0;
const RIGHT: f32 = 120.0 / 1080.0;

/// The 9:16 frame a feed shows and the free area inside it, for a preview
/// drawn in `preview`. A preview wider than 9:16 gets the frame centred in
/// it (a feed crops the rest), a narrower one gets the frame centred
/// vertically.
pub fn frames(preview: Rect) -> (Rect, Rect) {
    let aspect = 9.0 / 16.0;
    let mut size = vec2(preview.height() * aspect, preview.height());
    if size.x > preview.width() {
        size = vec2(preview.width(), preview.width() / aspect);
    }
    let feed = Rect::from_center_size(preview.center(), size);
    let safe = Rect::from_min_max(
        pos2(feed.left() + feed.width() * LEFT, feed.top() + feed.height() * TOP),
        pos2(feed.right() - feed.width() * RIGHT, feed.bottom() - feed.height() * BOTTOM),
    );
    (feed, safe)
}

/// Draw the guide over `preview`: everything the feed crops is dimmed, the
/// area the apps' interface covers is tinted and the free area outlined.
pub fn paint(painter: &Painter, preview: Rect) {
    let (feed, safe) = frames(preview);
    let dim = Color32::from_black_alpha(150);
    let tint = Color32::from_rgba_unmultiplied(255, 69, 58, 46);
    let line = Color32::from_white_alpha(230);

    // Outside the feed: cropped away.
    for r in [
        Rect::from_min_max(preview.min, pos2(feed.left(), preview.bottom())),
        Rect::from_min_max(pos2(feed.right(), preview.top()), preview.max),
        Rect::from_min_max(pos2(feed.left(), preview.top()), pos2(feed.right(), feed.top())),
        Rect::from_min_max(pos2(feed.left(), feed.bottom()), pos2(feed.right(), preview.bottom())),
    ] {
        if r.is_positive() {
            painter.rect_filled(r.intersect(preview), CornerRadius::ZERO, dim);
        }
    }
    // Inside the feed but under the apps' interface: four bands around the free area.
    for r in [
        Rect::from_min_max(feed.min, pos2(feed.right(), safe.top())),
        Rect::from_min_max(pos2(feed.left(), safe.bottom()), feed.max),
        Rect::from_min_max(pos2(feed.left(), safe.top()), pos2(safe.left(), safe.bottom())),
        Rect::from_min_max(pos2(safe.right(), safe.top()), pos2(feed.right(), safe.bottom())),
    ] {
        painter.rect_filled(r.intersect(preview), CornerRadius::ZERO, tint);
    }
    painter.rect_stroke(feed.intersect(preview), CornerRadius::ZERO, Stroke::new(1.0, line.gamma_multiply(0.5)), StrokeKind::Inside);
    // Dark under-stroke and label background: the guide stays readable over light content.
    let free = safe.intersect(preview);
    painter.rect_stroke(free, CornerRadius::same(2), Stroke::new(3.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
    painter.rect_stroke(free, CornerRadius::same(2), Stroke::new(1.5, line), StrokeKind::Inside);
    let label = painter.layout_no_wrap("Reels / TikTok safe area".into(), egui::FontId::proportional(11.0), line);
    let at = pos2(safe.left() + 6.0, safe.top() + 6.0);
    painter.rect_filled(Rect::from_min_size(at, label.size()).expand2(vec2(5.0, 2.0)), CornerRadius::same(4), Color32::from_black_alpha(170));
    painter.galley(at, label, line);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_vertical_preview_is_the_feed() {
        let preview = Rect::from_min_size(pos2(10.0, 20.0), vec2(360.0, 640.0));
        let (feed, safe) = frames(preview);
        assert!(close(feed.width(), 360.0) && close(feed.height(), 640.0));
        assert!(close(feed.min.x, 10.0) && close(feed.min.y, 20.0));
        // 1080x1920 margins: 270 px top, 670 px bottom, 65 px left, 120 px right, at one third scale.
        assert!(close(safe.top() - feed.top(), 90.0));
        assert!(close(feed.bottom() - safe.bottom(), 670.0 / 3.0));
        assert!(close(safe.left() - feed.left(), 65.0 / 3.0));
        assert!(close(feed.right() - safe.right(), 40.0));
    }

    #[test]
    fn a_wide_preview_gets_a_centred_feed() {
        let preview = Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0));
        let (feed, safe) = frames(preview);
        assert!(close(feed.height(), 900.0) && close(feed.width(), 506.25));
        assert!(close(feed.center().x, 800.0));
        assert!(feed.contains_rect(safe));
    }

    #[test]
    fn a_narrow_preview_gets_a_feed_that_fits_its_width() {
        let preview = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 900.0));
        let (feed, _) = frames(preview);
        assert!(close(feed.width(), 300.0) && close(feed.height(), 533.33));
        assert!(close(feed.center().y, 450.0));
        assert!(preview.contains_rect(feed));
    }
}
