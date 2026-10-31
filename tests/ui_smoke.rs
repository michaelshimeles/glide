//! Headless UI smoke test: drive the start view (recording setup) and every
//! editor panel through egui frames (no window), including selections,
//! playback, the export sheet and the title-bar controls, to catch panics
//! and layout loops without needing a display.

use std::time::Duration;

use glide::app::editor::{Editor, Selection, Tab};
use glide::app::home::{self, Devices, Settings, Thumbs};
use glide::render::Gpu;

fn frame(ctx: &egui::Context, t: f64, f: impl FnMut(&mut egui::Ui)) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1320.0, 840.0))),
        time: Some(t),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, f);
    // Nothing paints this output, and egui panics in debug builds when texture changes are dropped.
    out.textures_delta.clear();
}

#[test]
fn editor_and_home_render_every_panel() {
    let dir = std::env::temp_dir().join(format!("glide-ui-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Smoke.glide");
    glide::synth::write_package(&path, 3.0, 1280, 800).unwrap();

    let gpu = Gpu::headless().unwrap();
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm, egui_wgpu::RendererOptions::default());
    let ctx = egui::Context::default();
    glide::app::theme::apply(&ctx);

    let mut ed = Editor::open(&path, gpu).unwrap();
    let mut t = 0.0;
    let mut settings = Settings { projects_dir: Some(dir.clone()), ..Settings::default() };
    let mut devices = Devices::default();
    let mut thumbs = Thumbs::default();
    let projects = home::scan_projects(&dir);
    assert_eq!(projects.len(), 1);
    // The editor's frame: its title bar draws Back, the title (recents) and
    // the record controls from the app's settings.
    let mut actions = vec![];
    let mut editor_frame = |ed: &mut Editor, t: f64| {
        frame(&ctx, t, |ui| {
            let mut chrome = home::Chrome { settings: &mut settings, devices: &mut devices, projects: &projects, thumbs: &mut thumbs, action: home::Action::None };
            ed.ui(ui, &mut renderer, &mut chrome);
            if !matches!(chrome.action, home::Action::None) {
                actions.push(format!("{:?}", std::mem::discriminant(&chrome.action)));
            }
        })
    };
    for (tab, _) in Tab::ALL {
        ed.tab = tab;
        for _ in 0..2 {
            t += 0.016;
            editor_frame(&mut ed, t);
        }
    }
    // Selections show their own inspector panels.
    let zoom = ed.project.timeline.zooms.first().map(|z| z.id);
    let slice = ed.project.timeline.slices[0].id;
    for sel in [Selection::Slice(slice), zoom.map_or(Selection::None, Selection::Zoom)] {
        ed.selection = sel;
        t += 0.016;
        editor_frame(&mut ed, t);
    }
    // Edits: split, add a zoom, undo/redo.
    ed.seek(1500.0);
    ed.split();
    assert_eq!(ed.project.timeline.slices.len(), 2);
    let zooms = ed.project.timeline.zooms.len();
    ed.seek(400.0);
    ed.add_zoom();
    assert!(ed.project.timeline.zooms.len() >= zooms);
    ed.undo();
    ed.undo();
    assert_eq!(ed.project.timeline.slices.len(), 1);
    ed.redo();
    assert_eq!(ed.project.timeline.slices.len(), 2);

    // Zooms: split the selected one at the playhead, trim an edge, delete, undo.
    let id = ed.project.timeline.zooms.last().unwrap().id;
    let (z0, z1) = {
        let z = ed.project.timeline.zooms.iter().find(|z| z.id == id).unwrap();
        (z.start, z.end)
    };
    ed.selection = Selection::Zoom(id);
    ed.seek(ed.preview.scene.map.to_playback_nearest((z0 + z1) / 2.0));
    let count = ed.project.timeline.zooms.len();
    let slices = ed.project.timeline.slices.len();
    ed.split_zoom();
    assert_eq!(ed.project.timeline.zooms.len(), count + 1, "split the selected zoom");
    assert_eq!(ed.project.timeline.slices.len(), slices, "and left the clips alone");
    ed.selection = Selection::Zoom(id);
    ed.delete_selection();
    assert_eq!(ed.project.timeline.zooms.len(), count);
    ed.undo();
    ed.undo();
    assert_eq!(ed.project.timeline.zooms.len(), count);

    // Playback advances the playhead.
    ed.seek(0.0);
    ed.toggle_play();
    std::thread::sleep(Duration::from_millis(120));
    assert!(ed.tick());
    assert!(ed.playhead > 50.0, "playhead {}", ed.playhead);
    ed.toggle_play();

    // The export sheet renders.
    ed.export.open = true;
    t += 0.016;
    editor_frame(&mut ed, t);
    ed.export.open = false;

    // A blur over the first sidebar item: the synthetic UI moves the
    // selection away at 1.5 s, so "data may be visible" is flagged there and
    // cutting it removes that part of the recording.
    let (ws, hs) = (1280.0 * 1080.0 / 800.0, 1080.0);
    ed.edit(|p| {
        let id = p.timeline.new_id();
        let rect = glide::engine::geom::Rect::new(15.0 / ws, 75.0 / hs, 210.0 / ws, 32.0 / hs);
        p.timeline.masks.push(glide::project::model::Mask { id, start: 200.0, end: 2900.0, rect, ..Default::default() });
    });
    let t0 = std::time::Instant::now();
    ed.tick();
    while (ed.mask_watch.busy() || ed.mask_watch.found.is_empty()) && t0.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(50));
        ed.tick();
    }
    let found = ed.mask_watch.found.clone();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!((found[0].start - 1500.0).abs() <= 100.0 && found[0].end >= 2800.0, "{found:?}");
    assert!(glide::app::timeline::extra_height(&ed) > 0.0);
    t += 0.016;
    editor_frame(&mut ed, t);
    ed.cut_exposure(found[0]);
    assert!(ed.project.timeline.slices.iter().all(|s| s.end <= found[0].start || s.start >= found[0].end), "{:?}", ed.project.timeline.slices);
    assert_eq!(glide::app::timeline::extra_height(&ed), 0.0);
    t += 0.016;
    editor_frame(&mut ed, t);
    ed.save();
    drop(editor_frame);
    assert!(actions.is_empty(), "no title-bar button was pressed: {actions:?}");
    let root = ed.package.root.clone();

    // Start view (no recording open) with this folder as the library, every
    // source, the recents popover and the permission note.
    // With and without live preview textures (source and camera) and meters.
    let tex = ctx.load_texture("live", egui::ColorImage::new([64, 40], vec![egui::Color32::DARK_GRAY; 64 * 40]), egui::TextureOptions::LINEAR);
    for (source, live) in [
        (home::SourceKind::Display, Some((tex.id(), tex.size_vec2()))),
        (home::SourceKind::Window, None),
        (home::SourceKind::Area, Some((tex.id(), tex.size_vec2()))),
        (home::SourceKind::Device, None),
    ] {
        settings.source = source;
        settings.camera = live.is_some();
        let live = home::LiveView {
            screen: live,
            camera: live,
            system_level: live.map(|_| 0.4),
            mic_level: None,
            note: live.is_none().then(|| "Choose a window to see it here.".into()),
            covered: None,
        };
        for _ in 0..2 {
            t += 0.016;
            frame(&ctx, t, |ui| {
                let a = home::start_view(ui, &mut settings, &mut devices, &projects, &mut thumbs, Some("permission note"), &live);
                assert!(matches!(a, home::Action::None));
            });
        }
    }
    t += 0.016;
    frame(&ctx, t, |ui| {
        let a = home::recents(ui, &projects, &mut thumbs, Some(&root), &dir);
        assert!(matches!(a, home::Action::None));
    });
    let _ = std::fs::remove_dir_all(&dir);
}
