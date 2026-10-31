//! Inspector: style sections, and properties of the selected timeline item.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke, Ui, vec2};

use super::editor::{Deferred, Editor, Selection, Tab};
use super::theme::{self, pal};
use super::widgets::{self, caption, row, section, segmented, slider, toggle_row};
use crate::engine::geom::v2;
use crate::engine::spring::SpringConfig;
use crate::project::model::*;
use crate::render::art::WALLPAPERS;

const GRADIENTS: [(&str, [u8; 3], [u8; 3], f64); 6] = [
    ("Dusk", [255, 120, 80], [130, 70, 220], 135.0),
    ("Ocean", [40, 160, 255], [20, 220, 190], 120.0),
    ("Candy", [255, 110, 196], [120, 115, 245], 90.0),
    ("Forest", [30, 120, 90], [150, 210, 120], 160.0),
    ("Night", [20, 24, 40], [70, 60, 120], 180.0),
    ("Paper", [245, 245, 240], [220, 220, 215], 180.0),
];

pub fn ui(ui: &mut Ui, ed: &mut Editor) {
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut draft = ed.project.clone();
        match ed.selection {
            Selection::Zoom(id) => zoom_panel(ui, ed, &mut draft, id),
            Selection::Slice(id) => slice_panel(ui, &mut draft, id),
            Selection::Layout(id) => layout_panel(ui, &mut draft, id),
            Selection::Mask(id) => mask_panel(ui, ed, &mut draft, id),
            Selection::None => {
                tabs(ui, &mut ed.tab);
                match ed.tab {
                    Tab::Background => background(ui, ed, &mut draft),
                    Tab::Cursor => cursor(ui, &mut draft, ed.rec.mouse.is_empty()),
                    Tab::Camera => camera(ui, &mut draft, ed.rec.meta.camera.is_some()),
                    Tab::Captions => captions(ui, ed, &mut draft),
                    Tab::Audio => audio(ui, ed, &mut draft),
                    Tab::Shortcuts => shortcuts(ui, &mut draft, ed.rec.keys.len()),
                    Tab::Animation => animation(ui, ed, &mut draft),
                }
            }
        }
        if ui.input(|i| i.pointer.any_released()) {
            // End of a drag: the next change starts a new undo step.
        }
        ed.commit(draft);
        // Actions that edit the project themselves run after the draft lands.
        match ed.deferred.take() {
            Some(Deferred::StartVoice) => ed.start_voiceover(),
            Some(Deferred::StopVoice) => ed.stop_voiceover(),
            None => {}
        }
    });
}

fn tabs(ui: &mut Ui, tab: &mut Tab) {
    let p = pal(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (t, label) in Tab::ALL {
            let sel = *tab == t;
            let b = egui::Button::new(RichText::new(label).size(12.0).color(if sel { Color32::WHITE } else { p.text }))
                .fill(if sel { theme::accent() } else { p.control })
                .stroke(if sel { Stroke::NONE } else { Stroke::new(0.5, p.control_border) })
                .corner_radius(CornerRadius::same(12));
            if ui.add(b).clicked() {
                *tab = t;
            }
        }
    });
    ui.add_space(4.0);
    ui.separator();
}

/// Ask for a PNG or JPEG, check it decodes, and copy it into the package.
fn choose_image(ed: &mut Editor) -> Option<String> {
    let f = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg"]).pick_file()?;
    match crate::render::Image::from_png(&f).and_then(|_| ed.package.import_asset(&f, "background")) {
        Ok(rel) => Some(rel),
        Err(e) => {
            ed.status = Some(format!("Couldn't use that image: {e:#}"));
            None
        }
    }
}

fn swatch(ui: &mut Ui, size: egui::Vec2, selected: bool, paint: impl FnOnce(&egui::Painter, egui::Rect)) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    paint(ui.painter(), rect);
    let stroke = if selected { Stroke::new(2.5, theme::accent()) } else { Stroke::new(1.0, pal(ui).control_border) };
    ui.painter().rect_stroke(rect, CornerRadius::same(6), stroke, egui::StrokeKind::Outside);
    resp.clicked()
}

