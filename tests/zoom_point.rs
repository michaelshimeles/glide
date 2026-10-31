//! Choosing a manual zoom point by clicking the preview: the point has to be
//! the one under the pointer, whether the preview shows the whole screen or a
//! zoomed-in part of it.

use glide::app::editor::Editor;
use glide::engine::geom::v2;
use glide::project::model::Zoom;
use glide::render::Gpu;

const HEIGHT: f64 = 800.0;

fn editor(name: &str) -> (Editor, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("glide-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Point.glide");
    glide::synth::write_package(&path, 6.0, 1280, 800).unwrap();
    let mut ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();
    ed.edit(|p| {
        p.timeline.zooms.clear();
        let id = p.timeline.new_id();
        p.timeline.zooms.push(Zoom { id, start: 1000.0, end: 5000.0, scale: 3.0, ..Default::default() });
    });
    (ed, dir)
}

/// Draw the frame at `ms` the way the preview does and return how far the
/// window is zoomed.
fn show(ed: &mut Editor, ms: f64) -> f64 {
    let frame = ed.preview.scene.frame(ms, HEIGHT);
    let zoom = frame.window.rect.w / (ed.preview.scene.sizes.window.x * HEIGHT / ed.preview.scene.sizes.output.y);
    ed.preview.last_frame = Some(frame);
    zoom
}

#[test]
fn a_click_picks_the_screen_point_under_the_pointer_zoomed_or_not() {
    let (mut ed, dir) = editor("zoom-point");
    for (ms, zoomed) in [(0.0, false), (3000.0, true)] {
        let zoom = show(&mut ed, ms);
        assert_eq!(zoom > 1.5, zoomed, "zoom at {ms} ms is {zoom}");
        let frame = ed.preview.last_frame.clone().unwrap();
        for click in [v2(0.5, 0.5), v2(0.4, 0.45), v2(0.6, 0.55), v2(0.45, 0.6)] {
            let picked = ed.zoom_target_at(egui::vec2(click.x as f32, click.y as f32)).unwrap();
            // The picked point must be drawn exactly where the click was.
            let screen = ed.preview.scene.sizes.crop.at(picked);
            let out = ed.preview.scene.screen_to_output(screen, frame.window.rect);
            let (x, y) = (out.x / frame.size.x, out.y / frame.size.y);
            assert!((x - click.x).abs() < 2e-3 && (y - click.y).abs() < 2e-3, "zoom {zoom:.1}: clicked {click:?}, point is drawn at ({x:.3}, {y:.3})");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_chosen_point_ends_up_in_the_middle_of_the_zoomed_view() {
    let (mut ed, dir) = editor("zoom-centre");
    // Click a spot on the unzoomed preview, make it the zoom's target, and
    // look at the zoomed frame: that spot is now in the middle of the view.
    show(&mut ed, 0.0);
    let target = ed.zoom_target_at(egui::vec2(0.42, 0.47)).unwrap();
    ed.edit(|p| p.timeline.zooms[0].mode = glide::project::model::ZoomMode::Manual { target });
    let zoom = show(&mut ed, 3000.0);
    assert!(zoom > 1.5);
    let frame = ed.preview.last_frame.clone().unwrap();
    let out = ed.preview.scene.screen_to_output(ed.preview.scene.sizes.crop.at(target), frame.window.rect);
    let (x, y) = (out.x / frame.size.x, out.y / frame.size.y);
    assert!((x - 0.5).abs() < 0.01 && (y - 0.5).abs() < 0.01, "drawn at ({x:.3}, {y:.3})");
    let _ = std::fs::remove_dir_all(&dir);
}
