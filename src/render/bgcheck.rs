//! Background self-check: render a frame under every Background setting
//! (wallpapers, blur, gradients, colours, an image) and the padding,
//! roundness and shadow that frame it, and measure how much of the picture
//! each edit changes. Used by the tests (through the editor preview) and by
//! `daisycap check-background` (on real recordings).

use std::path::Path;

use anyhow::Result;

use super::Image;
use super::art::WALLPAPERS;
use crate::project::model::{Aspect, Background, Color, Style};

/// Which pixels a step is judged on.
#[derive(Clone, Copy, Debug)]
pub enum Region {
    /// The band along the frame's edges: background whenever there's padding.
    Border,
    All,
}

/// One edit, applied on top of the previous step's style.
pub struct Step {
    pub name: String,
    pub edit: Box<dyn Fn(&mut Style)>,
    pub region: Region,
    /// Colour distance (sum over RGB) a pixel must move to count.
    pub tol: i32,
    /// Fraction of the region that must change.
    pub min: f64,
}

/// What a step did.
#[derive(Debug)]
pub struct Outcome {
    pub name: String,
    pub changed: f64,
    pub min: f64,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.changed >= self.min
    }
}

/// The style every check starts from: the first wallpaper, unblurred, in
/// a fixed frame (with Auto, padding changes the frame's size).
pub fn base(style: &Style) -> Style {
    let mut s = style.clone();
    if s.aspect == Aspect::Auto {
        s.aspect = Aspect::Ratio { w: 16, h: 10 };
    }
    s.background = Background::Wallpaper { id: WALLPAPERS[0].0.into() };
    s.background_blur = 0.0;
    if s.padding <= 0.0 {
        // Edge to edge the background isn't drawn at all.
        s.padding = Style::REVEAL_PADDING;
    }
    s
}

fn step(name: &str, region: Region, tol: i32, min: f64, edit: impl Fn(&mut Style) + 'static) -> Step {
    Step { name: name.into(), edit: Box::new(edit), region, tol, min }
}

/// Every check, in order. `image` is a picture to use as a background.
pub fn steps(image: &str) -> Vec<Step> {
    use Region::*;
    let mut out: Vec<Step> = WALLPAPERS
        .iter()
        .skip(1)
        .map(|(id, name, ..)| step(&format!("wallpaper {name}"), Border, 30, 0.3, move |s| s.background = Background::Wallpaper { id: (*id).into() }))
        .collect();
    let dusk = vec![(Color::rgb(255, 120, 80), 0.0), (Color::rgb(130, 70, 220), 1.0)];
    let dusk2 = dusk.clone();
    let image = image.to_string();
    out.extend([
        step("wallpaper blur", Border, 12, 0.02, |s| s.background_blur = 40.0),
        step("wallpaper unblurred", Border, 12, 0.02, |s| s.background_blur = 0.0),
        step("gradient", Border, 30, 0.5, move |s| s.background = Background::Gradient { angle: 135.0, stops: dusk.clone() }),
        step("gradient angle", Border, 30, 0.3, move |s| s.background = Background::Gradient { angle: 315.0, stops: dusk2.clone() }),
        step("gradient colours", Border, 30, 0.5, |s| {
            s.background = Background::Gradient { angle: 315.0, stops: vec![(Color::rgb(40, 160, 255), 0.0), (Color::rgb(20, 220, 190), 1.0)] }
        }),
        step("colour", Border, 30, 0.5, |s| s.background = Background::Color { color: Color::rgb(52, 120, 246) }),
        step("another colour", Border, 30, 0.95, |s| s.background = Background::Color { color: Color::rgb(220, 40, 40) }),
        step("image", Border, 30, 0.9, move |s| s.background = Background::Image { path: image.clone() }),
        step("image blur", Border, 30, 0.2, |s| s.background_blur = 30.0),
        // A plain backdrop with a square, shadowless frame for the next three.
        step("plain backdrop", Border, 30, 0.9, |s| {
            s.background = Background::Color { color: Color::rgb(70, 150, 230) };
            s.background_blur = 0.0;
            s.corner_radius = 0.0;
            s.shadow.intensity = 0.0;
        }),
        step("padding", All, 30, 0.1, |s| s.padding += 15.0),
        // 40 px corners (at 1080p): about 0.07 % of a 16:10 frame.
        step("roundness", All, 30, 0.0004, |s| s.corner_radius = 40.0),
        step("shadow", All, 12, 0.02, |s| {
            s.shadow.intensity = 1.0;
            s.shadow.blur = 40.0;
        }),
    ]);
    out
}

/// Fraction of the pixels in `region` whose colour moved by more than `tol`.
pub fn changed(a: &Image, b: &Image, region: Region, tol: i32) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height), "frames of different sizes");
    let (w, h) = (a.width, a.height);
    let band = (w.min(h) as f64 * 0.03).ceil() as u32;
    let (mut n, mut moved) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if matches!(region, Region::Border) && !(x < band || y < band || x >= w - band || y >= h - band) {
                continue;
            }
            n += 1;
            let i = ((y * w + x) * 4) as usize;
            let d: i32 = (0..3).map(|c| (a.rgba[i + c] as i32 - b.rgba[i + c] as i32).abs()).sum();
            if d > tol {
                moved += 1;
            }
        }
    }
    moved as f64 / n.max(1) as f64
}

/// Run every step, rendering with `render` (style → RGBA frame) and
/// comparing each frame with the one before it.
pub fn run(style: &Style, image: &str, mut render: impl FnMut(&Style) -> Result<Image>) -> Result<Vec<Outcome>> {
    let mut s = base(style);
    let mut prev = render(&s)?;
    let mut out = vec![];
    for st in steps(image) {
        (st.edit)(&mut s);
        let next = render(&s)?;
        out.push(Outcome { name: st.name, changed: changed(&next, &prev, st.region, st.tol), min: st.min });
        prev = next;
    }
    Ok(out)
}

/// Write a checkerboard PNG to use as the background image.
pub fn checkerboard(path: &Path) -> Result<()> {
    let (w, h) = (320u32, 200u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let on = (x / 20 + y / 20) % 2 == 0;
            px.extend_from_slice(if on { &[240, 200, 40, 255] } else { &[20, 60, 200, 255] });
        }
    }
    image::save_buffer(path, &px, w, h, image::ExtendedColorType::Rgba8)?;
    Ok(())
}