fn background(ui: &mut Ui, ed: &mut Editor, d: &mut Project) {
    let before = d.style.clone();
    let s = &mut d.style;
    section(ui, "Background", |ui| {
        if s.padding <= 0.0 && s.device == DeviceFrame::None {
            caption(ui, "Hidden at 0 % padding. Pick a background (or raise Padding) to show it.");
        }
        #[derive(Clone, Copy, PartialEq)]
        enum K {
            Wall,
            Grad,
            Col,
            Img,
        }
        let mut k = match s.background {
            Background::Wallpaper { .. } => K::Wall,
            Background::Gradient { .. } => K::Grad,
            Background::Color { .. } => K::Col,
            Background::Image { .. } => K::Img,
        };
        if segmented(ui, &mut k, &[(K::Wall, "Wallpaper"), (K::Grad, "Gradient"), (K::Col, "Color"), (K::Img, "Image")]) {
            s.background = match k {
                K::Wall => Background::Wallpaper { id: "aurora".into() },
                K::Grad => Background::Gradient { angle: 135.0, stops: vec![(Color::rgb(255, 120, 80), 0.0), (Color::rgb(130, 70, 220), 1.0)] },
                K::Col => Background::Color { color: Color::rgb(52, 120, 246) },
                // Ask for the image straight away; cancelling keeps the old background.
                K::Img => choose_image(ed).map_or_else(|| s.background.clone(), |path| Background::Image { path }),
            };
        }
        ui.add_space(6.0);
        match &mut s.background {
            Background::Wallpaper { id } => {
                let thumbs = ed.thumbs.get_or_insert_with(|| {
                    WALLPAPERS
                        .iter()
                        .map(|w| {
                            let img = crate::render::art::wallpaper(w.0, 96, 60);
                            let ci = egui::ColorImage::from_rgba_unmultiplied([96, 60], &img.rgba);
                            ui.ctx().load_texture(format!("wall-{}", w.0), ci, egui::TextureOptions::LINEAR)
                        })
                        .collect()
                });
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (i, w) in WALLPAPERS.iter().enumerate() {
                        let tex = thumbs[i].id();
                        let clicked = swatch(ui, vec2(52.0, 34.0), *id == w.0, |p, r| {
                            p.image(tex, r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        });
                        if clicked {
                            *id = w.0.into();
                        }
                    }
                });
            }
            Background::Gradient { angle, stops } => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (_, a, b, ang) in GRADIENTS {
                        let (ca, cb) = (Color32::from_rgb(a[0], a[1], a[2]), Color32::from_rgb(b[0], b[1], b[2]));
                        let sel = stops.first().map(|s| s.0) == Some(Color::rgb(a[0], a[1], a[2]));
                        if swatch(ui, vec2(34.0, 34.0), sel, |p, r| {
                            let mut mesh = egui::Mesh::default();
                            mesh.colored_vertex(r.left_top(), ca);
                            mesh.colored_vertex(r.right_top(), widgets::lerp_color(ca, cb, 0.5));
                            mesh.colored_vertex(r.right_bottom(), cb);
                            mesh.colored_vertex(r.left_bottom(), widgets::lerp_color(ca, cb, 0.5));
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            p.add(mesh);
                        }) {
                            *angle = ang;
                            *stops = vec![(Color::rgb(a[0], a[1], a[2]), 0.0), (Color::rgb(b[0], b[1], b[2]), 1.0)];
                        }
                    }
                });
                slider(ui, "Angle", angle, 0.0..=360.0, "°");
                for (i, (c, _)) in stops.iter_mut().enumerate() {
                    row(ui, if i == 0 { "From" } else { "To" }, |ui| {
                        widgets::color_edit(ui, c);
                    });
                }
            }
            Background::Color { color } => {
                row(ui, "Color", |ui| {
                    widgets::color_edit(ui, color);
                });
            }
            Background::Image { path } => {
                ui.horizontal(|ui| {
                    if ui.button("Choose image…").clicked()
                        && let Some(rel) = choose_image(ed)
                    {
                        *path = rel;
                    }
                    if ui.button("Desktop wallpaper").on_hover_text("Use your current macOS desktop picture").clicked() {
                        let tmp = std::env::temp_dir().join(format!("glide-wallpaper-{}.png", std::process::id()));
                        match super::macos::save_desktop_wallpaper(&tmp).and_then(|_| ed.package.import_asset(&tmp, "wallpaper")) {
                            Ok(rel) => *path = rel,
                            Err(e) => ed.status = Some(format!("Couldn't use the desktop wallpaper: {e:#}")),
                        }
                        let _ = std::fs::remove_file(&tmp);
                    }
                });
                caption(ui, &std::path::Path::new(path.as_str()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "No image".into()));
            }
        }
        // Blurring a flat colour or a gradient changes nothing.
        if matches!(s.background, Background::Wallpaper { .. } | Background::Image { .. }) {
            slider(ui, "Blur", &mut s.background_blur, 0.0..=60.0, " px");
        }
    });
    section(ui, "Frame", |ui| {
        slider(ui, "Padding", &mut s.padding, 0.0..=40.0, "%");
        row(ui, "Device", |ui| {
            use crate::project::model::DeviceFrame as D;
            segmented(ui, &mut s.device, &[(D::None, "None"), (D::Laptop, "Laptop"), (D::Phone, "Phone"), (D::Tablet, "Tablet")]);
        });
        if s.device == DeviceFrame::None {
            slider(ui, "Roundness", &mut s.corner_radius, 0.0..=60.0, " px");
        }
        slider(ui, "Inset", &mut s.inset.padding, 0.0..=60.0, " px");
        if s.inset.padding > 0.0 {
            row(ui, "Inset color", |ui| {
                widgets::color_edit(ui, &mut s.inset.color);
            });
            slider(ui, "Inset opacity", &mut s.inset.alpha, 0.0..=1.0, "");
        }
        row(ui, "Aspect ratio", |ui| {
            let label = Aspect::PRESETS.iter().find(|(_, a)| *a == s.aspect).map_or("Custom", |(n, _)| n);
            egui::ComboBox::from_id_salt("aspect").selected_text(label).show_ui(ui, |ui| {
                for (n, a) in Aspect::PRESETS {
                    ui.selectable_value(&mut s.aspect, a, n);
                }
            });
        });
    });
    section(ui, "Shadow", |ui| {
        slider(ui, "Intensity", &mut s.shadow.intensity, 0.0..=1.0, "");
        slider(ui, "Angle", &mut s.shadow.angle, 0.0..=360.0, "°");
        slider(ui, "Distance", &mut s.shadow.distance, 0.0..=100.0, " px");
        slider(ui, "Blur", &mut s.shadow.blur, 0.0..=100.0, " px");
    });
    if s.reveal_background(&before) {
        ed.status = Some(format!("Padding was 0 %, which hides the background, so it's now {} %.", Style::REVEAL_PADDING));
    }
    section(ui, "Crop", |ui| {
        let c = &mut s.crop;
        let (mut l, mut t, mut r, mut b) = (c.x, c.y, 1.0 - c.x - c.w, 1.0 - c.y - c.h);
        let mut ch = false;
        ch |= slider(ui, "Left", &mut l, 0.0..=0.45, "");
        ch |= slider(ui, "Right", &mut r, 0.0..=0.45, "");
        ch |= slider(ui, "Top", &mut t, 0.0..=0.45, "");
        ch |= slider(ui, "Bottom", &mut b, 0.0..=0.45, "");
        if ch {
            *c = crate::engine::geom::Rect::new(l, t, (1.0 - l - r).max(0.1), (1.0 - t - b).max(0.1));
        }
        if ui.small_button("Reset crop").clicked() {
            *c = crate::engine::geom::Rect::UNIT;
        }
    });
}

