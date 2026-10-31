//! Recording setup and recent recordings: the window's start state (the
//! editor layout with no recording open) and the editor's title-bar controls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};
use serde::{Deserialize, Serialize};

use super::hotkey::Combo;
use super::theme::{self, pal};
use super::widgets::{self, caption, row, segmented};
use crate::project::{self, Package};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    #[default]
    Display,
    Window,
    Area,
    /// A USB-connected iPhone or iPad.
    Device,
}

/// Recording preferences, remembered between launches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: SourceKind,
    pub display: u32,
    pub window: Option<u32>,
    /// The chosen window's "App — Title", to find it again when its id
    /// changes (window ids don't survive an app relaunch).
    pub window_label: Option<String>,
    /// Area as x, y, w, h in display points.
    pub area: [f64; 4],
    pub system_audio: bool,
    pub mic: bool,
    pub mic_id: Option<String>,
    pub camera: bool,
    pub camera_id: Option<String>,
    /// The camera bubble's look and place, shown live in the preview and
    /// given to each new recording.
    pub camera_style: crate::project::model::CameraStyle,
    pub keys: bool,
    pub countdown: bool,
    pub fps: u32,
    pub projects_dir: Option<PathBuf>,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    pub hotkeys: Hotkeys,
    /// iPhone/iPad unique id.
    pub ios_device: Option<String>,
    /// Window sizes (points) saved from the Resize menu.
    pub window_sizes: Vec<(u32, u32)>,
}

/// Global shortcuts (None = unbound).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub record: Option<Combo>,
    pub pause: Option<Combo>,
    pub cancel: Option<Combo>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        use super::hotkey::{CMD, KEY_R, OPTION, SHIFT};
        Hotkeys {
            record: Some(Combo::new(KEY_R, CMD | SHIFT | OPTION)),
            pause: Some(Combo::new(35, CMD | SHIFT | OPTION)), // P
            cancel: None,
        }
    }
}

impl Hotkeys {
    pub fn record_label(&self) -> String {
        self.record.map_or_else(String::new, |c| c.label())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source: SourceKind::Display,
            display: 0,
            window: None,
            window_label: None,
            area: [200.0, 150.0, 1280.0, 720.0],
            system_audio: true,
            mic: false,
            mic_id: None,
            camera: false,
            camera_id: None,
            camera_style: crate::project::model::CameraStyle { mirror: true, ..Default::default() },
            keys: true,
            countdown: true,
            // Older Macs (light mode) keep up best at 30.
            fps: if crate::hw::light() { 30 } else { 60 },
            projects_dir: None,
            hide_desktop_icons: false,
            hide_dock: false,
            hotkeys: Hotkeys::default(),
            ios_device: None,
            window_sizes: vec![],
        }
    }
}

fn settings_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/Glide/settings.json")
}

impl Settings {
    pub fn load() -> Self {
        project::read_json(&settings_path()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = settings_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = project::write_json(&p, self);
    }
    pub fn dir(&self) -> PathBuf {
        self.projects_dir.clone().unwrap_or_else(project::projects_dir)
    }

    /// Keep the chosen window when its id is gone: find it again by app and
    /// title (then by app alone), else forget it so Record asks again.
    /// Returns whether the settings changed.
    pub fn resolve_window(&mut self, dev: &Devices) -> bool {
        let Some(id) = self.window else { return false };
        if let Some(w) = dev.windows.iter().find(|w| w.id == id) {
            if self.window_label.as_ref() == Some(&w.label) {
                return false;
            }
            self.window_label = Some(w.label.clone());
            return true;
        }
        // An empty list means no permission (or nothing loaded yet).
        if dev.windows.is_empty() {
            return false;
        }
        let found = self.window_label.as_deref().and_then(|label| {
            let app = label.split(" — ").next().unwrap_or(label);
            dev.windows
                .iter()
                .find(|w| w.label == label)
                .or_else(|| dev.windows.iter().find(|w| w.label.split(" — ").next() == Some(app)))
        });
        self.window = found.map(|w| w.id);
        self.window_label = found.map(|w| w.label.clone());
        true
    }
}

#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// A window that can be recorded.
#[derive(Clone, Debug)]
pub struct WindowEntry {
    pub id: u32,
    pub label: String,
    /// Size in points.
    pub size: (f64, f64),
    /// Size of the display it's on, in points.
    pub display: (f64, f64),
}

/// What the sidebar and the canvas say while no iPhone or iPad is listed.
pub const IOS_CONNECT_HINT: &str = "Connect an iPhone or iPad with a cable, unlock it and trust this Mac.";

/// The saved iPhone/iPad choice after looking at what is connected: kept when
/// it is listed, else the first listed one. With nothing listed the saved id
/// stays, since the list is empty for the first seconds after launch and the
/// pick shouldn't be thrown away.
pub fn repair_ios_choice(current: Option<&str>, listed: &[Device]) -> Option<String> {
    match current {
        Some(c) if listed.iter().any(|d| d.id == c) => Some(c.to_owned()),
        _ => listed.first().map(|d| d.id.clone()).or_else(|| current.map(str::to_owned)),
    }
}

/// The saved iPhone/iPad, only if it is in the list. An id that discovery
/// hasn't returned would fail with "no longer connected", so it isn't previewed.
pub fn listed_ios_id(saved: Option<&str>, listed: &[Device]) -> Option<String> {
    saved.filter(|id| listed.iter().any(|d| d.id == *id)).map(str::to_owned)
}

#[derive(Default)]
pub struct Devices {
    pub displays: Vec<(u32, String)>,
    pub windows: Vec<WindowEntry>,
    pub cameras: Vec<Device>,
    pub mics: Vec<Device>,
    pub ios: Vec<Device>,
}

impl Devices {
    pub fn load() -> Self {
        use crate::capture::{camera, screen};
        Devices {
            displays: screen::displays()
                .map(|ds| ds.iter().enumerate().map(|(i, d)| (d.id, format!("Display {} — {}×{}", i + 1, d.points.0, d.points.1))).collect())
                .unwrap_or_default(),
            windows: screen::windows()
                .map(|ws| {
                    // Untitled windows too (the on-screen picker offers them).
                    let me = std::process::id() as i32;
                    // SAFETY: SCWindow/SCRunningApplication accessors.
                    let ws: Vec<_> = ws
                        .into_iter()
                        .filter(|w| !w.app.is_empty() && w.app != "Glide" && unsafe { w.window.owningApplication().is_none_or(|a| a.processID() != me) })
                        .collect();
                    let displays = crate::capture::winsize::display_sizes(&ws);
                    ws.into_iter()
                        .zip(displays)
                        .map(|(w, display)| {
                            let label = if w.title.is_empty() { w.app.clone() } else { format!("{} — {}", w.app, w.title) };
                            WindowEntry { id: w.id, label, size: w.size, display }
                        })
                        .collect()
                })
                .unwrap_or_default(),
            cameras: camera::cameras().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
            mics: camera::microphones().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
            ios: camera::ios_devices().into_iter().map(|d| Device { id: d.id, name: d.name }).collect(),
        }
    }
}

pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub modified: SystemTime,
    pub duration_ms: f64,
}

