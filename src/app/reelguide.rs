//! The "Reel mode" guide: an overlay on the editor preview that shows the 9:16
//! frame a phone feed displays with a TikTok-style interface drawn over it
//! (tabs, side buttons, profile and caption), so you can see what the apps
//! will cover. Everything is drawn here, with placeholder text. Nothing in it
//! reaches the export.

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

use super::theme;

/// Width of the feed the positions below were measured on. Sizes scale with
/// the feed's width relative to this.
const REF_W: f32 = 616.0;

/// Where each part sits, as fractions of the feed's width (x) and height (y).
const ISLAND_Y: f32 = 0.017;
const TOP_ROW_Y: f32 = 0.108;
const TV_X: f32 = 0.073;
const FOLLOWING_X: f32 = 0.40;
const FOR_YOU_X: f32 = 0.619;
const SEARCH_X: f32 = 0.927;
const RAIL_X: f32 = 0.912;
/// Avatar, heart, comments, bookmark, share and the sound disc, top to bottom.
const RAIL_Y: [f32; 6] = [0.487, 0.566, 0.659, 0.753, 0.849, 0.947];
/// Counts under the heart, comments, bookmark and share.
const COUNT_Y: [f32; 4] = [0.602, 0.696, 0.791, 0.885];
const TEXT_X: f32 = 0.042;
const USER_Y: f32 = 0.864;
const CAPTION_Y: [f32; 2] = [0.901, 0.926];
const SOUND_Y: f32 = 0.961;

/// The 9:16 frame a feed shows, for a preview drawn in `preview`. A preview
/// wider than 9:16 gets the frame centred in it (a feed crops the rest), a
/// narrower one gets the frame centred vertically.
pub fn feed(preview: Rect) -> Rect {
    let aspect = 9.0 / 16.0;
    let mut size = vec2(preview.height() * aspect, preview.height());
    if size.x > preview.width() {
        size = vec2(preview.width(), preview.width() / aspect);
    }
    Rect::from_center_size(preview.center(), size)
}

/// Positions of every part of the interface in `feed`.
struct Spots {
    /// Scale from the reference feed to this one.
    k: f32,
    island: Rect,
    tv: Pos2,
    following: Pos2,
    for_you: Pos2,
    search: Pos2,
    rail: [Pos2; 6],
    counts: [Pos2; 4],
    user: Pos2,
    caption: [Pos2; 2],
    sound: Pos2,
}

fn spots(feed: Rect) -> Spots {
    let at = |fx: f32, fy: f32| pos2(feed.left() + feed.width() * fx, feed.top() + feed.height() * fy);
    let k = feed.width() / REF_W;
    Spots {
        k,
        island: Rect::from_center_size(at(0.5, ISLAND_Y) + vec2(0.0, 28.0 * k), vec2(196.0 * k, 56.0 * k)),
        tv: at(TV_X, TOP_ROW_Y),
        following: at(FOLLOWING_X, TOP_ROW_Y),
        for_you: at(FOR_YOU_X, TOP_ROW_Y),
        search: at(SEARCH_X, TOP_ROW_Y),
        rail: RAIL_Y.map(|y| at(RAIL_X, y)),
        counts: COUNT_Y.map(|y| at(RAIL_X, y)),
        user: at(TEXT_X, USER_Y),
        caption: CAPTION_Y.map(|y| at(TEXT_X, y)),
        sound: at(TEXT_X, SOUND_Y),
    }
}

/// Draw the guide over `preview`: everything the feed crops is black, so
/// what's left is exactly what a viewer sees, and the interface is drawn
/// over the feed.
pub fn paint(painter: &Painter, preview: Rect) {
    let feed = feed(preview);
    let crop = Color32::BLACK;
    for r in [
        Rect::from_min_max(preview.min, pos2(feed.left(), preview.bottom())),
        Rect::from_min_max(pos2(feed.right(), preview.top()), preview.max),
        Rect::from_min_max(pos2(feed.left(), preview.top()), pos2(feed.right(), feed.top())),
        Rect::from_min_max(pos2(feed.left(), feed.bottom()), pos2(feed.right(), preview.bottom())),
    ] {
        if r.is_positive() {
            painter.rect_filled(r.intersect(preview), CornerRadius::ZERO, crop);
        }
    }
    paint_interface(&painter.with_clip_rect(feed.intersect(preview)), feed);
}