fn spring_picker(ui: &mut Ui, label: &str, cfg: &mut Option<SpringConfig>, presets: &[(&str, Option<SpringConfig>)]) {
    ui.label(label);
    ui.horizontal_wrapped(|ui| {
        for (name, p) in presets {
            let sel = cfg == p;
            if ui.selectable_label(sel, *name).clicked() {
                *cfg = *p;
            }
        }
    });
    if let Some(c) = cfg {
        egui::CollapsingHeader::new("Customize").id_salt(label).show(ui, |ui| {
            caption(ui, "Motion follows Newtonian physics: a mass on a spring.");
            slider(ui, "Rigidity", &mut c.stiffness, 5.0..=1500.0, "");
            slider(ui, "Smoothness", &mut c.damping, 5.0..=200.0, "");
            slider(ui, "Momentum", &mut c.mass, 0.1..=15.0, "");
        });
    }
}

fn cursor(ui: &mut Ui, d: &mut Project, no_data: bool) {
    let c = &mut d.style.cursor;
    if no_data {
        caption(ui, "This recording has no cursor data.");
    }
    section(ui, "Cursor", |ui| {
        toggle_row(ui, "Hide cursor", &mut c.hide);
        slider(ui, "Size", &mut c.size, 0.5..=4.0, "×");
        row(ui, "Style", |ui| {
            segmented(ui, &mut c.skin, &[(CursorSkin::Touch, "Touch"), (CursorSkin::Vector, "Vector"), (CursorSkin::System, "System")]);
        });
    });
    section(ui, "Movement", |ui| {
        spring_picker(
            ui,
            "Smoothing",
            &mut c.smoothing,
            &[
                ("Smooth", Some(SpringConfig::CURSOR_SMOOTH)),
                ("Medium", Some(SpringConfig::CURSOR_MEDIUM)),
                ("Rapid", Some(SpringConfig::CURSOR_RAPID)),
                ("Off", None),
            ],
        );
        slider(ui, "Tilt", &mut c.tilt, 0.0..=1.0, "");
        let mut idle = c.hide_idle_ms.is_some();
        if toggle_row(ui, "Hide when idle", &mut idle) {
            c.hide_idle_ms = idle.then_some(2000.0);
        }
        if let Some(ms) = &mut c.hide_idle_ms {
            slider(ui, "Idle after", ms, 500.0..=10_000.0, " ms");
        }
        let mut lp = c.loop_ms.is_some();
        if toggle_row(ui, "Loop position at end", &mut lp) {
            c.loop_ms = lp.then_some(1000.0);
        }
        if let Some(ms) = &mut c.loop_ms {
            slider(ui, "Loop over", ms, 300.0..=5000.0, " ms");
        }
        let mut freeze = c.freeze_ms > 0.0;
        if toggle_row(ui, "Freeze at end", &mut freeze) {
            c.freeze_ms = if freeze { 1000.0 } else { 0.0 };
        }
        if c.freeze_ms > 0.0 {
            slider(ui, "Freeze for", &mut c.freeze_ms, 200.0..=5000.0, " ms");
        }
    });
    section(ui, "Clicks", |ui| {
        let mut kind = match c.click_effect {
            ClickEffect::None => 0,
            ClickEffect::Circle => 1,
            ClickEffect::Ripple { .. } => 2,
        };
        if segmented(ui, &mut kind, &[(0, "None"), (1, "Circle"), (2, "Ripple")]) {
            c.click_effect = match kind {
                1 => ClickEffect::Circle,
                2 => ClickEffect::Ripple { strength: 2.0 },
                _ => ClickEffect::None,
            };
        }
        if let ClickEffect::Ripple { strength } = &mut c.click_effect {
            slider(ui, "Strength", strength, 0.5..=5.0, "");
        }
        toggle_row(ui, "Click sounds", &mut d.style.audio.click_sounds);
        if d.style.audio.click_sounds {
            slider(ui, "Click volume", &mut d.style.audio.click_volume, 0.0..=1.0, "");
        }
    });
}