pub fn scan_projects(dir: &Path) -> Vec<ProjectEntry> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some(project::EXTENSION) {
                continue;
            }
            let pkg = Package::new(&path);
            let Ok(meta) = project::read_json::<crate::project::recording::RecordingMeta>(&pkg.rec("meta.json")) else { continue };
            let modified = std::fs::metadata(pkg.project_file()).or_else(|_| e.metadata()).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            out.push(ProjectEntry { name: pkg.name(), path, modified, duration_ms: meta.duration_ms });
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Thumbnails rendered on a background thread and cached on disk.
pub struct Thumbs {
    pub textures: HashMap<PathBuf, egui::TextureHandle>,
    pending: Arc<Mutex<Vec<(PathBuf, crate::render::Image)>>>,
    requested: std::collections::HashSet<PathBuf>,
    /// One background worker renders thumbnails in order (each needs a
    /// decoder and a GPU renderer, so they're never made in parallel).
    queue: Option<std::sync::mpsc::Sender<(PathBuf, egui::Context)>>,
}

impl Default for Thumbs {
    fn default() -> Self {
        Thumbs { textures: HashMap::new(), pending: Arc::new(Mutex::new(vec![])), requested: Default::default(), queue: None }
    }
}

impl Thumbs {
    pub fn get(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureId> {
        for (p, img) in self.pending.lock().unwrap().drain(..) {
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
            self.textures.insert(p.clone(), ctx.load_texture(p.to_string_lossy(), ci, egui::TextureOptions::LINEAR));
        }
        if let Some(t) = self.textures.get(path) {
            return Some(t.id());
        }
        if self.requested.insert(path.to_path_buf()) {
            let pending = self.pending.clone();
            let tx = self.queue.get_or_insert_with(|| {
                let (tx, rx) = std::sync::mpsc::channel::<(PathBuf, egui::Context)>();
                std::thread::spawn(move || {
                    for (p, ctx) in rx {
                        if let Some(img) = thumbnail(&p) {
                            pending.lock().unwrap().push((p, img));
                            ctx.request_repaint();
                        }
                    }
                });
                tx
            });
            let _ = tx.send((path.to_path_buf(), ctx.clone()));
        }
        None
    }

    pub fn forget(&mut self, path: &Path) {
        self.textures.remove(path);
        self.requested.remove(path);
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Load `thumb.png` (unless the project changed since) or render one from a
/// moment without a zoom, so it shows the whole composition.
fn thumbnail(path: &Path) -> Option<crate::render::Image> {
    let pkg = Package::new(path);
    let thumb = path.join("thumb.png");
    if modified(&thumb) >= modified(&pkg.project_file())
        && let Ok(img) = image::open(&thumb)
    {
        let img = img.to_rgba8();
        return Some(crate::render::Image { width: img.width(), height: img.height(), rgba: img.into_raw() });
    }
    objc2::rc::autoreleasepool(|_| {
        let (p, rec) = pkg.load().ok()?;
        let mut scene = crate::engine::scene::Scene::new(p.clone(), &rec, 30.0);
        let d = scene.duration();
        let t = [(d * 0.25).min(1500.0), d * 0.1, d * 0.5, d * 0.75, 0.0]
            .into_iter()
            .find(|&t| crate::engine::zoom::active(&p.timeline.zooms, scene.map.to_source(t)).is_none())
            .unwrap_or(0.0);
        let frame = scene.frame(t, 270.0);
        let gpu = crate::render::Gpu::headless().ok()?;
        let mut r = crate::render::Renderer::new(gpu);
        r.prepare(&p.style, &rec, Some(path));
        let mut src = crate::media::FrameSource::open(&pkg.rec(&rec.meta.screen.file)).ok()?;
        if let Some(f) = src.frame_at(frame.source_ms) {
            f.with_bgra(|v| r.set_screen(v));
        }
        if let Some(cam) = &rec.meta.camera
            && let Ok(mut c) = crate::media::FrameSource::open(&pkg.rec(&cam.file))
            && let Some(f) = c.frame_at(frame.source_ms - cam.offset_ms)
        {
            f.with_bgra(|v| r.set_camera(v));
        }
        let img = r.render_to_image(&frame, &p.style);
        let _ = image::save_buffer(&thumb, &img.rgba, img.width, img.height, image::ExtendedColorType::Rgba8);
        Some(img)
    })
}

/// True while a shortcut button waits for keys (global hotkeys are paused
/// so the old combination reaches the field instead of starting a recording).
/// The start view's canvas in pixels (w << 32 | h), as last laid out; the
/// live preview is scaled to it.
static CANVAS_PX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn canvas_px() -> [u32; 2] {
    let v = CANVAS_PX.load(std::sync::atomic::Ordering::Relaxed);
    [(v >> 32) as u32, v as u32]
}

pub static CAPTURING_SHORTCUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A shortcut button that records the next key combination when clicked.
fn shortcut_row(ui: &mut Ui, label: &str, combo: &mut Option<Combo>) -> bool {
    let id = ui.id().with(("shortcut", label));
    let capturing = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if capturing {
        CAPTURING_SHORTCUT.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let mut changed = false;
    row(ui, label, |ui| {
        let text = if capturing { "Press keys…".to_string() } else { combo.map_or("None".into(), |c| c.label()) };
        let b = ui.add(egui::Button::new(text).selected(capturing).min_size(vec2(110.0, 0.0)));
        if b.clicked() {
            ui.data_mut(|d| {
                d.insert_temp(id, !capturing);
            });
        }
        if capturing {
            let pressed = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            match pressed {
                Some((egui::Key::Escape, _)) => {
                    ui.data_mut(|d| d.insert_temp(id, false));
                }
                Some((egui::Key::Backspace | egui::Key::Delete, _)) => {
                    *combo = None;
                    changed = true;
                    ui.data_mut(|d| d.insert_temp(id, false));
                }
                Some((k, m)) => {
                    if let Some(c) = Combo::from_egui(k, m) {
                        *combo = Some(c);
                        changed = true;
                        ui.data_mut(|d| d.insert_temp(id, false));
                    }
                }
                None => {}
            }
        }
    });
    changed
}

pub enum Action {
    None,
    Record,
    SelectArea,
    HotkeysChanged,
    Open(PathBuf),
    RequestPermissions,
    /// Close the open recording (back to recording setup).
    Back,
    ResizeWindow { id: u32, width: u32, height: u32 },
    /// Pick a window on screen.
    SelectWindow,
    /// Ask again which iPhones and iPads are connected, and retry a preview
    /// that failed.
    RefreshIos,
    /// Remember the settings now (e.g. the camera bubble was restyled).
    SaveSettings,
}

/// Outcome of the last window resize, shown under the window picker.
static RESIZE_NOTE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn set_resize_note(note: Option<String>) {
    *RESIZE_NOTE.lock().unwrap_or_else(|e| e.into_inner()) = note;
}

fn size_label((w, h): (u32, u32)) -> String {
    format!("{w} × {h}")
}

/// The chosen window's size with a Resize menu (common sizes, saved sizes,
/// custom). Returns the size picked this frame.
pub fn window_size_row(ui: &mut Ui, s: &mut Settings, w: &WindowEntry) -> Option<(u32, u32)> {
    use crate::capture::winsize::PRESETS;
    let current = (w.size.0.round() as u32, w.size.1.round() as u32);
    let fits = |(a, b): (u32, u32)| a as f64 <= w.display.0 && b as f64 <= w.display.1 && (a, b) != current;
    let custom_id = ui.id().with("custom-window-size");
    let mut custom: Option<[u32; 2]> = ui.data(|d| d.get_temp(custom_id));
    let mut picked = None;
    ui.horizontal(|ui| {
        caption(ui, &format!("{} pt", size_label(current)));
        ui.menu_button("Resize", |ui| {
            for &size in PRESETS[0].1 {
                if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                    picked = Some(size);
                }
            }
            for (name, sizes) in &PRESETS[1..] {
                ui.menu_button(*name, |ui| {
                    for &size in *sizes {
                        if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                            picked = Some(size);
                        }
                    }
                });
            }
            ui.separator();
            if s.window_sizes.is_empty() {
                ui.add_enabled(false, egui::Button::new("No saved sizes"));
            }
            for &size in &s.window_sizes {
                if ui.add_enabled(fits(size), egui::Button::new(size_label(size))).clicked() {
                    picked = Some(size);
                }
            }
            let known = PRESETS.iter().any(|(_, l)| l.contains(&current)) || s.window_sizes.contains(&current);
            if ui.add_enabled(!known, egui::Button::new("Save current size")).clicked() {
                s.window_sizes.push(current);
            }
            if !s.window_sizes.is_empty() {
                ui.menu_button("Delete saved size", |ui| {
                    let mut gone = None;
                    for &size in &s.window_sizes {
                        if ui.button(size_label(size)).clicked() {
                            gone = Some(size);
                        }
                    }
                    if let Some(g) = gone {
                        s.window_sizes.retain(|x| *x != g);
                    }
                });
            }
            ui.separator();
            if ui.button("Custom…").clicked() {
                custom = Some([current.0, current.1]);
            }
        });
    });
    if let Some(c) = &mut custom {
        let mut done = false;
        ui.horizontal(|ui| {
            let max = (w.display.0.min(10_000.0) as u32, w.display.1.min(10_000.0) as u32);
            ui.add(egui::DragValue::new(&mut c[0]).range(200..=max.0).suffix(" w"));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut c[1]).range(200..=max.1).suffix(" h"));
            // Consume Return/Esc so a surrounding picker doesn't act on them too.
            if ui.button("Resize").clicked() || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                picked = Some((c[0], c[1]));
                done = true;
            }
            if ui.button("Cancel").clicked() || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                done = true;
            }
        });
        if done {
            custom = None;
        }
    }
    ui.data_mut(|d| match custom {
        Some(c) => {
            d.insert_temp(custom_id, c);
        }
        None => {
            d.remove::<[u32; 2]>(custom_id);
        }
    });
    if let Some(note) = RESIZE_NOTE.lock().unwrap_or_else(|e| e.into_inner()).as_deref() {
        caption(ui, note);
    }
    picked
}