/// A vertical gradient from `top` to `bottom` over `rect`.
fn scrim(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [(rect.left_top(), top), (rect.right_top(), top), (rect.right_bottom(), bottom), (rect.left_bottom(), bottom)] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
}

/// Draw an icon twice: a soft shadow under it, then the icon itself.
fn with_shadow(k: f32, draw: impl Fn(egui::Vec2, Color32)) {
    draw(vec2(0.0, 2.0 * k), Color32::from_black_alpha(90));
    draw(vec2(0.0, 0.0), Color32::WHITE);
}

fn paint_interface(p: &Painter, feed: Rect) {
    let s = spots(feed);
    let k = s.k;
    let white = Color32::WHITE;

    // The feeds darken the top and bottom so their white text stays readable.
    let (top, bottom) = (feed.height() * 0.16, feed.height() * 0.3);
    scrim(p, Rect::from_min_size(feed.min, vec2(feed.width(), top)), Color32::from_black_alpha(80), Color32::TRANSPARENT);
    scrim(p, Rect::from_min_max(pos2(feed.left(), feed.bottom() - bottom), feed.max), Color32::TRANSPARENT, Color32::from_black_alpha(150));

    p.rect_filled(s.island, CornerRadius::same(255), Color32::BLACK);

    // Top row: live, tabs, search.
    with_shadow(k, |o, c| icon_tv(p, s.tv + o, 38.0 * k, c));
    let tab = |pos: Pos2, text: &str, color: Color32| {
        p.text(pos + vec2(0.0, 1.5 * k), Align2::CENTER_CENTER, text, theme::semibold(26.0 * k), Color32::from_black_alpha(110));
        p.text(pos, Align2::CENTER_CENTER, text, theme::semibold(26.0 * k), color)
    };
    tab(s.following, "Following", white.gamma_multiply(0.7));
    let you = tab(s.for_you, "For You", white);
    p.rect_filled(Rect::from_center_size(pos2(you.center().x, you.bottom() + 6.0 * k), vec2(you.width(), 3.0 * k)), CornerRadius::ZERO, white);
    with_shadow(k, |o, c| icon_search(p, s.search + o, 38.0 * k, c));

    // Right column.
    let u = 40.0 * k;
    icon_avatar(p, s.rail[0], 36.0 * k);
    with_shadow(k, |o, c| icon_heart(p, s.rail[1] + o, u, c));
    with_shadow(k, |o, c| icon_comment(p, s.rail[2] + o, u, c));
    with_shadow(k, |o, c| icon_bookmark(p, s.rail[3] + o, u, c));
    with_shadow(k, |o, c| icon_share(p, s.rail[4] + o, u, c));
    icon_disc(p, s.rail[5], 36.0 * k);
    for (pos, text) in s.counts.iter().zip(["25.1K", "1,862", "3,420", "971"]) {
        shadowed(p, *pos, Align2::CENTER_CENTER, text, theme::semibold(18.0 * k), white, k);
    }

    // Profile, caption and sound.
    shadowed(p, s.user, Align2::LEFT_CENTER, "@yourname", theme::semibold(24.0 * k), white, k);
    for (pos, text) in s.caption.iter().zip(["Add your caption and hashtags here", "#fyp #foryou"]) {
        shadowed(p, *pos, Align2::LEFT_CENTER, text, FontId::proportional(21.0 * k), white, k);
    }
    with_shadow(k, |o, c| icon_note(p, s.sound + o, 20.0 * k, c));
    shadowed(p, s.sound + vec2(28.0 * k, 0.0), Align2::LEFT_CENTER, "original sound \u{2014} yourname", FontId::proportional(20.0 * k), white, k);
}