fn camera(ui: &mut Ui, d: &mut Project, has: bool) {
    if !has {
        caption(ui, "Record with the camera on to add a camera bubble.");
        return;
    }
    let c = &mut d.style.camera;
    section(ui, "Camera", |ui| {
        toggle_row(ui, "Hide camera", &mut c.hide);
        widgets::camera_look(ui, c);
        if !c.shape.is_square() {
            row(ui, "Aspect", |ui| {
                segmented(ui, &mut c.aspect, &[(CameraAspect::Horizontal, "16:9"), (CameraAspect::Vertical, "9:16"), (CameraAspect::Square, "1:1"), (CameraAspect::Original, "Auto")]);
            });
        }
        slider(ui, "Size while zoomed", &mut c.zoom_scale, 0.3..=1.0, "×");
    });
    section(ui, "Fine position", |ui| {
        slider(ui, "Horizontal", &mut c.position.x, 0.0..=1.0, "");
        slider(ui, "Vertical", &mut c.position.y, 0.0..=1.0, "");
    });
}

fn captions(ui: &mut Ui, ed: &mut Editor, d: &mut Project) {
    section(ui, "Captions", |ui| {
        toggle_row(ui, "Show captions", &mut d.style.captions.show);
        slider(ui, "Size", &mut d.style.captions.size, 0.5..=2.0, "×");
        if ed.rec.transcript.is_empty() {
            caption(ui, "No transcript yet.");
            if ed.rec.meta.mic.is_some() {
                transcribe_controls(ui, ed);
            } else {
                caption(ui, "Record with the microphone to generate captions.");
            }
        } else {
            caption(ui, &format!("{} words — click a line to fix it", ed.rec.transcript.len()));
            ui.collapsing("Transcribe again", |ui| {
                caption(ui, "Replaces the current captions, including your edits.");
                transcribe_controls(ui, ed);
            });
            let lines = crate::engine::captions::lines(&ed.rec.transcript);
            let mut edited = None;
            for (i, line) in lines.iter().enumerate() {
                let id = ui.id().with(("caption-line", i, line.start.to_bits()));
                let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| line.text());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(widgets::fmt_time(line.start)).size(10.0).color(pal(ui).secondary).monospace());
                    let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(f32::INFINITY).font(egui::FontId::proportional(12.0)));
                    if r.changed() {
                        ui.data_mut(|d| d.insert_temp(id, buf.clone()));
                    }
                    if r.lost_focus() {
                        ui.data_mut(|d| d.remove::<String>(id));
                        if buf != line.text() {
                            edited = Some((line.clone(), buf.clone()));
                        }
                    }
                });
            }
            if let Some((line, text)) = edited {
                let mut words = ed.rec.transcript.clone();
                crate::engine::captions::edit_line(&mut words, &line, &text);
                match crate::project::write_json(&crate::transcribe::transcript_path(&ed.package), &words) {
                    Ok(()) => {
                        ed.rec.transcript = words;
                        ed.preview.set_project(&ed.project, &ed.rec);
                    }
                    Err(e) => ed.status = Some(format!("Couldn't save captions: {e:#}")),
                }
            }
        }
    });
}