/// A sidebar section heading (small caps, as in Rec).
fn heading(ui: &mut Ui, title: &str) {
    ui.add_space(12.0);
    ui.label(RichText::new(title).font(theme::semibold(theme::SECONDARY)).color(pal(ui).secondary));
    ui.add_space(4.0);
}

/// Combo box filling the sidebar's width.
fn combo<T: Clone + PartialEq>(ui: &mut Ui, id: &str, value: &mut T, options: &[(T, String)], enabled: bool) {
    let current = options.iter().find(|(v, _)| v == value).map(|(_, l)| l.clone()).unwrap_or_default();
    ui.add_enabled_ui(enabled, |ui| {
        // Long names (window titles) end in "…" rather than widen the
        // sidebar past the window, which pushed the controls out of view.
        let width = ui.available_width();
        let r = egui::ComboBox::from_id_salt(id).width(width).truncate().selected_text(current.clone()).show_ui(ui, |ui| {
            ui.set_max_width(width.max(360.0));
            for (v, label) in options {
                if ui.add(egui::Button::selectable(v == value, label.as_str()).truncate()).on_hover_text(label).clicked() {
                    *value = v.clone();
                }
            }
        });
        r.response.on_hover_text(current);
    });
}

/// Equal-width selectable buttons across the sidebar.
fn choice<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = (ui.available_width() - 4.0 * (options.len() - 1) as f32) / options.len() as f32;
        for (v, label) in options {
            let b = egui::Button::selectable(*value == *v, RichText::new(*label).size(theme::SECONDARY + 1.0)).min_size(vec2(w, 26.0));
            if ui.add(b).clicked() {
                *value = *v;
            }
        }
    });
}