/// Text with a soft shadow so it reads over light content.
#[allow(clippy::too_many_arguments)]
fn shadowed(p: &Painter, pos: Pos2, anchor: Align2, text: &str, font: FontId, color: Color32, k: f32) {
    p.text(pos + vec2(0.0, 1.5 * k), anchor, text, font.clone(), Color32::from_black_alpha(110));
    p.text(pos, anchor, text, font, color);
}

fn fill(p: &Painter, points: Vec<Pos2>, color: Color32) {
    p.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

fn icon_heart(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let r = 0.27 * u;
    p.circle_filled(c + vec2(-0.235 * u, -0.14 * u), r, color);
    p.circle_filled(c + vec2(0.235 * u, -0.14 * u), r, color);
    fill(p, vec![c + vec2(-0.49 * u, -0.08 * u), c + vec2(0.49 * u, -0.08 * u), c + vec2(0.0, 0.47 * u)], color);
}

fn icon_comment(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let body = Rect::from_center_size(c + vec2(0.0, -0.04 * u), vec2(0.92 * u, 0.7 * u));
    p.rect_filled(body, CornerRadius::same((0.3 * u) as u8), color);
    fill(p, vec![c + vec2(-0.3 * u, 0.24 * u), c + vec2(-0.05 * u, 0.24 * u), c + vec2(-0.3 * u, 0.5 * u)], color);
    for dx in [-0.2, 0.0, 0.2] {
        p.circle_filled(c + vec2(dx * u, -0.04 * u), 0.055 * u, Color32::from_black_alpha(170));
    }
}

fn icon_bookmark(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let at = |x: f32, y: f32| c + vec2(x * u, y * u);
    p.rect_filled(Rect::from_min_max(at(-0.31, -0.43), at(0.31, 0.22)), CornerRadius::same((0.05 * u) as u8), color);
    // The two legs under the notch.
    fill(p, vec![at(-0.31, 0.2), at(0.0, 0.2), at(-0.31, 0.43)], color);
    fill(p, vec![at(0.0, 0.2), at(0.31, 0.2), at(0.31, 0.43)], color);
}

fn icon_share(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let at = |x: f32, y: f32| c + vec2(x * u, y * u);
    let tail = [at(-0.42, 0.4), at(-0.42, 0.12), at(-0.3, -0.08), at(-0.1, -0.17), at(0.12, -0.19)];
    p.add(Shape::line(tail.to_vec(), Stroke::new(0.13 * u, color)));
    fill(p, vec![at(0.08, -0.48), at(0.52, -0.19), at(0.08, 0.1)], color);
}

fn icon_search(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let ring = c + vec2(-0.06 * u, -0.06 * u);
    p.circle_stroke(ring, 0.28 * u, Stroke::new(0.1 * u, color));
    p.line_segment([ring + vec2(0.2 * u, 0.2 * u), c + vec2(0.4 * u, 0.4 * u)], Stroke::new(0.1 * u, color));
}

fn icon_tv(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let body = Rect::from_center_size(c + vec2(0.0, 0.1 * u), vec2(0.84 * u, 0.58 * u));
    p.rect_stroke(body, CornerRadius::same((0.12 * u) as u8), Stroke::new(0.09 * u, color), StrokeKind::Inside);
    let stroke = Stroke::new(0.08 * u, color);
    p.line_segment([c + vec2(-0.04 * u, -0.2 * u), c + vec2(-0.22 * u, -0.42 * u)], stroke);
    p.line_segment([c + vec2(0.04 * u, -0.2 * u), c + vec2(0.22 * u, -0.42 * u)], stroke);
}

fn icon_avatar(p: &Painter, c: Pos2, r: f32) {
    p.circle_filled(c, r, Color32::from_rgb(0x6d, 0x4f, 0xe8));
    p.circle_filled(c + vec2(-0.22 * r, -0.25 * r), 0.62 * r, Color32::from_rgba_unmultiplied(0xb4, 0x9d, 0xff, 150));
    p.circle_stroke(c, r, Stroke::new(0.08 * r, Color32::WHITE));
}

fn icon_disc(p: &Painter, c: Pos2, r: f32) {
    p.circle_filled(c, r, Color32::from_rgba_unmultiplied(30, 30, 34, 220));
    p.circle_stroke(c, r, Stroke::new(0.06 * r, Color32::from_white_alpha(40)));
    p.circle_filled(c, 0.38 * r, Color32::from_rgb(0x4f, 0x46, 0xe5));
}

/// A music note, `u` tall, centred vertically on `c.y` and starting at `c.x`.
fn icon_note(p: &Painter, c: Pos2, u: f32, color: Color32) {
    let head = c + vec2(0.22 * u, 0.28 * u);
    p.circle_filled(head, 0.2 * u, color);
    let stem = Stroke::new(0.09 * u, color);
    p.line_segment([head + vec2(0.17 * u, 0.0), c + vec2(0.39 * u, -0.45 * u)], stem);
    p.line_segment([c + vec2(0.39 * u, -0.45 * u), c + vec2(0.72 * u, -0.28 * u)], stem);
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
        let f = feed(preview);
        assert!(close(f.width(), 360.0) && close(f.height(), 640.0));
        assert!(close(f.min.x, 10.0) && close(f.min.y, 20.0));
    }

    #[test]
    fn a_wide_preview_gets_a_centred_feed() {
        let f = feed(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 900.0)));
        assert!(close(f.height(), 900.0) && close(f.width(), 506.25));
        assert!(close(f.center().x, 800.0));
    }

    #[test]
    fn a_narrow_preview_gets_a_feed_that_fits_its_width() {
        let preview = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 900.0));
        let f = feed(preview);
        assert!(close(f.width(), 300.0) && close(f.height(), 533.33));
        assert!(close(f.center().y, 450.0));
        assert!(preview.contains_rect(f));
    }

    #[test]
    fn the_interface_sits_inside_the_feed_in_reading_order() {
        let f = feed(Rect::from_min_size(pos2(0.0, 0.0), vec2(616.0, 1095.0)));
        let s = spots(f);
        assert!(close(s.k, 1.0));
        for p in [s.tv, s.following, s.for_you, s.search, s.user, s.sound].into_iter().chain(s.rail).chain(s.counts).chain(s.caption) {
            assert!(f.contains(p), "{p:?} is outside {f:?}");
        }
        assert!(f.contains_rect(s.island));
        // The side column hugs the right edge; the text starts at the left.
        assert!(close(s.rail[0].x / f.width(), 0.912));
        assert!(s.user.x < s.following.x && s.search.x > s.for_you.x);
        // Top to bottom: island, tabs, avatar... disc, then the profile text above the sound line.
        assert!(s.island.bottom() < s.for_you.y && s.for_you.y < s.rail[0].y);
        assert!(s.rail.windows(2).all(|w| w[0].y < w[1].y));
        assert!(s.user.y < s.caption[0].y && s.caption[0].y < s.caption[1].y && s.caption[1].y < s.sound.y);
        // Every count sits just under its icon, above the next one.
        for (i, c) in s.counts.iter().enumerate() {
            assert!(s.rail[i + 1].y < c.y && c.y < s.rail[i + 2].y);
        }
    }

    #[test]
    fn sizes_scale_with_the_feed() {
        let big = spots(Rect::from_min_size(pos2(0.0, 0.0), vec2(1232.0, 2190.0)));
        assert!(close(big.k, 2.0));
        assert!(close(big.island.width(), 392.0));
        assert!(close(big.rail[0].x, 0.912 * 1232.0));
    }
}
