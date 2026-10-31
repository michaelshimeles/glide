//! `glide snapshot` renders an editor frame to a PNG, in debug builds too.
//! egui panics in debug builds when a frame's texture changes are dropped.
use glide::app::snapshot::{Shot, render};

#[test]
fn snapshot_renders_an_editor_png_in_debug_builds() {
    let dir = std::env::temp_dir().join(format!("glide-snapshot-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Snap.glide");
    glide::synth::write_package(&path, 2.0, 640, 400).unwrap();
    let png = dir.join("snap.png");
    let shot = Shot {
        project: Some(&path),
        tab: None,
        select: None,
        export_sheet: false,
        fresh: false,
        dark: true,
        playhead_ms: 0.0,
        size: (1320.0, 840.0),
        window: false,
        recents: false,
        live: None,
    };
    render(&shot, &png).unwrap();
    assert!(std::fs::metadata(&png).unwrap().len() > 0);
    let _ = std::fs::remove_dir_all(&dir);
}
