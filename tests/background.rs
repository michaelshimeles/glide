//! The Background settings (wallpaper, gradient, colour, image, blur) and
//! the padding, roundness and shadow that frame them change the editor's
//! canvas as soon as they're edited, and the export draws the same thing.
//! Each check renders the preview, reads its pixels back and compares.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use daisycap::app::editor::Editor;
use daisycap::export::{self, Job, Source};
use daisycap::project::model::{Aspect, Background, Color, ExportSettings, Quality, Style};
use daisycap::render::bgcheck::{self, Region};
use daisycap::render::{Gpu, Image};

const SIZE: (u32, u32) = (640, 400);

/// A synthetic recording opened in the editor, with its auto zooms removed
/// so the background stays in view.
fn editor(name: &str) -> (PathBuf, Editor) {
    let dir = std::env::temp_dir().join(format!("daisycap-bg-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Background.daisycap");
    daisycap::synth::write_package(&path, 1.0, 1280, 800).unwrap();
    let mut ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    let mut d = ed.project.clone();
    d.timeline.zooms.clear();
    ed.commit(d);
    (dir, ed)
}

fn draw(ed: &mut Editor, size: (u32, u32)) -> Image {
    ed.preview.draw(0.0, size);
    let (width, height, rgba) = ed.preview.read_pixels().expect("preview pixels");
    Image { width, height, rgba }
}

/// Set the style the way the inspector does (on a draft, then commit) and
/// return the canvas the preview shows afterwards.
fn show(ed: &mut Editor, style: &Style) -> Image {
    let mut d = ed.project.clone();
    d.style = style.clone();
    ed.commit(d);
    draw(ed, SIZE)
}

#[test]
fn every_background_setting_changes_the_canvas() {
    let (dir, mut ed) = editor("live");
    assert!(ed.project.style.padding > 0.0, "the background shows by default");
    let image = dir.join("checker.png");
    bgcheck::checkerboard(&image).unwrap();
    let style = ed.project.style.clone();
    let outcomes = bgcheck::run(&style, &image.to_string_lossy(), |s| Ok(show(&mut ed, s))).unwrap();
    let failed: Vec<_> = outcomes.iter().filter(|o| !o.ok()).collect();
    assert!(failed.is_empty(), "no visible change for: {failed:#?}");
    assert_eq!(outcomes.len(), bgcheck::steps("").len());

    // A colour is drawn as that colour.
    let mut s = ed.project.style.clone();
    s.background = Background::Color { color: Color::rgb(52, 120, 246) };
    let c = show(&mut ed, &s);
    assert_eq!(&c.rgba[..3], &[52, 120, 246], "top-left pixel");

    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn background_edits_at_zero_padding_bring_it_into_view() {
    let (dir, mut ed) = editor("reveal");
    // Edge to edge, nothing but the recording shows.
    let mut s = ed.project.style.clone();
    s.padding = 0.0;
    s.corner_radius = 0.0;
    let flat = show(&mut ed, &s);
    // A background change alone is invisible at 0 %…
    s.background = Background::Color { color: Color::rgb(220, 40, 40) };
    let hidden = show(&mut ed, &s);
    assert!(bgcheck::changed(&hidden, &flat, Region::Border, 30) < 0.01, "nothing to see at 0 % padding");
    // …so the inspector adds padding along with it.
    let before = s.clone();
    s.background = Background::Color { color: Color::rgb(40, 200, 90) };
    assert!(s.reveal_background(&before));
    assert_eq!(s.padding, Style::REVEAL_PADDING);
    let shown = show(&mut ed, &s);
    let c = bgcheck::changed(&shown, &hidden, Region::Border, 30);
    assert!(c > 0.9, "revealed background: {:.0}% changed", c * 100.0);
    // Padding edits themselves are left alone.
    let before = s.clone();
    s.padding = 0.0;
    assert!(!s.reveal_background(&before));

    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn watermark_draws_in_the_bottom_left_corner() {
    let (dir, mut ed) = editor("watermark");
    assert!(ed.project.style.watermark, "the mark is on by default");
    let on = draw(&mut ed, SIZE);
    let mut s = ed.project.style.clone();
    s.watermark = false;
    let off = show(&mut ed, &s);
    let (w, h) = SIZE;
    let (mut inside, mut outside) = (0usize, 0usize);
    for (i, (a, b)) in on.rgba.chunks_exact(4).zip(off.rgba.chunks_exact(4)).enumerate() {
        if (0..3).any(|c| a[c].abs_diff(b[c]) > 2) {
            let (x, y) = (i as u32 % w, i as u32 / w);
            if x < w / 3 && y > h / 2 {
                inside += 1;
            } else {
                outside += 1;
            }
        }
    }
    assert!(inside > 100, "the mark draws in the corner ({inside} px)");
    assert_eq!(outside, 0, "{outside} px changed away from the corner");
    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Export the first frame of the project and decode it back.
fn export_frame(ed: &Editor, dir: &Path, name: &str) -> Image {    let settings = ExportSettings { height: 400, fps: 10, quality: Quality::Studio, ..Default::default() };
    let output = dir.join(format!("{name}.mp4"));
    let job = Job { project: ed.project.clone(), recording: ed.rec.clone(), source: Source::Package(ed.package.root.clone()), settings, output: output.clone() };
    export::run(&job, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let mut src = daisycap::media::FrameSource::open(&output).unwrap();
    let frame = src.frame_at(0.0).unwrap();
    frame.with_bgra(|v| {
        let mut rgba = Vec::with_capacity((v.width * v.height * 4) as usize);
        for row in v.bgra.chunks(v.stride).take(v.height as usize) {
            for p in row[..(v.width * 4) as usize].chunks_exact(4) {
                rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        Image { width: v.width, height: v.height, rgba }
    })
}

fn mean_abs_diff(a: &Image, b: &Image) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let sum: u64 = a.rgba.chunks_exact(4).zip(b.rgba.chunks_exact(4)).map(|(a, b)| (0..3).map(|c| a[c].abs_diff(b[c]) as u64).sum::<u64>()).sum();
    sum as f64 / (a.width * a.height * 3) as f64
}

#[test]
fn export_draws_the_background_the_preview_shows() {
    let (dir, mut ed) = editor("export");
    let styles: [(&str, fn(&mut Style)); 3] = [
        ("wallpaper", |s| {
            s.background = Background::Wallpaper { id: "ocean".into() };
            s.background_blur = 20.0;
        }),
        ("gradient", |s| {
            s.background = Background::Gradient { angle: 90.0, stops: vec![(Color::rgb(255, 110, 196), 0.0), (Color::rgb(120, 115, 245), 1.0)] };
            s.background_blur = 0.0;
            s.padding = 20.0;
            s.corner_radius = 30.0;
        }),
        ("colour", |s| {
            s.background = Background::Color { color: Color::rgb(30, 140, 90) };
            s.shadow.intensity = 1.0;
        }),
    ];
    let mut exported: Vec<Image> = vec![];
    for (name, f) in &styles {
        let mut s = ed.project.style.clone();
        f(&mut s);
        // A fixed frame, so the exports line up pixel for pixel.
        s.aspect = Aspect::Ratio { w: 16, h: 9 };
        show(&mut ed, &s);
        let video = export_frame(&ed, &dir, name);
        let preview = draw(&mut ed, (video.width, video.height));
        let diff = mean_abs_diff(&preview, &video);
        assert!(diff < 6.0, "{name}: export differs from the preview by {diff:.1} per channel");
        for (other, prev) in exported.iter().enumerate() {
            let c = bgcheck::changed(&video, prev, Region::Border, 30);
            assert!(c > 0.5, "{name} vs {}: only {:.0}% of the exported background differs", styles[other].0, c * 100.0);
        }
        exported.push(video);
    }

    drop(ed);
    let _ = std::fs::remove_dir_all(&dir);
}