/// Engine picker and the Transcribe button (or progress while running).
fn transcribe_controls(ui: &mut Ui, ed: &mut Editor) {
    use crate::transcribe::Engine;
    use crate::whisper::Model;
    use std::sync::atomic::Ordering;
    if ed.transcribing.is_some() {
        let p = &ed.transcribe_progress;
        let pct = p.permille.load(Ordering::Relaxed) as f32 / 10.0;
        let text = match (ed.caption_engine, p.phase.load(Ordering::Relaxed)) {
            (Engine::Whisper(m), 0) if !m.is_downloaded() => format!("Downloading Whisper {}… {pct:.0}%", m.name()),
            (Engine::Whisper(_), _) => format!("Transcribing on device… {pct:.0}%"),
            (Engine::Apple, _) => "Transcribing on device…".into(),
        };
        ui.horizontal(|ui| {
            ui.spinner();
            caption(ui, &text);
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    let mut whisper = matches!(ed.caption_engine, Engine::Whisper(_));
    row(ui, "Engine", |ui| {
        segmented(ui, &mut whisper, &[(true, "Whisper"), (false, "Apple")]);
    });
    match (whisper, ed.caption_engine) {
        (true, Engine::Apple) => ed.caption_engine = Engine::Whisper(crate::whisper::best_downloaded().unwrap_or(Model::Base)),
        (false, Engine::Whisper(_)) => ed.caption_engine = Engine::Apple,
        _ => {}
    }
    if let Engine::Whisper(mut model) = ed.caption_engine {
        let label = |m: Model| {
            if m.is_downloaded() { format!("{} (downloaded)", m.name()) } else { format!("{} ({} MB download)", m.name(), m.size_mb()) }
        };
        row(ui, "Model", |ui| {
            egui::ComboBox::from_id_salt("whisper-model").selected_text(label(model)).show_ui(ui, |ui| {
                for m in Model::ALL {
                    ui.selectable_value(&mut model, m, label(m));
                }
            });
        });
        ed.caption_engine = Engine::Whisper(model);
        caption(ui, "Runs on this Mac with Whisper. Larger models are more accurate but slower.");
    } else {
        caption(ui, "Uses Apple's on-device speech recognition (asks for Speech Recognition access).");
    }
    if ui.button("Transcribe microphone").clicked() {
        ed.start_transcription();
    }
}

fn audio(ui: &mut Ui, ed: &mut Editor, d: &mut Project) {
    let a = &mut d.style.audio;
    section(ui, "Audio", |ui| {
        slider(ui, "Volume", &mut a.volume, 0.0..=2.0, "");
        let peaks = *ed.peaks.lock().unwrap_or_else(|e| e.into_inner());
        let silent = |p: Option<f32>| peaks.is_some() && super::editor::TrackPeaks::silent(p);
        if ed.rec.meta.mic.is_some() {
            toggle_row(ui, "Mute microphone", &mut a.mute_mic);
            if silent(peaks.and_then(|p| p.mic)) {
                caption(ui, "The microphone only recorded silence.");
            }
            toggle_row(ui, "Enhance voice", &mut a.enhance_mic);
            if a.enhance_mic {
                caption(ui, "On-device voice isolation removes background noise.");
            }
        } else if ed.rec.meta.system_audio.is_some() {
            caption(ui, "The microphone was off for this recording (turn it on in the Record options next time).");
        }
        if ed.rec.meta.system_audio.is_some() {
            toggle_row(ui, "Mute system audio", &mut a.mute_system);
            if silent(peaks.and_then(|p| p.system)) {
                caption(ui, "System audio is silent: nothing played that ScreenCaptureKit could capture (sounds from command-line tools aren't).");
            }
        }
        if ed.rec.meta.mic.is_none() && ed.rec.meta.system_audio.is_none() {
            caption(ui, "This recording has no audio.");
        }
    });
    section(ui, "Background music", |ui| {
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked()
                && let Some(f) = rfd::FileDialog::new().add_filter("Audio", &["mp3", "m4a", "wav", "aac", "aiff"]).pick_file()
            {
                match ed.package.import_asset(&f, "music") {
                    Ok(rel) => a.music = Some(rel),
                    Err(e) => ed.status = Some(format!("Couldn't use that music file: {e:#}")),
                }
            }
            if a.music.is_some() && ui.button("Remove").clicked() {
                a.music = None;
            }
        });
        if let Some(m) = &a.music {
            caption(ui, &std::path::Path::new(m).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if crate::export::audio::music_missing(Some(&ed.package.root), a) {
                ui.label(RichText::new("Music file missing").size(theme::SECONDARY).color(Color32::from_rgb(255, 69, 58)));
            }
            slider(ui, "Music volume", &mut a.music_volume, 0.0..=1.0, "");
        }
    });
    section(ui, "Voice-over", |ui| {
        match ed.voice.as_ref().map(|v| v.elapsed()) {
            Some(elapsed) => {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, Color32::from_rgb(255, 59, 48));
                    ui.label(format!("Recording {elapsed:.1} s"));
                    if ui.button("Stop").clicked() {
                        ed.deferred = Some(Deferred::StopVoice);
                    }
                });
            }
            None => {
                if ui.button("Record from playhead").on_hover_text("Narrate over the video; the preview plays silently").clicked() {
                    ed.deferred = Some(Deferred::StartVoice);
                }
                if crate::capture::camera::builtin_mic_off(None) {
                    caption(ui, "The lid is closed, so the built-in microphone only records silence. Pick another input in System Settings › Sound.");
                }
            }
        }
        if !d.timeline.voiceovers.is_empty() {
            toggle_row(ui, "Lower recorded audio", &mut d.style.audio.duck_under_voiceover);
            if d.style.audio.duck_under_voiceover {
                slider(ui, "Level under voice", &mut d.style.audio.duck_level, 0.0..=1.0, "");
            }
            let mut remove = None;
            for vo in &mut d.timeline.voiceovers {
                ui.horizontal(|ui| {
                    caption(ui, &format!("{} · {:.1} s", widgets::fmt_time(vo.start), vo.duration / 1000.0));
                    ui.add(egui::Slider::new(&mut vo.volume, 0.0..=2.0).show_value(false));
                    if ui.small_button("Delete").clicked() {
                        remove = Some(vo.id);
                    }
                });
            }
            if let Some(id) = remove {
                d.timeline.voiceovers.retain(|v| v.id != id);
            }
        }
    });
}

