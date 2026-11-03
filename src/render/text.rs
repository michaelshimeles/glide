//! Text rasterisation (Inter) for captions and keyboard badges.

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont};

use super::Image;

static SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// Cache key of a rendered text. `pop` is the highlighted word and its colour
/// for short-form captions.
pub fn key(text: &str, px: u32, badge: bool, pop: Option<(usize, [u8; 4])>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (text, px, badge, pop).hash(&mut h);
    h.finish()
}

/// Draw one glyph's coverage over `buf` (premultiplied RGBA) in `rgb`.
#[allow(clippy::too_many_arguments)]
fn stamp(font: &FontRef<'static>, buf: &mut [f32], (w, h): (u32, u32), scale: PxScale, id: GlyphId, at: (f32, f32), rgb: [f32; 3]) {
    let glyph = id.with_scale_and_position(scale, ab_glyph::point(at.0, at.1));
    let Some(outlined) = font.outline_glyph(glyph) else { return };
    let b = outlined.px_bounds();
    outlined.draw(|gx, gy, cov| {
        let (x, y) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            return;
        }
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        for c in 0..3 {
            buf[i + c] = rgb[c] * cov + buf[i + c] * (1.0 - cov);
        }
        buf[i + 3] = cov + buf[i + 3] * (1.0 - cov);
    });
}

pub struct TextCache {
    font: FontRef<'static>,
}

impl Default for TextCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TextCache {
    pub fn new() -> Self {
        TextCache { font: FontRef::try_from_slice(SEMIBOLD).expect("Inter font") }
    }

    /// Measure a single line at `px`.
    pub fn width(&self, text: &str, px: f32) -> f32 {
        let f = self.font.as_scaled(PxScale::from(px));
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id);
            prev = Some(id);
        }
        w
    }

    /// Render white text on a dark rounded backdrop. Badges get a pill with
    /// a hairline border; captions a softer box.
    pub fn render(&self, text: &str, px: f32, badge: bool) -> Option<Image> {
        if text.is_empty() {
            return None;
        }
        let f = self.font.as_scaled(PxScale::from(px));
        let pad_x = if badge { px * 0.75 } else { px * 0.55 };
        let pad_y = if badge { px * 0.45 } else { px * 0.3 };
        let tw = self.width(text, px);
        let (w, h) = ((tw + 2.0 * pad_x).ceil() as u32 + 2, (f.ascent() - f.descent() + 2.0 * pad_y).ceil() as u32 + 2);
        let mut buf = vec![0f32; (w * h * 4) as usize];
        // Backdrop.
        let radius = if badge { h as f32 / 2.0 } else { px * 0.45 };
        let (bg, border) = if badge { ([0.08, 0.08, 0.09, 0.82], 0.18) } else { ([0.0, 0.0, 0.0, 0.62], 0.0) };
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = sd_round_rect(fx, fy, w as f32, h as f32, radius);
                let cov = (0.5 - d).clamp(0.0, 1.0);
                let rim = if border > 0.0 { (1.0 - (d + 1.0).abs()).clamp(0.0, 1.0) * border } else { 0.0 };
                let a = cov * bg[3];
                let i = ((y * w + x) * 4) as usize;
                let c = [bg[0] * a + rim, bg[1] * a + rim, bg[2] * a + rim];
                buf[i..i + 3].copy_from_slice(&c);
                buf[i + 3] = (a + rim).min(1.0);
            }
        }
        // Glyphs (white, premultiplied over the backdrop).
        let mut x = pad_x + 1.0;
        let baseline = pad_y + 1.0 + f.ascent();
        let mut prev = None;
        for ch in text.chars() {
            let id = f.glyph_id(ch);
            if let Some(p) = prev {
                x += f.kern(p, id);
            }
            let glyph = id.with_scale_and_position(PxScale::from(px), ab_glyph::point(x, baseline));
            if let Some(outlined) = self.font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                outlined.draw(|gx, gy, cov| {
                    let (px_, py_) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px_ < 0 || py_ < 0 || px_ >= w as i32 || py_ >= h as i32 {
                        return;
                    }
                    let i = ((py_ as u32 * w + px_ as u32) * 4) as usize;
                    for c in 0..3 {
                        buf[i + c] = cov + buf[i + c] * (1.0 - cov);
                    }
                    buf[i + 3] = cov + buf[i + 3] * (1.0 - cov);
                });
            }
            x += f.h_advance(id);
            prev = Some(id);
        }
        let rgba = buf.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        Some(Image { width: w, height: h, rgba })
    }

    /// Render short-form video captions: white text with a black outline and
    /// no backdrop, word number `active` in `highlight` (RGBA, 0..1).
    pub fn render_pop(&self, text: &str, px: f32, active: usize, highlight: [f32; 4]) -> Option<Image> {
        if text.is_empty() {
            return None;
        }
        let scale = PxScale::from(px);
        let f = self.font.as_scaled(scale);
        let stroke = (px * 0.08).max(1.5);
        let pad = stroke.ceil() + 2.0;
        let space = f.h_advance(f.glyph_id(' '));
        // Where every glyph goes, and which word it belongs to.
        let mut glyphs = vec![];
        let mut x = pad;
        for (word, chars) in text.split(' ').enumerate() {
            let mut prev = None;
            for ch in chars.chars() {
                let id = f.glyph_id(ch);
                if let Some(p) = prev {
                    x += f.kern(p, id);
                }
                glyphs.push((id, x, word));
                x += f.h_advance(id);
                prev = Some(id);
            }
            x += space;
        }
        let (w, h) = (((x - space + pad).ceil() as u32 + 1), (f.ascent() - f.descent() + 2.0 * pad).ceil() as u32 + 1);
        let baseline = pad + f.ascent();
        let mut buf = vec![0f32; (w * h * 4) as usize];
        // The outline: the glyph stamped in black on two rings around its place.
        for &(id, gx, _) in &glyphs {
            for (radius, steps) in [(stroke, 16), (stroke * 0.5, 8)] {
                for s in 0..steps {
                    let a = std::f32::consts::TAU * s as f32 / steps as f32;
                    stamp(&self.font, &mut buf, (w, h), scale, id, (gx + a.cos() * radius, baseline + a.sin() * radius), [0.0; 3]);
                }
            }
        }
        for &(id, gx, word) in &glyphs {
            let rgb = if word == active { [highlight[0], highlight[1], highlight[2]] } else { [1.0; 3] };
            stamp(&self.font, &mut buf, (w, h), scale, id, (gx, baseline), rgb);
        }
        let rgba = buf.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
        Some(Image { width: w, height: h, rgba })
    }
}