/// Recording setup for the sidebar: scene, screen source, camera, audio and
/// options.
pub fn record_settings(ui: &mut Ui, s: &mut Settings, dev: &mut Devices, permission_note: Option<&str>) -> Action {
    let mut action = Action::None;
    if let Some(note) = permission_note {
        ui.add_space(8.0);
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(255, 159, 10, 30))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.label(RichText::new(note).size(12.0));
                if ui.button("Request access").clicked() {
                    action = Action::RequestPermissions;
                }
            });
    }

    heading(ui, "SCENE");
    choice(ui, &mut s.camera, &[(false, "Screen"), (true, "Screen + Cam")]);

    heading(ui, "SCREEN");
    choice(ui, &mut s.source, &[(SourceKind::Display, "Display"), (SourceKind::Window, "Window"), (SourceKind::Area, "Area"), (SourceKind::Device, "iPhone")]);
    ui.add_space(6.0);
    match s.source {
        SourceKind::Display => {
            let mut displays = vec![(0, "Main display".to_string())];
            displays.extend(dev.displays.iter().cloned());
            combo(ui, "display", &mut s.display, &displays, true);
        }
        SourceKind::Window => {
            let chosen = s.window.and_then(|w| dev.windows.iter().find(|d| d.id == w)).cloned();
            let mut windows = vec![(None, "Choose a window".to_string())];
            windows.extend(dev.windows.iter().map(|d| (Some(d.id), d.label.clone())));
            ui.horizontal(|ui| {
                let w = ui.available_width() - 30.0;
                ui.allocate_ui(vec2(w, 24.0), |ui| combo(ui, "window", &mut s.window, &windows, true));
                if ui.button("↻").on_hover_text("Refresh windows").clicked() {
                    *dev = Devices::load();
                }
            });
            if let Some(w) = &chosen
                && let Some((width, height)) = window_size_row(ui, s, w)
            {
                action = Action::ResizeWindow { id: w.id, width, height };
            }
            if ui.small_button("Pick on screen…").on_hover_text("Highlight windows on the display and click one").clicked() {
                action = Action::SelectWindow;
            }
        }
        SourceKind::Device => {
            s.ios_device = repair_ios_choice(s.ios_device.as_deref(), &dev.ios);
            if dev.ios.is_empty() {
                caption(ui, IOS_CONNECT_HINT);
            } else {
                let devices: Vec<_> = dev.ios.iter().map(|d| (Some(d.id.clone()), d.name.clone())).collect();
                combo(ui, "ios", &mut s.ios_device, &devices, true);
            }
            if ui.small_button("Refresh").clicked() {
                action = Action::RefreshIos;
            }
        }
        SourceKind::Area => {
            ui.horizontal(|ui| {
                if ui.button("Select area…").on_hover_text("Drag a region on the screen").clicked() {
                    action = Action::SelectArea;
                }
                caption(ui, &format!("{:.0} × {:.0} at {:.0}, {:.0}", s.area[2], s.area[3], s.area[0], s.area[1]));
            });
            egui::CollapsingHeader::new("Exact size").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, l) in ["x", "y", "w", "h"].iter().enumerate() {
                        ui.label(*l);
                        ui.add(egui::DragValue::new(&mut s.area[i]).speed(4.0).range(0.0..=8000.0));
                    }
                });
                ui.horizontal(|ui| {
                    for (name, w, h) in [("1280×720", 1280.0, 720.0), ("1920×1080", 1920.0, 1080.0), ("1080×1080", 1080.0, 1080.0)] {
                        if ui.small_button(name).clicked() {
                            s.area[2] = w;
                            s.area[3] = h;
                        }
                    }
                });
            });
        }
    }

    heading(ui, "CAMERA");
    let mut cams = vec![(None, "Default camera".to_string())];
    cams.extend(dev.cameras.iter().map(|m| (Some(m.id.clone()), m.name.clone())));
    combo(ui, "cam", &mut s.camera_id, &cams, s.camera);
    if !s.camera {
        caption(ui, "Pick Screen + Cam to record the camera too.");
    } else {
        ui.add_space(6.0);
        if widgets::camera_look(ui, &mut s.camera_style) && !ui.ctx().input(|i| i.pointer.any_down()) {
            action = Action::SaveSettings;
        }
        caption(ui, "Drag the camera in the preview to move it; it snaps to corners.");
    }

    heading(ui, "AUDIO");
    ui.checkbox(&mut s.system_audio, "System audio");
    ui.checkbox(&mut s.mic, "Microphone");
    let mut mics = vec![(None, "Default microphone".to_string())];
    mics.extend(dev.mics.iter().map(|m| (Some(m.id.clone()), m.name.clone())));
    combo(ui, "mic", &mut s.mic_id, &mics, s.mic);
    if s.mic && crate::capture::camera::builtin_mic_off(s.mic_id.as_deref()) {
        caption(ui, "The lid is closed, so the built-in microphone only records silence. Choose another microphone.");
    }

    heading(ui, "OPTIONS");
    ui.checkbox(&mut s.keys, "Record key presses");
    ui.checkbox(&mut s.countdown, "3 s countdown");
    if matches!(s.source, SourceKind::Display | SourceKind::Area) {
        ui.checkbox(&mut s.hide_desktop_icons, "Hide desktop icons");
        ui.checkbox(&mut s.hide_dock, "Hide Dock");
    }
    row(ui, "Frame rate", |ui| {
        segmented(ui, &mut s.fps, &[(60, "60"), (30, "30")]);
    });
    egui::CollapsingHeader::new("Global shortcuts").show(ui, |ui| {
        let mut changed = false;
        changed |= shortcut_row(ui, "Start / stop", &mut s.hotkeys.record);
        changed |= shortcut_row(ui, "Pause / resume", &mut s.hotkeys.pause);
        changed |= shortcut_row(ui, "Cancel recording", &mut s.hotkeys.cancel);
        caption(ui, "Click a shortcut, then press the new keys (⌫ clears, ⎋ keeps it).");
        if changed {
            action = Action::HotkeysChanged;
        }
    });
    action
}