fn shortcuts(ui: &mut Ui, d: &mut Project, keys: usize) {
    let s = &mut d.style.shortcuts;
    section(ui, "Keyboard shortcuts", |ui| {
        toggle_row(ui, "Show shortcuts", &mut s.show);
        toggle_row(ui, "Include single keys", &mut s.single_keys);
        slider(ui, "Size", &mut s.size, 0.5..=2.0, "×");
        if keys == 0 {
            caption(ui, "No key presses were recorded. Grant Glide Input Monitoring in System Settings › Privacy & Security to capture them.");
        } else {
            caption(ui, &format!("{keys} key events recorded"));
        }
    });
}

fn animation(ui: &mut Ui, ed: &mut Editor, d: &mut Project) {
    section(ui, "Zoom animation", |ui| {
        let mut cfg = Some(d.style.screen_spring);
        spring_picker(ui, "Style", &mut cfg, &[("Focused", Some(SpringConfig::SCREEN_FOCUSED)), ("Smooth", Some(SpringConfig::SCREEN_SMOOTH))]);
        if let Some(c) = cfg {
            d.style.screen_spring = c;
        }
    });
    section(ui, "Motion blur", |ui| {
        let m = &mut d.style.motion_blur;
        slider(ui, "Amount", &mut m.amount, 0.0..=1.0, "");
        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
            slider(ui, "Cursor", &mut m.cursor, 0.0..=1.0, "");
            slider(ui, "Screen movement", &mut m.screen_move, 0.0..=1.0, "");
            slider(ui, "Zoom", &mut m.screen_zoom, 0.0..=1.0, "");
        });
    });
    section(ui, "Automatic zooms", |ui| {
        slider(ui, "Default zoom", &mut d.style.default_zoom, 1.2..=4.0, "×");
        if ui.button("Regenerate from clicks").clicked() {
            let events = crate::engine::events::Events::new(ed.rec.mouse.clone());
            let zooms = crate::engine::zoom::auto_zooms(&events.downs, ed.rec.meta.duration_ms, d.style.default_zoom, &mut d.timeline);
            d.timeline.zooms = zooms;
        }
        let n = d.timeline.zooms.len();
        caption(ui, &format!("{n} zoom{}", if n == 1 { "" } else { "s" }));
    });
    section(ui, "Typing", |ui| {
        let runs = crate::engine::typing::ranges(&ed.rec.keys);
        if runs.is_empty() {
            caption(ui, if ed.rec.keys.is_empty() { "No key presses were recorded." } else { "No typing sections found." });
            return;
        }
        let secs: f64 = runs.iter().map(|(s, e)| e - s).sum::<f64>() / 1000.0;
        caption(ui, &format!("{} typing section{} ({secs:.1} s). Speed them up:", runs.len(), if runs.len() == 1 { "" } else { "s" }));
        ui.horizontal(|ui| {
            for speed in [2.0, 3.0, 4.0] {
                if ui.button(format!("{speed}×")).clicked() {
                    let n = crate::engine::typing::speed_up(&mut d.timeline, &runs, speed);
                    ed.status = Some(format!("Sped up {n} typing section{} to {speed}×", if n == 1 { "" } else { "s" }));
                }
            }
        });
    });
}

