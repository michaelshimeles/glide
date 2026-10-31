//! Render the real UI headlessly to a PNG (no window, no display access):
//! used to check layouts in CI and on machines where the app can't be shown.

use std::path::Path;

use anyhow::Result;

use super::editor::{Editor, Selection, Tab};
use super::home::{self, Devices, Settings, Thumbs};
use super::theme;
use crate::render::Gpu;
use crate::render::gpu::read_texture;

pub struct Shot<'a> {
    /// Editor for this project, or the home view when None.
    pub project: Option<&'a Path>,
    pub tab: Option<Tab>,
    /// Select the first "zoom", "mask" or "slice".
    pub select: Option<String>,
    pub export_sheet: bool,
    /// Show the just-recorded banner.
    pub fresh: bool,
    pub dark: bool,
    pub playhead_ms: f64,
    pub size: (f32, f32),
    /// Home: show the Window source with the first window chosen.
    pub window: bool,
    /// Draw the recent-recordings popover under the title.
    pub recents: bool,
    /// Start view: show a live preview, of the main display ("display") or
    /// of Glide's synthetic demo screen ("synthetic").
    pub live: Option<String>,
}

pub fn render(shot: &Shot, out_file: &Path) -> Result<()> {
    let gpu = Gpu::headless()?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, format, egui_wgpu::RendererOptions::default());
    let ctx = egui::Context::default();
    ctx.set_theme(if shot.dark { egui::Theme::Dark } else { egui::Theme::Light });
    theme::apply(&ctx);
    let ppp = 2.0f32;
    let (w, h) = shot.size;

    let mut editor = match shot.project {
        Some(p) => Some(Editor::open(p, gpu.clone())?),
        None => None,
    };
    if let Some(ed) = &mut editor {
        if let Some(t) = shot.tab {
            ed.tab = t;
        }
        let tl = &ed.project.timeline;
        ed.selection = match shot.select.as_deref() {
            Some("zoom") => tl.zooms.first().map_or(Selection::None, |z| Selection::Zoom(z.id)),
            Some("mask") => tl.masks.first().map_or(Selection::None, |m| Selection::Mask(m.id)),
            Some("slice") => tl.slices.first().map_or(Selection::None, |s| Selection::Slice(s.id)),
            _ => Selection::None,
        };
        ed.export.open = shot.export_sheet;
        ed.fresh = shot.fresh;
        ed.seek(shot.playhead_ms);
    }
    let mut settings = Settings::default();
    if let Some(dir) = std::env::var_os("GLIDE_OUTPUT_DIR") {
        settings.projects_dir = Some(dir.into());
    }
    let mut devices = Devices::load();
    if shot.window {
        settings.source = home::SourceKind::Window;
        // Without Screen Recording access no windows are listed: a stand-in
        // with a long title (long titles once pushed the sidebar off screen).
        if devices.windows.is_empty() {
            devices.windows.push(home::WindowEntry {
                id: 1,
                label: "Google Chrome — Kampe launch strategy - Claude — a very long window title that keeps going".into(),
                size: (1280.0, 800.0),
                display: (2560.0, 1440.0),
            });
        }
        settings.window = devices.windows.first().map(|w| w.id);
    }
    let projects = home::scan_projects(&settings.dir());
    let mut thumbs = Thumbs::default();
    // A real frame from the live preview stream (main display).
    let live_tex = match shot.live.as_deref().filter(|_| editor.is_none()) {
        Some("synthetic") => {
            let (w, h) = (1440u32, 900u32);
            let mut px = crate::synth::screen_bgra(w, h, 0.0);
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
            Some(ctx.load_texture("live-preview", egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px), egui::TextureOptions::LINEAR))
        }
        Some(_) => {
            let p = crate::capture::preview::LivePreview::start(&crate::capture::preview::Source::Display { id: 0 }, 1280, 10, false, || {})?;
            let t0 = std::time::Instant::now();
            let frame = loop {
                if let Some(f) = p.take_frame() {
                    break f;
                }
                anyhow::ensure!(t0.elapsed().as_secs() < 5, "no live preview frame");
                std::thread::sleep(std::time::Duration::from_millis(50));
            };
            let img = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.rgba);
            Some(ctx.load_texture("live-preview", img, egui::TextureOptions::LINEAR))
        }
        None => None,
    };
    // With a synthetic preview, show a camera bubble and meters too.
    let synthetic = shot.live.as_deref() == Some("synthetic") && editor.is_none();
    let cam_tex = synthetic.then(|| {
        let (w, h) = (640usize, 480usize);
        let mut px = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 - 320.0, y as f32 - 200.0);
                let face = dx * dx / 1.0 + dy * dy / 1.4 < 110.0 * 110.0;
                let c = if face { [224, 182, 150] } else { [60 + (y / 8) as u8, 80, 110] };
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        ctx.load_texture("camera-preview", egui::ColorImage::from_rgba_unmultiplied([w, h], &px), egui::TextureOptions::LINEAR)
    });
    if synthetic {
        settings.camera = true;
        settings.mic = true;
    }
    let live = home::LiveView {
        screen: live_tex.as_ref().map(|t| (t.id(), t.size_vec2())),
        camera: cam_tex.as_ref().map(|t| (t.id(), t.size_vec2())),
        system_level: synthetic.then_some(0.3),
        mic_level: synthetic.then_some(0.6),
        note: None,
        covered: None,
    };

    let mut last = None;
    for i in 0..8 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))),
            time: Some(i as f64 * 0.1),
            system_theme: Some(if shot.dark { egui::Theme::Dark } else { egui::Theme::Light }),
            ..Default::default()
        };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        // Thumbnails load on background threads.
        let wait = match (i, editor.is_some()) {
            (4, _) if shot.recents => 5000,
            (4, true) => 600,
            (4, false) => 300,
            _ => 20,
        };
        std::thread::sleep(std::time::Duration::from_millis(wait));
        // Let background mask checks finish so their warnings show.
        if i == 4
            && let Some(ed) = &mut editor
        {
            let t0 = std::time::Instant::now();
            ed.tick();
            while ed.mask_watch.busy() && t0.elapsed().as_secs() < 30 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                ed.tick();
            }
        }
        let mut out = ctx.run_ui(input, |ui| {
            match &mut editor {
                Some(ed) => {
                    let mut chrome = home::Chrome { settings: &mut settings, devices: &mut devices, projects: &projects, thumbs: &mut thumbs, action: home::Action::None };
                    ed.ui(ui, &mut renderer, &mut chrome);
                }
                None => {
                    home::start_view(ui, &mut settings, &mut devices, &projects, &mut thumbs, None, &live);
                }
            }
            // The popover itself (a menu only opens from a click).
            if shot.recents {
                let current = editor.as_ref().map(|e| e.package.root.clone());
                egui::Area::new(egui::Id::new("recents-preview")).fixed_pos(egui::pos2(130.0, 50.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        home::recents(ui, &projects, &mut thumbs, current.as_deref(), &settings.dir());
                    });
                });
            }
        });

        for (id, deltas) in &out.textures_delta.set {
            for d in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, d);
            }
        }
        // Uploaded above. egui debug-asserts when a frame drops its texture deltas.
        out.textures_delta.clear();
        last = Some(out);
    }
    let out = last.unwrap();
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    let (pw, ph) = ((w * ppp) as u32, (h * ppp) as u32);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("snapshot"),
        size: wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [pw, ph], pixels_per_point: out.pixels_per_point };
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    let extra = renderer.update_buffers(&gpu.device, &gpu.queue, &mut enc, &prims, &sd);
    gpu.queue.submit(extra);
    {
        let bg = theme::palette(shot.dark).window;
        let [r, g, b, _] = bg.to_normalized_gamma_f32();
        let mut pass = enc
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("snapshot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        renderer.render(&mut pass, &prims, &sd);
    }
    let rgba = read_texture(&gpu, &target, pw, ph, enc);
    image::save_buffer(out_file, &rgba, pw, ph, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

/// Time the editor preview path (decode + zero-copy upload + render) over
/// consecutive frames, as playback does. Returns frames per second.
pub fn bench_preview(project: &Path, size: (u32, u32), frames: u32) -> Result<f64> {
    let gpu = Gpu::headless()?;
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm, egui_wgpu::RendererOptions::default());
    let mut ed = Editor::open(project, gpu.clone())?;
    let preview = &mut ed.preview;
    let step = 1000.0 / 60.0;
    // Warm up (first decode, pipeline creation).
    preview.render(&mut renderer, 0.0, size);
    let t = std::time::Instant::now();
    for i in 1..=frames {
        preview.render(&mut renderer, i as f64 * step, size);
    }
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    Ok(frames as f64 / t.elapsed().as_secs_f64())
}