/// One line describing what Record will capture.
pub fn source_summary(s: &Settings, dev: &Devices) -> String {
    let source = match s.source {
        SourceKind::Display => dev.displays.iter().find(|d| d.0 == s.display).map_or("Main display".to_string(), |d| d.1.split(" — ").next().unwrap_or("Display").to_string()),
        SourceKind::Window => s.window.and_then(|w| dev.windows.iter().find(|d| d.id == w)).map_or("A window (pick it next)".to_string(), |d| d.label.clone()),
        SourceKind::Area => format!("Area {:.0} × {:.0}", s.area[2], s.area[3]),
        SourceKind::Device => s.ios_device.as_ref().and_then(|id| dev.ios.iter().find(|d| &d.id == id)).map_or("iPhone / iPad".to_string(), |d| d.name.clone()),
    };
    let on = |b: bool| if b { "on" } else { "off" };
    format!("{source}  ·  system audio {}  ·  mic {}  ·  camera {}", on(s.system_audio), on(s.mic), on(s.camera))
}

fn source_name(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Display => "Display",
        SourceKind::Window => "Window",
        SourceKind::Area => "Area",
        SourceKind::Device => "iPhone",
    }
}

/// Recent recordings, newest first, with small thumbnails; plus Open… and
/// the projects folder. Used as the popover off the window title.
pub fn recents(ui: &mut Ui, projects: &[ProjectEntry], thumbs: &mut Thumbs, current: Option<&Path>, dir: &Path) -> Action {
    const WIDTH: f32 = 340.0;
    ui.set_width(WIDTH);
    ui.label(RichText::new("Recent recordings").font(theme::semibold(12.0)).color(pal(ui).secondary));
    let action = recent_list(ui, projects, thumbs, current, WIDTH, 10);
    ui.separator();
    if ui.button("Open…").clicked()
        && let Some(f) = rfd::FileDialog::new().set_directory(dir).pick_folder()
    {
        return Action::Open(f);
    }
    if ui.button("Show projects folder").clicked() {
        super::macos::open_folder(dir);
    }
    action
}

/// Rows of recent recordings with thumbnails, newest first.
fn recent_list(ui: &mut Ui, projects: &[ProjectEntry], thumbs: &mut Thumbs, current: Option<&Path>, width: f32, max: usize) -> Action {
    let mut action = Action::None;
    let p = pal(ui);
    if projects.is_empty() {
        caption(ui, "Nothing recorded yet.");
    }
    for e in projects.iter().take(max) {
        let open = current == Some(e.path.as_path());
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 46.0), Sense::click());
        if resp.hovered() || open {
            ui.painter().rect_filled(rect, CornerRadius::same(6), if open { p.pressed } else { p.hover });
        }
        let thumb = egui::Rect::from_min_size(rect.min + vec2(4.0, 5.0), vec2(58.0, 36.0));
        ui.painter().rect_filled(thumb, CornerRadius::same(4), p.control);
        if let Some(tex) = thumbs.get(ui.ctx(), &e.path) {
            ui.painter().image(tex, thumb, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        let x = thumb.right() + 10.0;
        let text = ui.painter().with_clip_rect(rect.shrink(2.0));
        text.text(egui::pos2(x, rect.top() + 7.0), egui::Align2::LEFT_TOP, &e.name, theme::semibold(12.5), p.text);
        text.text(
            egui::pos2(x, rect.top() + 25.0),
            egui::Align2::LEFT_TOP,
            format!("{}  ·  {}", widgets::fmt_time(e.duration_ms), ago(e.modified)),
            egui::FontId::proportional(11.0),
            p.secondary,
        );
        if resp.clicked() && !open {
            action = Action::Open(e.path.clone());
        }
        resp.context_menu(|ui| {
            if ui.button("Show in Finder").clicked() {
                super::macos::reveal(&e.path);
            }
        });
    }
    action
}

/// A small down chevron at the right end of a menu button (Inter has no ▾;
/// the label leaves room for it).
fn chevron(ui: &Ui, r: &egui::Response) {
    let c = egui::pos2(r.rect.right() - 13.0, r.rect.center().y + 1.0);
    let stroke = Stroke::new(1.5, pal(ui).secondary);
    ui.painter().line_segment([c + vec2(-4.0, -2.5), c], stroke);
    ui.painter().line_segment([c, c + vec2(4.0, -2.5)], stroke);
}

/// What the editor's title bar needs from the app: recording settings and
/// recent recordings. Its `action` is handled after the frame.
pub struct Chrome<'a> {
    pub settings: &'a mut Settings,
    pub devices: &'a mut Devices,
    pub projects: &'a [ProjectEntry],
    pub thumbs: &'a mut Thumbs,
    pub action: Action,
}