fn sd_round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0);
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    (qx.max(0.0).hypot(qy.max(0.0))) + qx.max(qy).min(0.0) - r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixels that are opaque and close to the highlight colour.
    fn highlighted(img: &Image) -> usize {
        img.rgba.chunks(4).filter(|p| p[3] > 240 && p[0] > 230 && (190..230).contains(&p[1]) && p[2] < 40).count()
    }

    #[test]
    fn pop_highlights_only_the_active_word() {
        let t = TextCache::new();
        let yellow = [1.0, 0.84, 0.04, 1.0];
        let first = t.render_pop("HELLO WORLD", 80.0, 0, yellow).unwrap();
        let second = t.render_pop("HELLO WORLD", 80.0, 1, yellow).unwrap();
        let none = t.render_pop("HELLO WORLD", 80.0, 7, yellow).unwrap();
        assert_eq!((first.width, first.height), (second.width, second.height));
        assert!(highlighted(&first) > 500 && highlighted(&second) > 500);
        assert_eq!(highlighted(&none), 0);
        assert_ne!(first.rgba, second.rgba);
        // A black outline surrounds the letters: opaque pixels that are dark.
        assert!(first.rgba.chunks(4).any(|p| p[3] > 240 && p[0] < 20 && p[1] < 20 && p[2] < 20));
        assert!(t.render_pop("", 80.0, 0, yellow).is_none());
    }

    #[test]
    fn cache_key_tells_highlights_apart() {
        let a = key("A B", 40, false, Some((0, [255, 214, 10, 255])));
        assert_ne!(a, key("A B", 40, false, Some((1, [255, 214, 10, 255]))));
        assert_ne!(a, key("A B", 40, false, Some((0, [255, 0, 0, 255]))));
        assert_ne!(a, key("A B", 40, false, None));
    }
}
