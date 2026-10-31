//! Drives the editor timeline with pointer events (no window) to check that
//! a zoom segment can be trimmed from either edge as well as moved, and a
//! clip edge follows the pointer from where the drag began.

use egui::{Color32, Event, PointerButton, Pos2, Rect, Shape, vec2};
use glide::app::editor::{Editor, Selection};
use glide::app::home::{self, Devices, Settings, Thumbs};
use glide::project::model::{Mask, Zoom};
use glide::render::Gpu;

/// Zoom blocks are drawn with this fill (70 % unselected, 95 % selected).
const ZOOM_PURPLE: Color32 = Color32::from_rgb(0x8b, 0x5c, 0xf6);
/// Clip blocks use this fill, with the same two alphas.
const CLIP_AMBER: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);

struct Harness {
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    settings: Settings,
    devices: Devices,
    thumbs: Thumbs,
    t: f64,
    /// Where the zoom block was painted in the last frame.
    zoom_rect: Option<Rect>,
    /// Where the first clip block was painted in the last frame.
    clip_rect: Option<Rect>,
}

impl Harness {
    fn new(gpu: &Gpu) -> Self {
        let ctx = egui::Context::default();
        glide::app::theme::apply(&ctx);
        Harness {
            ctx,
            renderer: egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm, egui_wgpu::RendererOptions::default()),
            settings: Settings::default(),
            devices: Devices::default(),
            thumbs: Thumbs::default(),
            t: 0.0,
            zoom_rect: None,
            clip_rect: None,
        }
    }

    fn frame(&mut self, ed: &mut Editor, events: Vec<Event>) {
        self.t += 0.016;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1320.0, 840.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let projects = vec![];
        let (renderer, settings, devices, thumbs) = (&mut self.renderer, &mut self.settings, &mut self.devices, &mut self.thumbs);
        let mut out = self.ctx.run_ui(input, |ui| {
            let mut chrome = home::Chrome { settings, devices, projects: &projects, thumbs, action: home::Action::None };
            ed.ui(ui, renderer, &mut chrome);
        });
        out.textures_delta.clear();
        let block = |color: Color32| {
            let fills = [color.gamma_multiply(0.7), color.gamma_multiply(0.95)];
            out.shapes.iter().find_map(|s| match &s.shape {
                Shape::Rect(r) if fills.contains(&r.fill) && r.rect.height() > 20.0 => Some(r.rect),
                _ => None,
            })
        };
        self.zoom_rect = block(ZOOM_PURPLE);
        self.clip_rect = block(CLIP_AMBER);
    }

    fn pointer(&mut self, ed: &mut Editor, pos: Pos2, down: Option<bool>) {
        let mut events = vec![Event::PointerMoved(pos)];
        if let Some(pressed) = down {
            events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() });
        }
        self.frame(ed, events);
    }

    /// Press at `from`, move to `to` in a few steps, release.
    fn drag(&mut self, ed: &mut Editor, from: Pos2, to: Pos2) {
        self.pointer(ed, from, None);
        self.pointer(ed, from, Some(true));
        for i in 1..=6 {
            self.pointer(ed, from + (to - from) * (i as f32 / 6.0), None);
        }
        self.pointer(ed, to, Some(false));
        self.pointer(ed, to, None);
    }
}