impl Chrome<'_> {
    fn set(&mut self, a: Action) {
        if !matches!(a, Action::None) {
            self.action = a;
        }
    }

    /// Back, then the title, which opens recent recordings.
    pub fn title_left(&mut self, ui: &mut Ui, title: &str, open: Option<&Path>) {
        if open.is_some() && ui.button("‹ Back").on_hover_text("Close this recording and set up a new one").clicked() {
            self.set(Action::Back);
        }
        ui.add_space(8.0);
        let dir = self.settings.dir();
        let mut a = Action::None;
        let r = ui.menu_button(RichText::new(format!("{title}      ")).font(theme::semibold(theme::BODY)), |ui| {
            a = recents(ui, self.projects, self.thumbs, open, &dir);
        });
        chevron(ui, &r.response);
        r.response.on_hover_text("Recent recordings");
        self.set(a);
    }

    /// Compact record controls: Record now, and a menu for the source and
    /// audio/camera (laid out right to left, so Record comes last).
    pub fn record_controls(&mut self, ui: &mut Ui) {
        let s = &mut *self.settings;
        let mut a = Action::None;
        let r = ui.menu_button(format!("{}      ", source_name(s.source)), |ui| {
            for k in [SourceKind::Display, SourceKind::Window, SourceKind::Area, SourceKind::Device] {
                if ui.selectable_label(s.source == k, source_name(k)).clicked() {
                    s.source = k;
                }
            }
            ui.separator();
            ui.checkbox(&mut s.system_audio, "System audio");
            ui.checkbox(&mut s.mic, "Microphone");
            ui.checkbox(&mut s.camera, "Camera");
            ui.separator();
            if ui.button("More options…").on_hover_text("Close this recording and show every recording option").clicked() {
                a = Action::Back;
            }
        });
        chevron(ui, &r.response);
        r.response.on_hover_text("What to record");
        let rec = egui::Button::new(RichText::new("●  Record").color(Color32::from_rgb(255, 69, 58)).font(theme::semibold(theme::BODY)));
        if ui.add(rec).on_hover_text(format!("{}  ({})", source_summary(s, self.devices), s.hotkeys.record_label())).clicked() {
            a = Action::Record;
        }
        self.set(a);
    }
}

/// What the recording-setup view shows live: the source's preview, the
/// camera's, audio levels (None = that input is off) and why there's no
/// preview, if there isn't one.
#[derive(Default)]
pub struct LiveView {
    pub screen: Option<(egui::TextureId, egui::Vec2)>,
    pub camera: Option<(egui::TextureId, egui::Vec2)>,
    pub system_level: Option<f32>,
    pub mic_level: Option<f32>,
    pub note: Option<String>,
    /// Shown over the live screen (e.g. Glide itself covers the display).
    pub covered: Option<String>,
}

const RED: Color32 = Color32::from_rgb(255, 69, 58);
const SIDEBAR_W: f32 = 300.0;

/// Small rounded label (PREVIEW, …) in the toolbar.
fn pill(ui: &mut Ui, text: &str, fill: Color32, fg: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), theme::semibold(theme::SECONDARY), fg);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(16.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, fg);
}

/// Horizontal peak meter, -60..0 dBFS, green/yellow/red.
fn meter(ui: &mut Ui, label: &str, level: Option<f32>) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.add_sized(vec2(44.0, 14.0), egui::Label::new(RichText::new(label).size(theme::SECONDARY).color(p.secondary)));
        let (rect, _) = ui.allocate_exact_size(vec2(180.0, 8.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(3), p.control);
        let Some(level) = level else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "off", egui::FontId::proportional(9.0), p.tertiary);
            return;
        };
        let db = 20.0 * level.max(1e-6).log10();
        let frac = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let x = |f: f32| rect.left() + rect.width() * f;
        let at = frac(db);
        for (lo, hi, col) in [(0.0, frac(-18.0), Color32::from_rgb(0x34, 0xc7, 0x59)), (frac(-18.0), frac(-6.0), Color32::from_rgb(0xff, 0xcc, 0x00)), (frac(-6.0), 1.0, RED)] {
            let hi = hi.min(at);
            if hi > lo {
                painter.rect_filled(egui::Rect::from_x_y_ranges(x(lo)..=x(hi), rect.y_range()), CornerRadius::same(2), col);
            }
        }
    });
}

/// The bubble's rect for `style` over a preview of the screen at `frame`,
/// laid out as the editor lays it out over the output.
fn bubble_rect(style: &crate::project::model::CameraStyle, frame: egui::Rect, cam: egui::Vec2) -> egui::Rect {
    use crate::engine::{camera, geom::v2};
    let out = v2(frame.width() as f64, frame.height() as f64);
    let aspect = camera::aspect(style, v2(cam.x as f64, cam.y as f64));
    let r = camera::bubble_rect(out, aspect, style.size, style.position);
    egui::Rect::from_min_size(frame.min + vec2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32))
}