fn zoom_panel(ui: &mut Ui, ed: &mut Editor, d: &mut Project, id: u64) {
    let Some(z) = d.timeline.zooms.iter_mut().find(|z| z.id == id) else { return };
    let mut delete = false;
    let mut split = false;
    section(ui, "Zoom", |ui| {
        slider(ui, "Zoom level", &mut z.scale, 1.0..=5.0, "×");
        let mut mode = match z.mode {
            ZoomMode::Auto => 0,
            ZoomMode::Manual { .. } => 1,
            ZoomMode::FollowMouse => 2,
        };
        if segmented(ui, &mut mode, &[(0, "Auto"), (1, "Manual"), (2, "Follow mouse")]) {
            z.mode = match mode {
                1 => ZoomMode::Manual { target: v2(0.5, 0.5) },
                2 => ZoomMode::FollowMouse,
                _ => ZoomMode::Auto,
            };
        }
        if let ZoomMode::Manual { target } = &mut z.mode {
            caption(ui, "Click the preview to choose where to zoom.");
            slider(ui, "X", &mut target.x, 0.0..=1.0, "");
            slider(ui, "Y", &mut target.y, 0.0..=1.0, "");
        } else {
            slider(ui, "Edge snapping", &mut z.snap_edges, 0.0..=0.5, "");
        }
        toggle_row(ui, "Instant (no animation)", &mut z.instant);
        toggle_row(ui, "Disabled", &mut z.disabled);
        caption(ui, &format!("{} – {}", widgets::fmt_time(z.start), widgets::fmt_time(z.end)));
        ui.horizontal(|ui| {
            split = ui.button("Split at playhead").on_hover_text("Cut this zoom in two at the playhead (⇧S)").clicked();
            delete = ui.button("Delete zoom").clicked();
        });
    });
    if split {
        ed.split_zoom_in(d, id);
    }
    if delete {
        d.timeline.zooms.retain(|z| z.id != id);
        ed.selection = Selection::None;
    }
    if ui.button("Done").clicked() {
        ed.selection = Selection::None;
    }
}