fn editor_with_zoom(name: &str, gpu: Gpu) -> (Editor, std::path::PathBuf, u64) {
    let dir = std::env::temp_dir().join(format!("glide-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Zoom.glide");
    glide::synth::write_package(&path, 6.0, 1280, 800).unwrap();
    let mut ed = Editor::open(&path, gpu).unwrap();
    let mut id = 0;
    ed.edit(|p| {
        p.timeline.zooms.clear();
        id = p.timeline.new_id();
        p.timeline.zooms.push(Zoom { id, start: 2000.0, end: 4000.0, ..Default::default() });
    });
    (ed, dir, id)
}

fn zoom(ed: &Editor, id: u64) -> Zoom {
    *ed.project.timeline.zooms.iter().find(|z| z.id == id).expect("zoom exists")
}

#[test]
fn dragging_a_zoom_edge_inward_trims_it() {
    let gpu = Gpu::headless().unwrap();
    let (mut ed, dir, id) = editor_with_zoom("zoom-trim", gpu.clone());
    let mut h = Harness::new(&gpu);
    // Two frames so the timeline has laid out and drawn.
    h.frame(&mut ed, vec![]);
    h.frame(&mut ed, vec![]);
    let r = h.zoom_rect.expect("the zoom block is on the timeline");
    let y = r.center().y;

    // Left edge, dragged to the right: the start moves, the end stays.
    h.drag(&mut ed, Pos2::new(r.left() + 3.0, y), Pos2::new(r.left() + 3.0 + 40.0, y));
    let z = zoom(&ed, id);
    assert!(z.start > 2000.0 + 100.0, "start moved in: {z:?}");
    assert_eq!(z.end, 4000.0, "end must not move when trimming the left edge: {z:?}");

    // Right edge, dragged to the left: the end moves, the start stays.
    h.frame(&mut ed, vec![]);
    let r = h.zoom_rect.expect("zoom still drawn");
    let start = zoom(&ed, id).start;
    h.drag(&mut ed, Pos2::new(r.right() - 3.0, y), Pos2::new(r.right() - 3.0 - 40.0, y));
    let z = zoom(&ed, id);
    assert!(z.end < 4000.0 - 100.0, "end moved in: {z:?}");
    assert_eq!(z.start, start, "start must not move when trimming the right edge: {z:?}");

    // Dragging the middle still moves the whole segment.
    h.frame(&mut ed, vec![]);
    let r = h.zoom_rect.expect("zoom still drawn");
    let len = {
        let z = zoom(&ed, id);
        z.end - z.start
    };
    let before = zoom(&ed, id).start;
    h.drag(&mut ed, r.center(), r.center() + vec2(30.0, 0.0));
    let z = zoom(&ed, id);
    assert!(z.start > before, "body drag moves the zoom: {z:?}");
    assert!((z.end - z.start - len).abs() < 1e-6, "length unchanged by a move: {z:?}");
    assert_eq!(ed.selection, Selection::Zoom(id));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zoom_split_delete_and_undo_redo() {
    let gpu = Gpu::headless().unwrap();
    let (mut ed, dir, id) = editor_with_zoom("zoom-split", gpu);
    let duration = ed.rec.meta.duration_ms;
    let slices = ed.project.timeline.slices.clone();

    // Split the selected zoom at the playhead; the clips stay whole.
    ed.selection = Selection::Zoom(id);
    ed.seek(3000.0);
    ed.split_zoom();
    let zs = ed.project.timeline.zooms.clone();
    assert_eq!(zs.len(), 2, "{zs:?}");
    assert_eq!((zs[0].start, zs[0].end, zs[1].start, zs[1].end), (2000.0, 3000.0, 3000.0, 4000.0));
    assert_eq!(ed.project.timeline.slices, slices, "splitting a zoom doesn't cut the video");

    // Too close to an end: nothing changes and the user is told why.
    ed.seek(2100.0);
    ed.split_zoom();
    assert_eq!(ed.project.timeline.zooms.len(), 2);
    assert!(ed.status.as_deref().is_some_and(|s| s.contains("inside the zoom")), "{:?}", ed.status);
    assert_eq!(ed.project.timeline.slices, slices);

    // Split (S) cuts the clip even while a zoom is selected.
    ed.selection = Selection::Zoom(id);
    ed.seek(1000.0);
    ed.split();
    assert_eq!(ed.project.timeline.slices.len(), slices.len() + 1);
    assert_eq!(ed.project.timeline.zooms.len(), 2, "and leaves the zooms alone");

    // Delete the second half, then undo and redo it.
    ed.undo();
    ed.selection = Selection::Zoom(zs[1].id);
    ed.delete_selection();
    assert_eq!(ed.project.timeline.zooms.len(), 1);
    assert_eq!(ed.selection, Selection::None);
    ed.undo();
    assert_eq!(ed.project.timeline.zooms, zs);
    ed.redo();
    assert_eq!(ed.project.timeline.zooms.len(), 1);
    ed.undo();
    ed.undo();
    assert_eq!(ed.project.timeline.zooms.len(), 1, "undo of the split restores the single zoom");
    assert_eq!((ed.project.timeline.zooms[0].start, ed.project.timeline.zooms[0].end), (2000.0, 4000.0));

    // A zoom added at the very end still has room to exist.
    ed.seek(ed.duration());
    ed.add_zoom();
    let added = ed.project.timeline.zooms.last().unwrap();
    assert!(added.end <= duration && added.end - added.start >= Zoom::MIN_LEN_MS, "{added:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zoom_edges_clamp_to_the_recording_when_dragged_far() {
    let gpu = Gpu::headless().unwrap();
    let (mut ed, dir, id) = editor_with_zoom("zoom-clamp", gpu.clone());
    let duration = ed.rec.meta.duration_ms;
    let mut h = Harness::new(&gpu);
    h.frame(&mut ed, vec![]);
    h.frame(&mut ed, vec![]);
    let r = h.zoom_rect.unwrap();
    let y = r.center().y;

    // Drag the left edge far past the right edge: stops at the minimum length.
    h.drag(&mut ed, Pos2::new(r.left() + 3.0, y), Pos2::new(r.right() + 300.0, y));
    let z = zoom(&ed, id);
    assert_eq!(z.end, 4000.0);
    assert_eq!(z.start, 4000.0 - Zoom::MIN_LEN_MS, "{z:?}");

    // Drag the right edge far beyond the recording: stops at its end.
    h.frame(&mut ed, vec![]);
    let r = h.zoom_rect.unwrap();
    h.drag(&mut ed, Pos2::new(r.right() - 3.0, y), Pos2::new(r.right() + 4000.0, y));
    let z = zoom(&ed, id);
    assert_eq!(z.end, duration, "{z:?}");
    assert!(z.start < z.end);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn holding_a_clip_edge_keeps_the_trim_where_the_pointer_is() {
    let gpu = Gpu::headless().unwrap();
    let (mut ed, dir, _) = editor_with_zoom("clip-trim", gpu.clone());
    let mut h = Harness::new(&gpu);
    h.frame(&mut ed, vec![]);
    h.frame(&mut ed, vec![]);
    let r = h.clip_rect.expect("the clip block is on the timeline");
    let y = r.center().y;
    let slice = ed.project.timeline.slices[0];
    assert_eq!((slice.start, slice.speed), (0.0, 1.0), "{slice:?}");

    // Press on the left edge, move 100 px into the clip, then keep the pointer still.
    let from = Pos2::new(r.left(), y);
    let to = from + vec2(100.0, 0.0);
    h.pointer(&mut ed, from, None);
    h.pointer(&mut ed, from, Some(true));
    // A slow start: egui reports the drag once the pointer is 6 px away, still inside the 7 px edge zone.
    h.pointer(&mut ed, from + vec2(3.0, 0.0), None);
    h.pointer(&mut ed, from + vec2(6.5, 0.0), None);
    for i in 1..=6 {
        h.pointer(&mut ed, from + (to - from) * (i as f32 / 6.0), None);
    }
    for _ in 0..10 {
        h.pointer(&mut ed, to, None);
    }
    h.pointer(&mut ed, to, Some(false));
    h.pointer(&mut ed, to, None);

    let s = ed.project.timeline.slices[0];
    let expected = 100.0 / ed.px_per_sec as f64 * 1000.0;
    assert!((s.start - expected).abs() < 5.0, "start {} ms, expected about {expected} ms", s.start);
    assert_eq!(s.end, slice.end, "the end must not move: {s:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn a_mask_added_at_the_end_is_visible() {
    let gpu = Gpu::headless().unwrap();
    let (mut ed, dir, _) = editor_with_zoom("mask-end", gpu);
    let duration = ed.rec.meta.duration_ms;
    ed.seek(ed.duration());
    ed.add_mask();
    let m = ed.project.timeline.masks.last().unwrap();
    assert!(m.end <= duration && m.end - m.start >= Mask::MIN_LEN_MS, "{m:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