/// Anti-aliased mesh filling the convex outline `pts` (centre `c`): a 1 px
/// ramp to transparent along the edge, as egui feathers its own shapes.
fn outline_mesh(pts: &[egui::Pos2], c: egui::Pos2, tex: egui::TextureId, color: Color32, uv: impl Fn(egui::Pos2) -> egui::Pos2) -> egui::Mesh {
    let mut m = egui::Mesh::with_texture(tex);
    let n = pts.len() as u32;
    m.vertices.push(egui::epaint::Vertex { pos: c, uv: uv(c), color });
    for (i, &p) in pts.iter().enumerate() {
        let (prev, next) = (pts[(i + pts.len() - 1) % pts.len()], pts[(i + 1) % pts.len()]);
        let t = next - prev;
        let mut normal = vec2(t.y, -t.x).normalized();
        if normal.dot(p - c) < 0.0 {
            normal = -normal;
        }
        for (q, col) in [(p - normal * 0.5, color), (p + normal * 0.5, Color32::TRANSPARENT)] {
            m.vertices.push(egui::epaint::Vertex { pos: q, uv: uv(q), color: col });
        }
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let (ai, ao, bi, bo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        m.add_triangle(0, ai, bi);
        m.add_triangle(ai, ao, bi);
        m.add_triangle(bi, ao, bo);
    }
    m
}

/// The live camera over the preview `frame` in its bubble (shape, border,
/// shadow, mirror), draggable: it moves while dragged and snaps to a corner
/// when released near one.
fn camera_bubble(ui: &mut Ui, style: &mut crate::project::model::CameraStyle, frame: egui::Rect, tex: egui::TextureId, cam: egui::Vec2) -> Action {
    use crate::engine::{camera, geom::v2};
    const SNAP: f64 = 0.12;
    let mut action = Action::None;
    let bubble = bubble_rect(style, frame, cam);
    let resp = ui.interact(bubble, ui.id().with("camera-drag"), Sense::drag());
    if resp.dragged() {
        // Position is the bubble's place in the room the safe frame leaves.
        let m = frame.width().min(frame.height()) * camera::SAFE_MARGIN as f32;
        let room = (frame.size() - vec2(2.0 * m, 2.0 * m) - bubble.size()).max(vec2(1.0, 1.0));
        let min = bubble.min + resp.drag_delta() - frame.min - vec2(m, m);
        style.position = v2((min.x / room.x).clamp(0.0, 1.0) as f64, (min.y / room.y).clamp(0.0, 1.0) as f64);
    }
    let snap = |p: f64| if p < SNAP { Some(0.0) } else if p > 1.0 - SNAP { Some(1.0) } else { None };
    let corner = snap(style.position.x).zip(snap(style.position.y)).map(|(x, y)| v2(x, y));
    if resp.drag_stopped() {
        if let Some(c) = corner {
            style.position = c;
        }
        action = Action::SaveSettings;
    }
    let bubble = bubble_rect(style, frame, cam);
    let k = frame.height() / 1080.0;
    let radius = style.radius(bubble.width() as f64, bubble.height() as f64) as f32;
    let n = style.squircle_exponent() as f32;
    let painter = ui.painter_at(frame);
    if style.shadow {
        let r = if n > 0.0 { bubble.width().min(bubble.height()) * 0.3 } else { radius };
        let shadow = egui::epaint::Shadow { offset: [0, (8.0 * k).round() as i8], blur: (32.0 * k).clamp(4.0, 60.0) as u8, spread: 0, color: Color32::from_black_alpha(115) };
        painter.add(shadow.as_shape(bubble, CornerRadius::same(r.min(255.0) as u8)));
    }
    let bw = (style.border.max(0.0) as f32 * k).round().min(bubble.width().min(bubble.height()) / 2.0);
    if bw > 0.0 {
        let [r, g, b, _] = style.border_color.0;
        let pts = widgets::bubble_outline(bubble, radius, n);
        painter.add(outline_mesh(&pts, bubble.center(), egui::TextureId::default(), Color32::from_rgb(r, g, b), |_| egui::epaint::WHITE_UV));
    }
    let inner = bubble.shrink(bw);
    // Centre crop of the camera to the bubble's aspect, mirrored if asked.
    let crop = crate::engine::scene::centre_crop((cam.x / cam.y) as f64, (inner.width() / inner.height()) as f64);
    let uv = |p: egui::Pos2| {
        let (fx, fy) = ((p.x - inner.min.x) / inner.width(), (p.y - inner.min.y) / inner.height());
        let fx = if style.mirror { 1.0 - fx } else { fx };
        egui::pos2(crop.x as f32 + fx * crop.w as f32, crop.y as f32 + fy * crop.h as f32)
    };
    let pts = widgets::bubble_outline(inner, (radius - bw).max(0.0), n);
    painter.add(outline_mesh(&pts, inner.center(), tex, Color32::WHITE, uv));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
    }
    if resp.dragged()
        && let Some(c) = corner
    {
        // Where it will snap.
        let ghost = bubble_rect(&crate::project::model::CameraStyle { position: c, ..*style }, frame, cam);
        painter.rect_stroke(ghost, CornerRadius::same(6), Stroke::new(1.5, theme::accent().gamma_multiply(0.8)), egui::StrokeKind::Outside);
    }
    action
}

