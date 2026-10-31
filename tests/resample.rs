//! Export sharpness: at native size the screen is drawn pixel for pixel, and
//! downscaled exports filter fine detail instead of aliasing it.

use daisycap::engine::scene::Scene;
use daisycap::project::model::ExportSettings;
use daisycap::render::{Gpu, Image, Renderer, VideoFrame};

const W: u32 = 640;
const H: u32 = 400;

/// A 1-px black/white checkerboard: the finest detail a screen can show.
fn checker() -> Vec<u8> {
    let mut px = vec![0u8; (W * H * 4) as usize];
    for y in 0..H {
        for x in 0..W {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            let i = ((y * W + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    px
}

/// Render the checkerboard as a bare screen (no padding, corners, shadow,
/// cursor, zoom or watermark) at an export height (0 = native).
fn render(height: u32) -> Image {
    let rec = daisycap::synth::recording(W, H, 1000.0);
    let mut project = daisycap::project::new_project("resample", &rec);
    let s = &mut project.style;
    s.padding = 0.0;
    s.corner_radius = 0.0;
    s.shadow.intensity = 0.0;
    s.cursor.hide = true;
    s.watermark = false;
    project.timeline.zooms.clear();
    let mut scene = Scene::new(project.clone(), &rec, 60.0);
    let (_, h) = scene.sizes.render_size(height);
    let frame = scene.frame(0.0, h as f64);
    let mut r = Renderer::new(Gpu::headless().unwrap());
    r.prepare(&project.style, &rec, None);
    let px = checker();
    r.set_screen(&VideoFrame { width: W, height: H, stride: (W * 4) as usize, bgra: &px });
    r.render_to_image(&frame, &project.style)
}

#[test]
fn native_export_is_pixel_exact() {
    let img = render(ExportSettings::NATIVE);
    assert_eq!((img.width, img.height), (W, H));
    let src = checker();
    let off = img.rgba.chunks(4).zip(src.chunks(4)).filter(|(a, b)| a[0].abs_diff(b[0]) > 1).count();
    assert_eq!(off, 0, "{off} pixels differ from the recording");
}

#[test]
fn downscaled_export_does_not_alias() {
    // At 0.68x (a 4K export of a 5K-wide design) each output pixel covers
    // ~1.5 checker texels: a proper filter gives near-flat mid-grey, while
    // bilinear sampling, as before, beats into black/white moire.
    let img = render(272);
    let w = img.width as usize;
    let inner = img.rgba.chunks(4).enumerate().filter(|(i, _)| {
        let (x, y) = (i % w, i / w);
        x >= 4 && y >= 4 && x + 4 < w && y + 4 < img.height as usize
    });
    let (lo, hi) = inner.fold((255u8, 0u8), |(lo, hi), (_, p)| (lo.min(p[0]), hi.max(p[0])));
    assert!(hi - lo <= 48 && lo >= 96 && hi <= 160, "grey range {lo}..{hi}");
}