fn slice_panel(ui: &mut Ui, d: &mut Project, id: u64) {
    let Some(s) = d.timeline.slices.iter_mut().find(|s| s.id == id) else { return };
    section(ui, "Clip", |ui| {
        ui.label("Speed");
        ui.horizontal_wrapped(|ui| {
            for sp in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0] {
                if ui.selectable_label((s.speed - sp).abs() < 1e-6, format!("{sp}×")).clicked() {
                    s.speed = sp;
                }
            }
        });
        slider(ui, "Mic volume", &mut s.mic_volume, 0.0..=2.0, "");
        slider(ui, "System volume", &mut s.system_volume, 0.0..=2.0, "");
        toggle_row(ui, "Hide cursor", &mut s.hide_cursor);
        toggle_row(ui, "Raw cursor movement", &mut s.no_smoothing);
        caption(ui, &format!("Source {} – {}", widgets::fmt_time(s.start), widgets::fmt_time(s.end)));
    });
}

fn layout_panel(ui: &mut Ui, d: &mut Project, id: u64) {
    let Some(l) = d.timeline.layouts.iter_mut().find(|l| l.id == id) else { return };
    section(ui, "Camera layout", |ui| {
        segmented(ui, &mut l.kind, &[(LayoutKind::Both, "Both"), (LayoutKind::ScreenOnly, "Screen"), (LayoutKind::CameraOnly, "Camera")]);
        if l.kind == LayoutKind::Both {
            let mut size = l.camera_size.unwrap_or(0.35);
            if slider(ui, "Camera size", &mut size, 0.1..=0.8, "") {
                l.camera_size = Some(size);
            }
        }
    });
}

fn mask_panel(ui: &mut Ui, ed: &mut Editor, d: &mut Project, id: u64) {
    use crate::project::model::{Mask, MaskKind};
    let Some(m) = d.timeline.masks.iter_mut().find(|m| m.id == id) else { return };
    let mut delete = false;
    section(ui, "Mask", |ui| {
        let before = m.kind;
        segmented(ui, &mut m.kind, &[(MaskKind::Blur, "Blur"), (MaskKind::Pixelate, "Pixelate"), (MaskKind::Highlight, "Highlight")]);
        if m.kind != before {
            m.amount = Mask::default_amount(m.kind);
        }
        match m.kind {
            MaskKind::Blur => slider(ui, "Blur", &mut m.amount, 2.0..=60.0, " px"),
            MaskKind::Pixelate => slider(ui, "Pixel size", &mut m.amount, 4.0..=60.0, " px"),
            MaskKind::Highlight => slider(ui, "Dim", &mut m.amount, 0.1..=0.95, ""),
        };
        slider(ui, "Roundness", &mut m.radius, 0.0..=60.0, " px");
        caption(ui, "Drag on the preview to draw the region; drag inside it to move.");
        egui::CollapsingHeader::new("Position").show(ui, |ui| {
            slider(ui, "Left", &mut m.rect.x, 0.0..=1.0, "");
            slider(ui, "Top", &mut m.rect.y, 0.0..=1.0, "");
            slider(ui, "Width", &mut m.rect.w, 0.01..=1.0, "");
            slider(ui, "Height", &mut m.rect.h, 0.01..=1.0, "");
        });
        caption(ui, &format!("{} – {}", widgets::fmt_time(m.start), widgets::fmt_time(m.end)));
        ui.horizontal(|ui| {
            if ui.button("Delete mask").clicked() {
                delete = true;
            }
            if ui.button("Done").clicked() {
                ed.selection = Selection::None;
            }
        });
    });
    if delete {
        d.timeline.masks.retain(|m| m.id != id);
        ed.selection = Selection::None;
    }
}