/// The window with no recording open, laid out like Rec: the selected
/// source live, filling the canvas (with the camera in a bubble), recording
/// setup and recent recordings in a sidebar on the right, and a bar along the
/// bottom with the record button, timer and audio meters.
pub fn start_view(
    ui: &mut Ui,
    s: &mut Settings,
    dev: &mut Devices,
    projects: &[ProjectEntry],
    thumbs: &mut Thumbs,
    permission_note: Option<&str>,
    live: &LiveView,
) -> Action {
    let mut chrome = Chrome { settings: s, devices: dev, projects, thumbs, action: Action::None };
    let p = pal(ui);
    egui::Panel::top("start-toolbar")
        .exact_size(theme::TOOLBAR_H)
        .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin { left: theme::toolbar_left(), right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            super::toolbar_drag(ui);
            ui.horizontal_centered(|ui| {
                chrome.title_left(ui, "New recording", None);
                ui.add_space(4.0);
                if live.screen.is_some() {
                    pill(ui, "PREVIEW", p.control, p.secondary);
                }
            });
            let rect = ui.max_rect();
            ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, p.separator));
        });

    egui::Panel::bottom("start-controls")
        .exact_size(68.0)
        .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin::symmetric(16, 10)))
        .show(ui, |ui| {
            let s = &*chrome.settings;
            let mut a = Action::None;
            ui.horizontal_centered(|ui| {
                // Record: a round button like a camera's.
                let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                let c = rect.center();
                ui.painter().circle_stroke(c, 21.0, Stroke::new(3.0, if resp.hovered() { p.text } else { p.secondary }));
                ui.painter().circle_filled(c, 16.0, RED);
                let hk = s.hotkeys.record_label();
                if resp.on_hover_text(format!("Start recording  {hk}\n{}", source_summary(s, chrome.devices))).clicked() {
                    a = Action::Record;
                }
                ui.add_space(10.0);
                ui.label(RichText::new("00:00").font(egui::FontId::monospace(22.0)).color(p.tertiary));
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    meter(ui, "System", live.system_level.filter(|_| s.system_audio));
                    meter(ui, "Mic", live.mic_level.filter(|_| s.mic));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Open Folder").on_hover_text("Show the projects folder in Finder").clicked() {
                        super::macos::open_folder(&s.dir());
                    }
                    let next = match s.source {
                        SourceKind::Area => "You'll drag to select the area next".to_string(),
                        SourceKind::Window if s.window.is_none() => "You'll pick the window on screen next".to_string(),
                        _ if s.countdown => format!("Record {}  ·  3 s countdown", source_name(s.source).to_lowercase()),
                        _ => format!("Record {}", source_name(s.source).to_lowercase()),
                    };
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{next}  ·  {hk}")).size(theme::SECONDARY).color(p.secondary));
                });
            });
            chrome.set(a);
        });

    egui::Panel::right("start-sidebar")
        .exact_size(SIDEBAR_W)
        .resizable(false)
        .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin { left: 14, right: 4, top: 4, bottom: 6 }))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let width = SIDEBAR_W - 32.0;
                ui.set_max_width(width);
                let a = record_settings(ui, chrome.settings, chrome.devices, permission_note);
                chrome.set(a);
                heading(ui, "RECENT RECORDINGS");
                let dir = chrome.settings.dir();
                if ui.add(egui::Button::new("Open a recording…").min_size(vec2(width, 26.0))).clicked()
                    && let Some(f) = rfd::FileDialog::new().set_directory(&dir).pick_folder()
                {
                    chrome.set(Action::Open(f));
                }
                ui.add_space(4.0);
                let a = recent_list(ui, chrome.projects, chrome.thumbs, None, width, 12);
                chrome.set(a);
                ui.add_space(8.0);
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin::same(16)))
        .show(ui, |ui| {
            let area = ui.available_rect_before_wrap();
            let backdrop = if ui.visuals().dark_mode { Color32::from_rgb(0x1c, 0x1c, 0x1e) } else { Color32::from_rgb(0xe4, 0xe4, 0xe7) };
            ui.painter().rect_filled(area, CornerRadius::same(8), backdrop);
            // The source, live, as large as the canvas allows: frames come
            // scaled to the canvas's pixels and are drawn 1:1, pixel-aligned.
            let ppp = ui.ctx().pixels_per_point();
            let px = |v: f32| (v * ppp).floor().max(0.0) as u64;
            CANVAS_PX.store(px(area.width()) << 32 | px(area.height()), std::sync::atomic::Ordering::Relaxed);
            let rect = match live.screen {
                Some((tex, size)) => {
                    let fit = (area.width() / size.x).min(area.height() / size.y).min(1.0);
                    let snap = |v: f32| (v * ppp).round() / ppp;
                    let min = area.center() - size * fit / 2.0;
                    let rect = egui::Rect::from_min_size(egui::pos2(snap(min.x), snap(min.y)), size * fit);
                    egui::Image::new((tex, size)).corner_radius(6).paint_at(ui, rect);
                    if let Some(note) = &live.covered {
                        let font = egui::FontId::proportional(13.0);
                        let galley = ui.painter().layout(note.clone(), font, p.secondary, rect.width() - 48.0);
                        let card = egui::Rect::from_center_size(rect.center(), galley.size() + vec2(32.0, 20.0));
                        ui.painter().rect_filled(card, CornerRadius::same(8), Color32::from_black_alpha(150));
                        ui.painter().galley(card.min + vec2(16.0, 10.0), galley, p.secondary);
                    }
                    rect
                }
                None => {
                    let note = live.note.as_deref().unwrap_or("Starting preview…");
                    ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, note, egui::FontId::proportional(13.0), p.secondary);
                    area
                }
            };
            // The camera bubble, styled and placed as it will be recorded.
            if let Some((cam, cam_size)) = live.camera
                && chrome.settings.camera
            {
                let a = camera_bubble(ui, &mut chrome.settings.camera_style, rect, cam, cam_size);
                chrome.set(a);
            }
        });
    chrome.action
}

fn ago(t: SystemTime) -> String {
    let s = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u32, label: &str) -> WindowEntry {
        WindowEntry { id, label: label.into(), size: (800.0, 600.0), display: (1440.0, 900.0) }
    }

    #[test]
    fn window_is_found_again_after_its_id_changes() {
        let mut s = Settings { window: Some(10), ..Default::default() };
        let mut dev = Devices { windows: vec![win(10, "Safari — Docs"), win(11, "Notes")], ..Default::default() };
        assert!(s.resolve_window(&dev));
        assert_eq!(s.window_label.as_deref(), Some("Safari — Docs"));
        assert!(!s.resolve_window(&dev));
        // Safari relaunched: same window, new id.
        dev.windows = vec![win(42, "Safari — Docs"), win(11, "Notes")];
        assert!(s.resolve_window(&dev));
        assert_eq!(s.window, Some(42));
        // Title changed: same app.
        dev.windows = vec![win(43, "Safari — Other"), win(11, "Notes")];
        s.resolve_window(&dev);
        assert_eq!(s.window, Some(43));
        // App gone: forget it so Record asks again.
        dev.windows = vec![win(11, "Notes")];
        s.resolve_window(&dev);
        assert_eq!(s.window, None);
        // No window list (no permission): keep what's chosen.
        s.window = Some(7);
        dev.windows.clear();
        assert!(!s.resolve_window(&dev));
        assert_eq!(s.window, Some(7));
    }

    fn phone(id: &str) -> Device {
        Device { id: id.into(), name: format!("{id} phone") }
    }

    #[test]
    fn repair_ios_choice_keeps_a_listed_pick() {
        assert_eq!(repair_ios_choice(Some("b"), &[phone("a"), phone("b")]).as_deref(), Some("b"));
    }

    #[test]
    fn repair_ios_choice_replaces_a_stale_pick_with_the_first_listed() {
        assert_eq!(repair_ios_choice(Some("old"), &[phone("a"), phone("b")]).as_deref(), Some("a"));
    }

    #[test]
    fn repair_ios_choice_keeps_the_pick_while_nothing_is_listed() {
        assert_eq!(repair_ios_choice(Some("old"), &[]).as_deref(), Some("old"));
    }

    #[test]
    fn repair_ios_choice_picks_the_first_when_none_was_saved() {
        assert_eq!(repair_ios_choice(None, &[phone("a")]).as_deref(), Some("a"));
        assert_eq!(repair_ios_choice(None, &[]), None);
    }

    #[test]
    fn listed_ios_id_never_returns_an_id_discovery_did_not_list() {
        assert_eq!(listed_ios_id(Some("old"), &[]), None);
        assert_eq!(listed_ios_id(Some("old"), &[phone("a")]), None);
        assert_eq!(listed_ios_id(Some("a"), &[phone("a")]).as_deref(), Some("a"));
        assert_eq!(listed_ios_id(None, &[phone("a")]), None);
    }
}
