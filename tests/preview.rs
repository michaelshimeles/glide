//! The preview draws its scene (cursor, zoom, captions, masks) at the time
//! of the video frame on display. After a jump that frame is the old one
//! until the new one decodes, and the scene waits with it.

use std::time::Duration;

use glide::app::editor::Editor;
use glide::project::model::Slice;
use glide::render::Gpu;

const SIZE: (u32, u32) = (640, 400);

fn drawn_at(ed: &Editor) -> f64 {
    ed.preview.last_frame.as_ref().expect("a drawn frame").playback_ms
}

fn width(ed: &Editor) -> u32 {
    ed.preview.read_pixels().expect("a drawn picture").0
}

#[test]
fn the_scene_waits_for_the_video_frame() {
    let dir = std::env::temp_dir().join(format!("glide-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Preview.glide");
    // A keyframe every 2 s.
    glide::synth::write_package(&path, 4.0, 1280, 800).unwrap();
    let mut ed = Editor::open(&path, Gpu::headless().unwrap()).unwrap();

    let mut shown = 100.0;
    ed.preview.draw(shown, None, SIZE);
    assert!(ed.preview.settle(Duration::from_secs(10)), "no video frame");
    // Jumps to the end of a GOP: each new frame decodes from its keyframe,
    // which usually takes longer than a draw.
    for to in [1900.0, 3900.0, 1800.0, 3800.0] {
        ed.preview.draw(to, None, SIZE);
        // Still decoding now, so it was when the draw read the frame. A
        // decode that already finished leaves nothing to check here.
        if !ed.preview.settle(Duration::ZERO) {
            assert_eq!(drawn_at(&ed), shown, "the scene at the time of the frame on display");
        }
        assert!(ed.preview.settle(Duration::from_secs(10)), "no video frame for {to}");
        ed.preview.draw(to, None, SIZE);
        assert_eq!(drawn_at(&ed), to);
        shown = to;
    }

    // A jump, then an edit that cuts the moment still on display and a new
    // preview size: the preview keeps its last picture, at its old size,
    // until the new frame arrives.
    ed.preview.draw(1900.0, None, SIZE);
    let mut cut = ed.project.clone();
    cut.timeline.slices = vec![Slice { start: 0.0, end: 3000.0, speed: 1.0, ..Default::default() }];
    ed.commit(cut);
    let bigger = (800, 500);
    ed.preview.draw(1900.0, None, bigger);
    if !ed.preview.settle(Duration::ZERO) {
        assert_eq!(drawn_at(&ed), shown, "the last picture kept");
        assert_eq!(width(&ed), SIZE.0, "the last picture kept at its size");
    }
    assert!(ed.preview.settle(Duration::from_secs(10)), "no video frame after the edit");
    ed.preview.draw(1900.0, None, bigger);
    assert_eq!((drawn_at(&ed), width(&ed)), (1900.0, bigger.0));

    let _ = std::fs::remove_dir_all(&dir);
}
