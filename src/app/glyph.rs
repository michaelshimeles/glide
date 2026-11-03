//! Line icons for the editor's rails and toolbars, drawn with the painter so
//! they need no image files.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Frame,
    Cursor,
    Camera,
    Captions,
    Audio,
    Keys,
    Motion,
    Scissors,
    ZoomPlus,
    Mask,
    Shake,
    Trash,
    Loop,
    Prev,
    Next,
    Play,
    Pause,
    Crop,
    Phone,
    Chevron,
}

/// Draw `g` centred on `c`, about `size` points across.
pub fn paint(p: &Painter, c: Pos2, size: f32, g: Glyph, color: Color32) {
    // Everything below is laid out on a 24 x 24 grid around the origin.
    let u = size / 24.0;
    let at = |x: f32, y: f32| c + vec2(x * u, y * u);
    let stroke = Stroke::new((1.6 * u).max(1.0), color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let poly = |pts: &[(f32, f32)]| {
        p.add(Shape::closed_line(pts.iter().map(|&(x, y)| at(x, y)).collect(), stroke));
    };
    let fill = |pts: &[(f32, f32)]| {
        p.add(Shape::convex_polygon(pts.iter().map(|&(x, y)| at(x, y)).collect(), color, Stroke::NONE));
    };
    let outline = |a: (f32, f32), b: (f32, f32), r: f32| {
        p.rect_stroke(Rect::from_min_max(at(a.0, a.1), at(b.0, b.1)), CornerRadius::same((r * u).round() as u8), stroke, StrokeKind::Inside);
    };
    let circle = |x: f32, y: f32, r: f32| {
        p.circle_stroke(at(x, y), r * u, stroke);
    };
    let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<(f32, f32)> {
        (0..=16)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / 16.0).to_radians();
                (cx + r * a.cos(), cy - r * a.sin())
            })
            .collect()
    };
    match g {
        Glyph::Frame => {
            outline((-9.0, -7.0), (9.0, 7.0), 3.0);
            circle(3.0, -2.0, 2.0);
            line(&[(-9.0, 3.0), (-4.0, -1.0), (0.0, 3.0), (3.0, 1.0), (9.0, 5.0)]);
        }
        Glyph::Cursor => poly(&[(-4.0, -9.0), (-4.0, 6.5), (-0.5, 3.0), (2.5, 9.5), (5.0, 8.5), (2.0, 2.5), (7.0, 2.0)]),
        Glyph::Camera => {
            outline((-9.0, -5.0), (9.0, 7.0), 3.0);
            circle(0.0, 1.0, 3.0);
            line(&[(-4.0, -5.0), (-2.0, -8.0), (2.0, -8.0), (4.0, -5.0)]);
        }
        Glyph::Captions => {
            outline((-9.0, -8.0), (9.0, 4.0), 3.0);
            line(&[(-3.0, 4.0), (-3.0, 9.0), (2.0, 4.0)]);
            line(&[(-5.0, -3.0), (5.0, -3.0)]);
            line(&[(-5.0, 0.0), (2.0, 0.0)]);
        }
        Glyph::Audio => {
            poly(&[(-9.0, -3.0), (-5.0, -3.0), (0.0, -8.0), (0.0, 8.0), (-5.0, 3.0), (-9.0, 3.0)]);
            line(&arc(1.0, 0.0, 4.5, -50.0, 50.0));
            line(&arc(1.0, 0.0, 8.5, -50.0, 50.0));
        }
        Glyph::Keys => {
            outline((-4.0, -4.0), (4.0, 4.0), 0.0);
            for (x, y) in [(-6.0, -6.0), (6.0, -6.0), (-6.0, 6.0), (6.0, 6.0)] {
                circle(x, y, 2.6);
            }
        }
        Glyph::Motion => {
            circle(-7.0, 7.0, 2.2);
            circle(7.0, -7.0, 2.2);
            line(&[(-5.0, 7.0), (1.0, 7.0), (4.0, 4.0), (1.0, 1.0), (-2.0, -2.0), (1.0, -7.0), (4.8, -7.0)]);
        }
        Glyph::Scissors => {
            circle(-5.0, 7.0, 3.0);
            circle(5.0, 7.0, 3.0);
            line(&[(-4.0, 4.5), (6.0, -9.0)]);
            line(&[(4.0, 4.5), (-6.0, -9.0)]);
        }
        Glyph::ZoomPlus => {
            circle(-2.0, -2.0, 7.0);
            line(&[(3.0, 3.0), (9.0, 9.0)]);
            line(&[(-5.0, -2.0), (1.0, -2.0)]);
            line(&[(-2.0, -5.0), (-2.0, 1.0)]);
        }
        Glyph::Mask => {
            outline((-8.0, -8.0), (8.0, 8.0), 2.0);
            circle(0.0, 0.0, 4.0);
        }
        // A sound wave: bars of different heights.
        Glyph::Shake => {
            for (x, h) in [(-8.0, 3.0), (-4.0, 7.0), (0.0, 10.0), (4.0, 6.0), (8.0, 3.0)] {
                line(&[(x, -h), (x, h)]);
            }
        }
        Glyph::Trash => {
            line(&[(-8.0, -5.0), (8.0, -5.0)]);
            line(&[(-3.0, -5.0), (-3.0, -8.0), (3.0, -8.0), (3.0, -5.0)]);
            line(&[(-6.0, -5.0), (-5.0, 8.0), (5.0, 8.0), (6.0, -5.0)]);
            line(&[(-2.0, -1.0), (-2.0, 5.0)]);
            line(&[(2.0, -1.0), (2.0, 5.0)]);
        }
        Glyph::Loop => {
            let (r, from, to) = (7.0f32, 40.0f32, 330.0f32);
            line(&arc(0.0, 0.0, r, from, to));
            // Arrow head at the end of the arc, along its direction.
            let a = to.to_radians();
            let end = (r * a.cos(), -r * a.sin());
            let dir = (-a.sin(), -a.cos());
            let n = (a.cos(), -a.sin());
            fill(&[
                (end.0 + dir.0 * 4.5, end.1 + dir.1 * 4.5),
                (end.0 + n.0 * 3.2 - dir.0 * 0.5, end.1 + n.1 * 3.2 - dir.1 * 0.5),
                (end.0 - n.0 * 3.2 - dir.0 * 0.5, end.1 - n.1 * 3.2 - dir.1 * 0.5),
            ]);
        }
        Glyph::Prev => {
            line(&[(-7.0, -7.0), (-7.0, 7.0)]);
            fill(&[(8.0, -7.0), (8.0, 7.0), (-4.0, 0.0)]);
        }
        Glyph::Next => {
            line(&[(7.0, -7.0), (7.0, 7.0)]);
            fill(&[(-8.0, -7.0), (-8.0, 7.0), (4.0, 0.0)]);
        }
        Glyph::Play => fill(&[(-5.0, -8.0), (-5.0, 8.0), (8.0, 0.0)]),
        Glyph::Pause => {
            fill(&[(-6.0, -7.0), (-2.0, -7.0), (-2.0, 7.0), (-6.0, 7.0)]);
            fill(&[(2.0, -7.0), (6.0, -7.0), (6.0, 7.0), (2.0, 7.0)]);
        }
        Glyph::Crop => {
            line(&[(-4.0, -9.0), (-4.0, 4.0), (9.0, 4.0)]);
            line(&[(-9.0, -4.0), (4.0, -4.0), (4.0, 9.0)]);
        }
        Glyph::Phone => {
            outline((-5.0, -9.0), (5.0, 9.0), 3.0);
            line(&[(-2.0, 6.0), (2.0, 6.0)]);
        }
        Glyph::Chevron => line(&[(-4.0, -2.0), (0.0, 2.0), (4.0, -2.0)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    /// Draw every glyph into a painter: none may panic.
    #[test]
    fn every_glyph_paints() {
        let ctx = egui::Context::default();
        let layer = egui::LayerId::background();
        let painter = Painter::new(ctx, layer, Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0)));
        for g in [
            Glyph::Frame,
            Glyph::Cursor,
            Glyph::Camera,
            Glyph::Captions,
            Glyph::Audio,
            Glyph::Keys,
            Glyph::Motion,
            Glyph::Scissors,
            Glyph::ZoomPlus,
            Glyph::Mask,
            Glyph::Shake,
            Glyph::Trash,
            Glyph::Loop,
            Glyph::Prev,
            Glyph::Next,
            Glyph::Play,
            Glyph::Pause,
            Glyph::Crop,
            Glyph::Phone,
            Glyph::Chevron,
        ] {
            paint(&painter, pos2(50.0, 50.0), 20.0, g, Color32::WHITE);
        }
    }
}
