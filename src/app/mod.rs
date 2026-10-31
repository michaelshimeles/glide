//! The Glide app: one main window (recording setup ↔ editor), a floating
//! recording control bar (native panel), a menu bar item and global
//! shortcuts (default ⌥⇧⌘R). egui on wgpu with a transparent titlebar and
//! Inter, sharing one GPU device with the editor's renderer.

pub mod editor;
pub mod maskwatch;
pub mod home;
pub mod hotkey;
pub mod ioswatch;
pub mod inspector;
pub mod lifecycle;
pub mod macos;
pub mod menubar;
pub mod player;
pub mod preview;
pub mod snapshot;
pub mod theme;
pub mod timeline;
pub mod updater;
pub mod voiceover;
pub mod recbar;
pub mod restart;
pub mod widgets;
pub mod winpick;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Color32, RichText, Sense, vec2};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId, WindowLevel};

use crate::capture::preview as livecap;
use crate::recorder::{self, Recorder, Target};
use crate::render::Gpu;
use editor::Editor;
use home::{Devices, Settings, SourceKind, Thumbs};
use lifecycle::{ExportGuard, Leaving, Switch};

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    ToggleRecording,
    TogglePause,
    Restart,
    Cancel,
    ShowWindow,
    OpenFolder,
    /// Close the open recording and show recording setup.
    Back,
    Open(PathBuf),
    /// Show the area picker; `true` starts recording when confirmed.
    SelectArea(bool),
    /// Pick a window on screen (true = record it right away).
    SelectWindow(bool),
    /// Record a source chosen from the menu bar.
    RecordSource(SourceKind),
    /// Start Sparkle, queued at launch so it doesn't hold up the first frame.
    StartUpdater,
    CheckForUpdates,
    Quit,
}

static QUEUE: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static PROXY: OnceLock<Mutex<EventLoopProxy<()>>> = OnceLock::new();
static DRAG: AtomicBool = AtomicBool::new(false);

fn wake() {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(());
    }
}

pub fn push(c: Cmd) {
    QUEUE.lock().unwrap().push(c);
    wake();
}

/// Empty toolbar space drags the window (the titlebar is transparent).
pub fn toolbar_drag(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let bg = ui.interact(rect, ui.id().with("drag"), Sense::click_and_drag());
    if bg.drag_started() {
        DRAG.store(true, Ordering::Relaxed);
    }
    if bg.double_clicked() {
        push(Cmd::ShowWindow);
    }
}

/// How long a failed iPhone preview waits before it is tried again.
const IOS_RETRY: Duration = Duration::from_secs(2);

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui: egui_winit::State,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    repaint_at: Option<Instant>,
    /// Clear to transparent and skip titlebar chrome (area picker).
    overlay: bool,
    /// The display an overlay covers (0 for other windows).
    display: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Main,
    /// The picker overlay on this display.
    Overlay(u32),
}

struct Starting<K, V> {
    key: K,
    rx: std::sync::mpsc::Receiver<Result<V, String>>,
    handle: std::thread::JoinHandle<()>,
}

/// Something started on a background thread for a key (a preview stream for
/// a source, a meter for a microphone): what's wanted, what's running and
/// what's starting. Changes settle before restarting; a key that failed
/// isn't retried until the wanted key changes, or until `reset_failed` or
/// `retry_failed` clears the failure.
struct Slot<K, V> {
    want: Option<K>,
    want_since: Option<Instant>,
    running: Option<(K, V)>,
    starting: Option<Starting<K, V>>,
    failed: Option<(K, String)>,
    /// When `failed` was set.
    failed_at: Option<Instant>,
    /// The last failure text printed, so a retry that fails the same way
    /// again isn't printed again.
    logged: Option<String>,
    /// Threads still dropping stopped values (stopping a capture session
    /// takes a moment, so it isn't done on the UI thread).
    dropping: Vec<std::thread::JoinHandle<()>>,
}

impl<K, V> Default for Slot<K, V> {
    fn default() -> Self {
        Slot { want: None, want_since: None, running: None, starting: None, failed: None, failed_at: None, logged: None, dropping: vec![] }
    }
}

impl<K: Clone + PartialEq + Send + 'static, V: Send + 'static> Slot<K, V> {
    /// Match `want`; `start` runs on a background thread. Returns true when
    /// the running value was stopped (its output is stale).
    fn update(&mut self, want: Option<K>, name: &str, start: fn(&K) -> Result<V, String>) -> bool {
        let mut stopped = false;
        if want != self.want {
            self.want = want.clone();
            self.want_since = Some(Instant::now());
            self.reset_failed();
        }
        if self.running.as_ref().is_some_and(|(k, _)| Some(k) != want.as_ref()) {
            let (_, v) = self.running.take().unwrap();
            self.drop_later(v);
            stopped = true;
        }
        if let Some(s) = &self.starting
            && let Ok(result) = s.rx.try_recv()
        {
            let s = self.starting.take().unwrap();
            let _ = s.handle.join();
            match result {
                Ok(v) if Some(&s.key) == want.as_ref() && self.running.is_none() => {
                    self.logged = None;
                    self.running = Some((s.key, v));
                }
                Ok(v) => self.drop_later(v),
                Err(e) => self.fail(s.key, name, e),
            }
        }
        // Wait for edits (an area being typed in) to settle before starting.
        let settled = self.want_since.is_none_or(|t| t.elapsed() >= Duration::from_millis(300));
        if let Some(k) = want
            && settled
            && self.running.is_none()
            && self.starting.is_none()
            && self.failed.as_ref().is_none_or(|(f, _)| *f != k)
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let key = k.clone();
            let handle = std::thread::spawn(move || {
                let _ = tx.send(objc2::rc::autoreleasepool(|_| start(&key)));
                wake();
            });
            self.starting = Some(Starting { key: k, rx, handle });
        }
        stopped
    }

    fn active(&self) -> bool {
        self.running.is_some() || self.starting.is_some()
    }

    /// Record `k` as failed with `e`, printing the text unless the last
    /// failure said the same.
    fn fail(&mut self, k: K, name: &str, e: String) {
        if self.logged.as_deref() != Some(e.as_str()) {
            crate::diag::note(&format!("glide: {name}: {e}"));
            self.logged = Some(e.clone());
        }
        self.failed = Some((k, e));
        self.failed_at = Some(Instant::now());
    }

    /// Stop the running value because it reported `msg`, and count its key as
    /// failed (so `retry_failed` can bring it back). Returns true if
    /// something was running, whose output is then stale.
    fn fail_running(&mut self, name: &str, msg: String) -> bool {
        let Some((k, v)) = self.running.take() else { return false };
        self.drop_later(v);
        self.fail(k, name, msg);
        true
    }

    /// Drop `v` on a thread of its own, remembered so `stop_and_join` can
    /// wait for it.
    fn drop_later(&mut self, v: V) {
        self.dropping.retain(|h| !h.is_finished());
        self.dropping.push(std::thread::spawn(move || drop(v)));
    }

    /// Stop everything now, on this thread, and return only when it has
    /// stopped: the running value is dropped here, a value still starting is
    /// joined and dropped, and earlier background drops are joined. For a
    /// single-client device that must be free before something else opens it.
    fn stop_and_join(&mut self) {
        drop(self.running.take());
        if let Some(s) = self.starting.take() {
            let _ = s.handle.join();
            if let Ok(Ok(v)) = s.rx.try_recv() {
                drop(v);
            }
        }
        for h in self.dropping.drain(..) {
            let _ = h.join();
        }
        self.want = None;
        self.reset_failed();
    }

    /// Forget a failure so the wanted key starts again on the next `update`.
    fn reset_failed(&mut self) {
        self.failed = None;
        self.failed_at = None;
    }

    /// Forget a failure that is at least `after` old at `now`. Returns
    /// whether it did.
    fn retry_failed(&mut self, now: Instant, after: Duration) -> bool {
        match self.failed_at {
            Some(at) if self.failed.is_some() && now.saturating_duration_since(at) >= after => {
                self.reset_failed();
                true
            }
            _ => false,
        }
    }

    /// Why the wanted key isn't running, if it failed.
    fn error(&self) -> Option<&str> {
        self.failed.as_ref().filter(|(k, _)| self.want.as_ref() == Some(k)).map(|(_, e)| e.as_str())
    }
}

/// Live preview state for recording setup: the source's stream (with its
/// system-audio level), the camera's, the microphone's meter, the textures
/// their frames go into and the levels shown.
#[derive(Default)]
struct Live {
    /// Source and whether to measure its audio.
    screen: Slot<(livecap::Source, bool), livecap::LivePreview>,
    camera: Slot<livecap::Source, livecap::LivePreview>,
    /// Microphone name (None = default input).
    mic: Slot<Option<String>, crate::capture::meter::MicMeter>,
    /// The screen preview: a native texture over the newest frame's
    /// IOSurface (no copies) and its size in pixels.
    tex: Option<(egui::TextureId, [u32; 2])>,
    /// Frames whose IOSurfaces back textures that may still be drawn.
    frames: std::collections::VecDeque<crate::render::interop::Wrapped>,
    interop: Option<crate::render::interop::TextureCache>,
    /// The registered native texture, reused across sources.
    tex_id: Option<egui::TextureId>,
    cam_tex: Option<egui::TextureHandle>,
    /// Displayed levels (peak hold with decay).
    levels: [f32; 2],
    /// When `levels` last decayed.
    levels_at: Option<Instant>,
}

/// Area picker state, in overlay-window points (= display points).
#[derive(Default)]
struct AreaPick {
    rect: Option<egui::Rect>,
    drag: Option<AreaDrag>,
    record_after: bool,
    display: u32,
    /// Set when the picker is confirmed or cancelled; the window closes.
    closed: bool,
    /// Picking a window instead of dragging an area.
    windows: Option<WindowMode>,
}

/// Window-picker state: windows front to back, the clicked one, and the one
/// a context menu was opened on.
#[derive(Default)]
struct WindowMode {
    list: Vec<winpick::PickWindow>,
    selected: Option<u32>,
    menu_for: Option<u32>,
}

#[derive(Clone, Copy)]
enum AreaDrag {
    New(egui::Pos2),
    Move(egui::Vec2),
    Corner(egui::Pos2),
}

enum View {
    /// No recording open: recording setup in the editor's layout.
    Start,
    Editor(Box<Editor>),
}

struct Recording {
    rec: Option<Recorder>,
    /// Countdown before capture starts.
    countdown_until: Option<Instant>,
}

type Finished = Result<PathBuf, String>;

/// Scripted run for `GLIDE_GUI_SELFTEST=1` (scripts/selftest.sh): record a
/// test pattern, stop, open it in the editor, export, check and quit. Never
/// touches the mouse or needs permissions.
struct SelfTest {
    phase: u8,
    at: Instant,
    paused_at: Option<Instant>,
    resumed: bool,
    /// The recording made by the test.
    package: Option<PathBuf>,
}

fn log(msg: &str) {
    eprintln!("selftest: {msg}");
}

/// Scripted playback check for `GLIDE_GUI_PLAYTEST=<package>`
/// (scripts/selftest.sh): open the project and play it through the
/// speakers (while the script records what the Mac plays), checking that
/// the output follows the volume, mute and playhead. Logs `playtest:` lines
/// and exits non-zero on failure.
struct PlayTest {
    path: PathBuf,
    phase: u8,
    at: Instant,
    started: Instant,
    /// Seconds to play before changing anything.
    secs: f64,
    /// Seconds to wait before opening the project (GLIDE_PLAYTEST_DELAY).
    delay: f64,
    max_drift: f64,
    loud_db: f32,
    volume: f64,
}

fn plog(msg: &str) {
    eprintln!("playtest: {msg}");
}

struct App {
    gpu: Option<Gpu>,
    main: Option<Win>,
    /// Floating recording controls (while recording).
    bar: Option<recbar::RecordBar>,
    /// Where the user last left the bar (screen points, bottom-left).
    bar_origin: Option<objc2_core_foundation::CGPoint>,
    /// Picker overlays, one per display.
    overlays: Vec<Win>,
    area: AreaPick,
    view: View,
    settings: Settings,
    devices: Devices,
    projects: Vec<home::ProjectEntry>,
    thumbs: Thumbs,
    recording: Option<Recording>,
    finishing: bool,
    /// Live preview of the selected source on the recording-setup canvas.
    live: Live,
    /// The main window is minimised, hidden or fully covered.
    main_occluded: bool,
    /// Native textures to unregister from the main window's renderer on
    /// its next redraw (a closed editor's preview).
    stale_textures: Vec<egui::TextureId>,
    /// When playback's next preview frame is due (redraws are paced to it).
    next_frame_at: Option<Instant>,
    finished: Arc<Mutex<Option<Finished>>>,
    restart: restart::Restart,
    /// Set by the thread that cancels the old take of a restart.
    restart_done: Arc<AtomicBool>,
    error: Option<String>,
    menubar: Option<menubar::MenuBar>,
    last_tick: Instant,
    open_at_launch: Option<PathBuf>,
    permission_note: Option<String>,
    selftest: Option<SelfTest>,
    playtest: Option<PlayTest>,
}

fn permission_note() -> Option<String> {
    use crate::capture::tcc::{self, Status};
    if tcc::screen_status() != Status::Granted {
        return Some("Glide needs Screen Recording access. Request it, then enable Glide in System Settings › Privacy & Security › Screen & System Audio Recording, and reopen Glide.".into());
    }
    None
}

impl App {
    fn new(open: Option<PathBuf>) -> App {
        let selftest = std::env::var_os("GLIDE_GUI_SELFTEST").map(|_| SelfTest { phase: 0, at: Instant::now(), paused_at: None, resumed: false, package: None });
        let playtest = std::env::var_os("GLIDE_GUI_PLAYTEST").map(|p| PlayTest {
            path: PathBuf::from(p),
            phase: 0,
            at: Instant::now(),
            started: Instant::now(),
            secs: std::env::var("GLIDE_PLAYTEST_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0),
            delay: std::env::var("GLIDE_PLAYTEST_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(0.5),
            max_drift: 0.0,
            loud_db: 0.0,
            volume: 1.0,
        });
        let scripted = selftest.is_some() || playtest.is_some();
        let mut settings = if scripted { Settings::default() } else { Settings::load() };
        if let Some(dir) = std::env::var_os("GLIDE_OUTPUT_DIR") {
            settings.projects_dir = Some(PathBuf::from(dir));
        }
        if selftest.is_some() {
            settings.countdown = false;
        }
        let projects = home::scan_projects(&settings.dir());
        App {
            gpu: None,
            main: None,
            bar: None,
            bar_origin: None,
            overlays: vec![],
            area: AreaPick::default(),
            view: View::Start,
            devices: Devices::load(),
            settings,
            projects,
            thumbs: Thumbs::default(),
            recording: None,
            finishing: false,
            live: Live::default(),
            main_occluded: false,
            stale_textures: vec![],
            next_frame_at: None,
            finished: Arc::new(Mutex::new(None)),
            restart: restart::Restart::Idle,
            restart_done: Arc::new(AtomicBool::new(false)),
            error: None,
            menubar: None,
            last_tick: Instant::now(),
            open_at_launch: open,
            permission_note: permission_note(),
            selftest,
            playtest,
        }
    }

    fn open_window(&mut self, el: &ActiveEventLoop, title: &str, size: (f64, f64), min: (f64, f64)) -> Win {
        use winit::platform::macos::WindowAttributesExtMacOS;
        let attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(winit::dpi::LogicalSize::new(size.0, size.1))
            .with_min_inner_size(winit::dpi::LogicalSize::new(min.0, min.1))
            .with_titlebar_transparent(true)
            .with_fullsize_content_view(true)
            .with_title_hidden(true);
        self.make_window(el, attrs, false)
    }

    /// Borderless transparent window covering a display (area picker).
    fn open_overlay(&mut self, el: &ActiveEventLoop, display: u32) -> Win {
        let (id, x, y, w, h) = macos::display_frame(display);
        let attrs = Window::default_attributes()
            .with_title("Select Area")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_position(winit::dpi::LogicalPosition::new(x, y))
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            .with_window_level(WindowLevel::AlwaysOnTop);
        let mut win = self.make_window(el, attrs, true);
        win.display = id;
        win
    }

    /// Cover every display with a picker overlay (once) and focus the one
    /// on `display`.
    fn open_overlays(&mut self, el: &ActiveEventLoop, display: u32) {
        if self.overlays.is_empty() {
            for d in macos::display_ids() {
                let w = self.open_overlay(el, d);
                self.overlays.push(w);
            }
        }
        for w in &self.overlays {
            if w.display == display {
                w.window.focus_window();
            }
            w.window.request_redraw();
        }
        macos::activate();
    }

    fn make_window(&mut self, el: &ActiveEventLoop, attrs: winit::window::WindowAttributes, overlay: bool) -> Win {
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let surface = match &self.gpu {
            Some(g) => g.instance.create_surface(window.clone()).expect("surface"),
            None => {
                let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
                let s = instance.create_surface(window.clone()).expect("surface");
                let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&s),
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                }))
                .expect("no compatible GPU adapter");
                let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("glide"),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    ..Default::default()
                }))
                .expect("create device");
                self.gpu = Some(Gpu { instance, adapter, device, queue });
                s
            }
        };
        let g = self.gpu.as_ref().unwrap();
        let caps = surface.get_capabilities(&g.adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let px = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: px.width.max(1),
            height: px.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: if overlay {
                [wgpu::CompositeAlphaMode::PreMultiplied, wgpu::CompositeAlphaMode::PostMultiplied]
                    .into_iter()
                    .find(|m| caps.alpha_modes.contains(m))
                    .unwrap_or(caps.alpha_modes[0])
            } else {
                caps.alpha_modes[0]
            },
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&g.device, &config);
        let renderer = egui_wgpu::Renderer::new(&g.device, format, egui_wgpu::RendererOptions::default());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let egui = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(g.device.limits().max_texture_dimension_2d as usize),
        );
        if !overlay {
            let (x, y) = theme::TRAFFIC_LIGHTS;
            macos::inset_traffic_lights(&window, x, y);
        }
        window.request_redraw();
        Win { window, surface, config, egui, ctx, renderer, repaint_at: None, overlay, display: 0 }
    }

    fn show_main(&mut self, el: &ActiveEventLoop) {
        if self.main.is_none() {
            let w = self.open_window(el, "Glide", (1320.0, 840.0), (980.0, 640.0));
            self.main = Some(w);
        }
        if let Some(w) = &self.main {
            w.window.set_visible(true);
            w.window.focus_window();
            w.window.request_redraw();
        }
        if !self.scripted() {
            macos::activate();
        }
    }

    /// Unregister the open editor's preview texture from the window's
    /// renderer (it would otherwise hold its GPU target for good). Done on
    /// the next redraw: this may run while the window is out for drawing.
    fn release_editor_texture(&mut self) {
        if let View::Editor(ed) = &self.view
            && let Some(id) = ed.preview.texture_id
        {
            self.stale_textures.push(id);
        }
    }

    /// Before the editor goes away: if it is exporting, ask whether to cancel
    /// the export (scripted runs just cancel). False means stay where we are.
    fn confirm_leaving(&mut self, leaving: Leaving) -> bool {
        let running = matches!(&self.view, View::Editor(ed) if ed.export_running());
        match lifecycle::export_guard(running, self.scripted(), leaving) {
            ExportGuard::Proceed => return true,
            ExportGuard::ProceedCancelling => {}
            ExportGuard::Ask { prompt } => {
                let answer = rfd::MessageDialog::new().set_title("Glide").set_description(prompt).set_buttons(rfd::MessageButtons::YesNo).show();
                if answer != rfd::MessageDialogResult::Yes {
                    return false;
                }
            }
        }
        if let View::Editor(ed) = &mut self.view {
            ed.cancel_export();
        }
        true
    }
    /// Show `path` in the editor. Returns whether that project is now open.
    /// The running export is cancelled only once the new editor is ready.
    fn open_project(&mut self, el: &ActiveEventLoop, path: PathBuf, why: Leaving) -> bool {
        self.show_main(el);
        let Some(gpu) = self.gpu.clone() else { return false };
        let scripted = self.scripted();
        let (current, same) = match &mut self.view {
            View::Editor(ed) => {
                let same = lifecycle::same_project(&ed.package.root, &path);
                (Some(&mut **ed), same)
            }
            View::Start => (None, false),
        };
        let ask = |prompt| rfd::MessageDialog::new().set_title("Glide").set_description(prompt).set_buttons(rfd::MessageButtons::YesNo).show() == rfd::MessageDialogResult::Yes;
        match lifecycle::switch_to(current, same, scripted, why, || Editor::open(&path, gpu), ask) {
            Switch::Same => true,
            Switch::Opened(ed) => {
                if let Some(w) = &self.main {
                    w.window.set_title(&ed.project.name);
                }
                self.release_editor_texture();
                self.view = View::Editor(Box::new(ed));
                true
            }
            Switch::Declined => {
                if why == Leaving::OpenRecording
                    && let View::Editor(ed) = &mut self.view
                {
                    ed.status = Some("Your recording is saved. Open it from Recent once the export is done.".into());
                }
                false
            }
            Switch::Failed(e) => {
                self.error = Some(format!("Couldn't open {}: {e:#}", path.display()));
                false
            }
        }
    }

    /// (Re)register the global shortcuts from settings.
    fn bind_hotkeys(&mut self) {
        let hk = self.settings.hotkeys.clone();
        let mut failed = vec![];
        for (slot, combo, cmd) in [(1, hk.record, Cmd::ToggleRecording), (2, hk.pause, Cmd::TogglePause), (3, hk.cancel, Cmd::Cancel)] {
            match combo {
                Some(c) => {
                    if let Err(code) = hotkey::bind(slot, c, move || push(cmd.clone())) {
                        eprintln!("glide: could not register {} (error {code})", c.label());
                        failed.push(c.label());
                    }
                }
                None => hotkey::unbind(slot),
            }
        }
        if let Some(m) = &self.menubar {
            m.set_hotkey(&hk.record_label(), !failed.iter().any(|l| *l == hk.record_label()));
        }
        if !failed.is_empty() {
            self.error = Some(format!("Another app already uses {} — pick a different shortcut.", failed.join(", ")));
        }
    }

    fn recorder_options(&self) -> recorder::Options {
        let s = &self.settings;
        // GLIDE_SELFTEST_REAL records the main display (to check what ends up
        // in a real recording, e.g. that the control bar doesn't).
        if self.selftest.is_some() && std::env::var_os("GLIDE_SELFTEST_REAL").is_some() {
            return recorder::Options { target: Target::Display { id: 0 }, system_audio: false, mic: None, camera: None, fps: 60, keys: false, dir: s.dir(), name: Some("Selftest".into()), ..Default::default() };
        }
        if self.selftest.is_some() {
            return recorder::Options { target: Target::Pattern, system_audio: false, mic: None, camera: None, fps: 60, keys: false, dir: s.dir(), name: Some("Selftest".into()), ..Default::default() };
        }
        let target = match s.source {
            SourceKind::Display => Target::Display { id: s.display },
            SourceKind::Window => match s.window {
                Some(id) => Target::Window { id },
                None => Target::Display { id: s.display },
            },
            SourceKind::Area => Target::Area {
                display: s.display,
                rect: crate::engine::geom::Rect::new(s.area[0], s.area[1], s.area[2], s.area[3]),
            },
            SourceKind::Device => match &s.ios_device {
                Some(id) => Target::Device { id: id.clone() },
                None => Target::Display { id: s.display },
            },
        };
        recorder::Options {
            target,
            system_audio: s.system_audio,
            mic: s.mic.then(|| s.mic_id.clone()),
            camera: s.camera.then(|| s.camera_id.clone()),
            camera_style: Some(s.camera_style),
            fps: s.fps,
            keys: s.keys,
            dir: s.dir(),
            name: None,
            hide_desktop_icons: s.hide_desktop_icons,
            hide_dock: s.hide_dock,
            exclude_own_audio: true,
        }
    }

    fn start_recording(&mut self, _el: &ActiveEventLoop) {
        if self.recording.is_some() || self.finishing || self.restart.busy() {
            return;
        }
        self.error = None;
        if !self.scripted() {
            self.settings.save();
        }
        if let View::Editor(ed) = &mut self.view {
            ed.save();
            // The editor is hidden while recording: don't keep playing it.
            ed.pause();
        }
        let countdown = self.settings.countdown.then(|| Instant::now() + Duration::from_secs(3));
        self.recording = Some(Recording { rec: None, countdown_until: countdown });
        if let Some(w) = &self.main {
            w.window.set_visible(false);
        }
        if self.bar.is_none()
            && let Some(mtm) = objc2::MainThreadMarker::new()
        {
            let hk = &self.settings.hotkeys;
            let display = if matches!(self.settings.source, SourceKind::Display | SourceKind::Area) { self.settings.display } else { 0 };
            let pause = hk.pause.map(|c| c.label()).unwrap_or_default();
            self.bar = Some(recbar::RecordBar::new(mtm, display, self.bar_origin, &hk.record_label(), &pause));
        }
        if countdown.is_none() {
            self.begin_capture();
        }
    }

    fn begin_capture(&mut self) {
        let opts = self.recorder_options();
        // An iPhone's screen has one client at a time: let the preview's
        // session finish stopping before the recorder opens the device.
        let device = matches!(opts.target, Target::Device { .. });
        if device {
            self.live.screen.stop_and_join();
            self.live.tex = None;
        }
        match Recorder::start(opts) {
            Ok(r) => {
                if let Some(rec) = &mut self.recording {
                    rec.rec = Some(r);
                    rec.countdown_until = None;
                }
            }
            Err(e) => {
                if device {
                    crate::diag::note(&format!("glide: ios recording: could not start: {e:#}"));
                }
                self.recording = None;
                self.error = Some(format!("Couldn't start recording: {e:#}"));
                self.close_bar();
                self.permission_note = permission_note();
                push(Cmd::ShowWindow);
            }
        }
    }

    fn stop_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.close_bar();
        let Some(r) = rec.rec else {
            push(Cmd::ShowWindow);
            return;
        };
        self.finishing = true;
        let done = self.finished.clone();
        std::thread::spawn(move || {
            // Always report back, even if finalising panics, so the UI can't
            // stay on "Saving…" forever.
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| objc2::rc::autoreleasepool(|_| r.stop())))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("saving the recording crashed")))
                .map_err(|e| format!("{e:#}"));
            *done.lock().unwrap() = Some(res);
            wake();
        });
    }

    /// Discard the current recording (if any).
    fn cancel_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        if let Some(r) = rec.rec {
            Self::spawn_cancel(r, None);
        }
        self.close_bar();
        push(Cmd::ShowWindow);
    }

    /// Cancel a take off the main thread, then set `done` and wake the loop.
    fn spawn_cancel(r: Recorder, done: Option<Arc<AtomicBool>>) {
        std::thread::spawn(move || {
            // Report back even if the cancel panics, so a restart can't hang.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| objc2::rc::autoreleasepool(|_| r.cancel())));
            if let Some(done) = done {
                done.store(true, Ordering::SeqCst);
            }
            wake();
        });
    }

    /// Feed a restart event to the state machine and do what it says.
    fn restart_event(&mut self, el: &ActiveEventLoop, event: restart::Event) {
        let (next, action) = restart::step(self.restart, event);
        self.restart = next;
        match action {
            restart::Do::Nothing => {}
            restart::Do::CancelOldTake => match self.recording.take().and_then(|rec| rec.rec) {
                Some(r) => Self::spawn_cancel(r, Some(self.restart_done.clone())),
                // Still counting down: there is no recorder to cancel.
                None => self.restart_event(el, restart::Event::Cancelled),
            },
            restart::Do::StartNewTake => self.start_recording(el),
            restart::Do::CloseBar => {
                self.close_bar();
                push(Cmd::ShowWindow);
            }
        }
    }

    /// A save or a restart's cancel is still running.
    fn winding_down(&self) -> bool {
        (self.finishing && self.finished.lock().unwrap().is_none()) || (self.restart.busy() && !self.restart_done.load(Ordering::SeqCst))
    }

    /// Remove the recording bar, remembering where it was.
    fn close_bar(&mut self) {
        if let Some(b) = self.bar.take() {
            self.bar_origin = Some(b.origin());
        }
    }

    fn handle(&mut self, el: &ActiveEventLoop, c: Cmd) {
        match c {
            Cmd::ToggleRecording => {
                if self.recording.is_some() {
                    self.stop_recording();
                } else {
                    self.start_recording(el);
                }
            }
            Cmd::TogglePause => {
                if let Some(Recording { rec: Some(r), .. }) = &mut self.recording {
                    if r.is_paused() { r.resume() } else { r.pause() }
                }
            }
            Cmd::Restart => self.restart_event(el, restart::Event::Requested { recording: self.recording.is_some() }),
            Cmd::Cancel => {
                if self.restart.busy() {
                    self.restart_event(el, restart::Event::Aborted);
                } else {
                    self.cancel_recording();
                }
            }
            Cmd::ShowWindow => self.show_main(el),
            Cmd::OpenFolder => macos::open_folder(&self.settings.dir()),
            Cmd::StartUpdater => updater::start(),
            Cmd::CheckForUpdates => updater::check_for_updates(),
            Cmd::Back => self.close_project(),
            Cmd::Open(p) => {
                self.open_project(el, p, Leaving::OpenProject);
            }
            Cmd::RecordSource(kind) => {
                if self.recording.is_some() || self.restart.busy() {
                    return;
                }
                self.settings.source = kind;
                match kind {
                    SourceKind::Area => push(Cmd::SelectArea(true)),
                    // Pick the window on screen first.
                    SourceKind::Window => push(Cmd::SelectWindow(true)),
                    _ => push(Cmd::ToggleRecording),
                }
            }
            Cmd::SelectArea(record_after) => {
                if self.recording.is_some() || self.restart.busy() {
                    return;
                }
                // Pick on the display under the pointer.
                let display = macos::display_under_pointer();
                let (id, _, _, dw, dh) = macos::display_frame(display);
                let s = &self.settings;
                // Start from the saved area when it was on this display,
                // trimmed to it.
                let same_display = macos::display_frame(s.display).0 == id;
                let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(dw as f32, dh as f32));
                let rect = (same_display && s.source == SourceKind::Area && s.area[2] > 8.0 && s.area[3] > 8.0)
                    .then(|| egui::Rect::from_min_size(egui::pos2(s.area[0] as f32, s.area[1] as f32), vec2(s.area[2] as f32, s.area[3] as f32)).intersect(screen))
                    .filter(|r| r.width() > 8.0 && r.height() > 8.0);
                self.area = AreaPick { rect, drag: None, record_after, display: id, closed: false, windows: None };
                self.open_overlays(el, id);
            }
            Cmd::SelectWindow(record_after) => {
                if self.recording.is_some() || self.restart.busy() {
                    return;
                }
                // Pick on the display under the pointer.
                let display = macos::display_under_pointer();
                let (id, ..) = macos::display_frame(display);
                let windows = Some(WindowMode { list: winpick::list_all(), ..Default::default() });
                self.area = AreaPick { rect: None, drag: None, record_after, display: id, closed: false, windows };
                self.open_overlays(el, id);
            }
            Cmd::Quit => {
                if !self.confirm_leaving(Leaving::Quit) {
                    return;
                }
                if let View::Editor(ed) = &mut self.view {
                    ed.save();
                }
                if self.recording.is_some() {
                    self.stop_recording();
                }
                let deadline = Instant::now() + Duration::from_secs(60);
                while self.winding_down() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                el.exit();
            }
        }
    }

    fn tick(&mut self, el: &ActiveEventLoop) {
        let cmds: Vec<Cmd> = std::mem::take(&mut *QUEUE.lock().unwrap());
        for c in cmds {
            self.handle(el, c);
        }
        self.update_live();
        if self.restart.busy() && self.restart_done.swap(false, Ordering::SeqCst) {
            self.restart_event(el, restart::Event::Cancelled);
        }
        if let Some(rec) = &self.recording
            && rec.rec.is_none()
            && rec.countdown_until.is_some_and(|t| Instant::now() >= t)
        {
            self.begin_capture();
        }
        if let Some(Recording { rec: Some(r), .. }) = &self.recording
            && let Some(e) = r.error()
        {
            self.error = Some(format!("Recording stopped: {e}"));
            self.stop_recording();
        }
        let done = self.finished.lock().unwrap().take();
        if let Some(res) = done {
            self.finishing = false;
            match res {
                Ok(path) => {
                    self.projects = home::scan_projects(&self.settings.dir());
                    if self.open_project(el, path, Leaving::OpenRecording)
                        && let View::Editor(ed) = &mut self.view
                    {
                        ed.fresh = true;
                    }
                }
                Err(e) => {
                    self.error = Some(format!("Recording failed: {e}"));
                    self.show_main(el);
                }
            }
        }
        self.selftest_step(el);
        self.playtest_step();
        let clock = match &self.recording {
            Some(Recording { rec: Some(r), .. }) => crate::capture::record::clock(r.elapsed()),
            Some(Recording { countdown_until: Some(t), .. }) => format!("{}", t.saturating_duration_since(Instant::now()).as_secs() + 1),
            _ => String::new(),
        };
        let paused = matches!(&self.recording, Some(Recording { rec: Some(r), .. }) if r.is_paused());
        if let Some(b) = &mut self.bar {
            b.update(match &self.recording {
                _ if self.restart.busy() => recbar::State::Restarting,
                Some(Recording { rec: None, countdown_until: Some(t) }) => recbar::State::Countdown(t.saturating_duration_since(Instant::now()).as_secs() + 1),
                _ => recbar::State::Recording { clock: clock.clone(), paused },
            });
        }
        if let Some(m) = &mut self.menubar {
            m.update(&menubar::View { recording: self.recording.is_some(), paused, finishing: self.finishing, clock: &clock });
        }
    }

    /// Driven by GLIDE_GUI_SELFTEST or GLIDE_GUI_PLAYTEST: keep the user's
    /// settings untouched and stay in the background.
    fn scripted(&self) -> bool {
        self.selftest.is_some() || self.playtest.is_some()
    }

    fn playtest_step(&mut self) {
        let Some(pt) = &mut self.playtest else { return };
        let since = pt.at.elapsed();
        let fail = |msg: String| {
            plog(&format!("FAIL {msg}"));
            std::process::exit(1);
        };
        if pt.started.elapsed() > Duration::from_secs(90) {
            fail(format!("timed out in phase {}", pt.phase));
        }
        if pt.phase == 0 {
            if since.as_secs_f64() > pt.delay {
                push(Cmd::Open(pt.path.clone()));
                pt.phase = 1;
                pt.at = Instant::now();
            }
            return;
        }
        let View::Editor(ed) = &mut self.view else {
            if since > Duration::from_secs(10) {
                fail(format!("couldn't open {}: {:?}", pt.path.display(), self.error));
            }
            return;
        };
        let settled = !ed.audio.mixing();
        // Edit the project the way the Audio tab does.
        let edit = |ed: &mut Editor, f: &dyn Fn(&mut crate::project::model::AudioStyle)| {
            let mut d = ed.project.clone();
            f(&mut d.style.audio);
            ed.commit(d);
            ed.audio.reset_meter();
        };
        match pt.phase {
            // Mixed: play from the start, looping so the checks below have
            // something to hear.
            1 if settled => {
                if !ed.audio.has_audio() {
                    fail("the project has nothing to hear".into());
                }
                pt.volume = ed.project.style.audio.volume;
                ed.looping = true;
                ed.playhead = 0.0;
                ed.toggle_play();
                ed.audio.reset_meter();
                plog(&format!("playing {} ({:.1} s)", pt.path.display(), ed.duration() / 1000.0));
                pt.phase = 2;
                pt.at = Instant::now();
            }
            2 => {
                if since > Duration::from_millis(500) {
                    pt.max_drift = pt.max_drift.max((ed.audio.position_ms() - ed.playhead_now()).abs());
                }
                if since.as_secs_f64() >= pt.secs {
                    let m = ed.audio.meter();
                    pt.loud_db = m.rms_db();
                    plog(&format!("output frames={} rms={:.1} dBFS peak={:.3} max_drift={:.0} ms", m.frames, m.rms_db(), m.peak, pt.max_drift));
                    if (m.frames as f64) < pt.secs * 0.7 * 44_100.0 {
                        fail(format!("the output only rendered {} frames in {:.1} s", m.frames, pt.secs));
                    }
                    if pt.loud_db < -45.0 {
                        fail(format!("playback is silent ({:.1} dBFS)", pt.loud_db));
                    }
                    if pt.max_drift > 80.0 {
                        fail(format!("audio drifted {:.0} ms from the playhead", pt.max_drift));
                    }
                    let v = pt.volume;
                    edit(ed, &|a| a.volume = v * 0.25);
                    pt.phase = 3;
                    pt.at = Instant::now();
                }
            }
            // A quarter of the volume is 12 dB down, straight away. (Measured
            // against the soundtrack, which may be quiet for a while.)
            3 if since > Duration::from_millis(700) => {
                let m = ed.audio.meter();
                let Some(gain) = m.gain_db() else {
                    if since > Duration::from_secs(4) {
                        fail("the soundtrack stayed silent".into());
                    }
                    return;
                };
                let want = 20.0 * (0.25 * pt.volume).log10() as f32;
                plog(&format!("volume x0.25 rms={:.1} dBFS, {gain:.1} dB relative to the soundtrack (want {want:.1})", m.rms_db()));
                if (gain - want).abs() > 2.0 {
                    fail("the volume didn't change what's heard".into());
                }
                let v = pt.volume;
                edit(ed, &|a| {
                    a.volume = v;
                    a.mute_system = true;
                    a.mute_mic = true;
                });
                pt.phase = 4;
                pt.at = Instant::now();
            }
            4 if settled && since > Duration::from_millis(200) => {
                ed.audio.reset_meter();
                pt.phase = 5;
                pt.at = Instant::now();
            }
            5 if since > Duration::from_millis(600) => {
                let m = ed.audio.meter();
                plog(&format!("muted rms={:.1} dBFS frames={}", m.rms_db(), m.frames));
                if m.rms_db() > -70.0 || m.frames == 0 {
                    fail("mute didn't silence playback".into());
                }
                edit(ed, &|a| {
                    a.mute_system = false;
                    a.mute_mic = false;
                });
                pt.phase = 6;
                pt.at = Instant::now();
            }
            6 if settled && since > Duration::from_millis(200) => {
                ed.audio.reset_meter();
                pt.phase = 7;
                pt.at = Instant::now();
            }
            7 if since > Duration::from_millis(600) => {
                let m = ed.audio.meter();
                let Some(gain) = m.gain_db() else {
                    if since > Duration::from_secs(4) {
                        fail("unmuting didn't bring the sound back".into());
                    }
                    return;
                };
                plog(&format!("unmuted rms={:.1} dBFS, {gain:.1} dB relative to the soundtrack", m.rms_db()));
                if m.rms_db() < -70.0 {
                    fail("unmuting didn't bring the sound back".into());
                }
                let to = ed.duration() * 0.5;
                ed.seek(to);
                pt.phase = 8;
                pt.at = Instant::now();
            }
            8 if since > Duration::from_millis(400) => {
                let now = ed.playhead_now();
                let drift = ed.audio.position_ms() - now;
                plog(&format!("after seek audio={:.0} ms playhead={now:.0} ms", ed.audio.position_ms()));
                if drift.abs() > 80.0 {
                    fail(format!("audio is {drift:.0} ms off the playhead after a seek"));
                }
                ed.toggle_play();
                ed.looping = false;
                plog("PASS");
                pt.phase = 9;
                push(Cmd::Quit);
            }
            _ => {}
        }
    }

    fn selftest_step(&mut self, el: &ActiveEventLoop) {
        let Some(st) = &mut self.selftest else { return };
        let since = st.at.elapsed();
        let fail = |msg: String| {
            log(&format!("FAIL {msg}"));
            std::process::exit(1);
        };
        if since > Duration::from_secs(120) {
            fail(format!("timed out in phase {}", st.phase));
        }
        match st.phase {
            0 if since > Duration::from_millis(800) => {
                let start = matches!(self.view, View::Start);
                // The display's live preview fills the canvas (when Screen
                // Recording is allowed); give it a few seconds to arrive.
                self.settings.source = SourceKind::Display;
                let screen = crate::capture::tcc::screen_status() == crate::capture::tcc::Status::Granted;
                if screen && self.live.tex.is_none() && since < Duration::from_secs(6) {
                    return;
                }
                let live = self.live.tex.map(|t| t.1);
                log(&format!("launched main_window={} menubar={} start_view={start} live_preview={live:?}", self.main.is_some(), self.menubar.is_some()));
                if !start {
                    fail("didn't open on recording setup".into());
                }
                if screen && live.is_none() {
                    fail("no live preview frames".into());
                }
                st.phase = 10;
                st.at = Instant::now();
                push(Cmd::SelectArea(false));
            }
            10 if since > Duration::from_millis(600) => {
                let Some(w) = self.overlays.iter().find(|w| w.display == self.area.display) else {
                    fail("area picker did not open".into());
                    return;
                };
                let size = w.window.inner_size();
                log(&format!("area picker open {}x{} px, {} displays covered", size.width, size.height, self.overlays.len()));
                if let (Some(dir), Some(num)) = (std::env::var_os("GLIDE_SELFTEST_SHOTS"), macos::window_number(&w.window)) {
                    let out = PathBuf::from(dir).join("area-picker.png");
                    let ok = std::process::Command::new("screencapture").args(["-x", "-o", "-l", &num.to_string()]).arg(&out).status().is_ok_and(|s| s.success());
                    log(&format!("area picker screenshot {} ok={ok}", out.display()));
                }
                st.phase = 11;
                st.at = Instant::now();
                self.finish_area(Some(egui::Rect::from_min_size(egui::pos2(200.0, 150.0), vec2(640.0, 400.0))));
            }
            11 if since > Duration::from_millis(300) => {
                let a = self.settings.area;
                log(&format!("area saved source={:?} area={:?} overlay_closed={}", self.settings.source, a, self.overlays.is_empty()));
                if self.settings.source != SourceKind::Area || a != [200.0, 150.0, 640.0, 400.0] || !self.overlays.is_empty() {
                    fail("area picker result".into());
                }
                st.phase = 12;
                st.at = Instant::now();
                push(Cmd::SelectWindow(false));
            }
            12 if since > Duration::from_millis(800) => {
                let (Some(w), Some(wm)) = (self.overlays.first(), &mut self.area.windows) else {
                    fail("window picker did not open".into());
                    return;
                };
                // Highlight the frontmost window first (as a click would), then
                // check again after it has been drawn.
                if wm.selected.is_none() && !wm.list.is_empty() {
                    wm.selected = wm.list.iter().find(|x| x.display == w.display).or(wm.list.first()).map(|x| x.entry.id);
                    for o in &self.overlays {
                        o.window.request_redraw();
                    }
                    st.at = Instant::now();
                    return;
                }
                log(&format!("window picker open on {} displays with {} windows, selected {:?}", self.overlays.len(), wm.list.len(), wm.selected));
                if let Some(dir) = std::env::var_os("GLIDE_SELFTEST_SHOTS") {
                    for o in &self.overlays {
                        let Some(num) = macos::window_number(&o.window) else { continue };
                        let out = PathBuf::from(&dir).join(format!("window-picker-{}.png", o.display));
                        let ok = std::process::Command::new("screencapture").args(["-x", "-o", "-l", &num.to_string()]).arg(&out).status().is_ok_and(|s| s.success());
                        log(&format!("window picker screenshot {} ok={ok}", out.display()));
                    }
                }
                st.phase = 13;
                st.at = Instant::now();
                self.finish_window(None);
            }
            13 if since > Duration::from_millis(300) => {
                log(&format!("window picker cancelled overlay_closed={} source={:?}", self.overlays.is_empty(), self.settings.source));
                if !self.overlays.is_empty() || self.settings.source != SourceKind::Area {
                    fail("window picker cancel".into());
                }
                // A real recording is of the main display; put the bar there too.
                if std::env::var_os("GLIDE_SELFTEST_REAL").is_some() {
                    self.settings.source = SourceKind::Display;
                    self.settings.display = 0;
                }
                st.phase = 1;
                st.at = Instant::now();
                push(Cmd::ToggleRecording);
            }
            1 => {
                if let Some(Recording { rec: Some(r), .. }) = &self.recording {
                    match st.paused_at {
                        None if r.elapsed() >= 1.0 => {
                            st.paused_at = Some(Instant::now());
                            push(Cmd::TogglePause);
                        }
                        Some(t) if !st.resumed && t.elapsed() > Duration::from_millis(700) => {
                            log(&format!("paused={}", r.is_paused()));
                            if !r.is_paused() {
                                fail("pause had no effect".into());
                            }
                            st.resumed = true;
                            push(Cmd::TogglePause);
                        }
                        _ => {}
                    }
                    if st.resumed && r.elapsed() >= 3.0 {
                        let live = self.live.screen.active() || self.live.camera.active() || self.live.mic.active();
                        let bar = self.bar.as_ref().map(|b| (b.window_number(), b.draggable()));
                        let at = self.bar.as_ref().map(|b| (b.origin().x.round(), b.origin().y.round()));
                        log(&format!("recording bar={bar:?} origin={at:?} elapsed={:.1}s live_preview_running={live}", r.elapsed()));
                        if let (Some(dir), Some((num, _))) = (std::env::var_os("GLIDE_SELFTEST_SHOTS"), bar) {
                            let out = PathBuf::from(dir).join("record-bar.png");
                            let ok = std::process::Command::new("screencapture").args(["-x", "-l", &num.to_string()]).arg(&out).status().is_ok_and(|s| s.success());
                            log(&format!("recording bar screenshot {} ok={ok}", out.display()));
                        }
                        if bar.is_none_or(|(_, draggable)| !draggable) {
                            fail("recording bar missing or not draggable".into());
                        }
                        if live {
                            fail("live preview kept running while recording".into());
                        }
                        st.phase = 2;
                        push(Cmd::ToggleRecording);
                    }
                }
                if let Some(e) = &self.error {
                    fail(e.clone());
                }
            }
            2 => {
                if let Some(e) = &self.error {
                    fail(e.clone());
                }
                if let View::Editor(ed) = &mut self.view {
                    log(&format!(
                        "editor opened {} duration={:.0}ms zooms={} mouse_events={}",
                        ed.package.root.display(),
                        ed.duration(),
                        ed.project.timeline.zooms.len(),
                        ed.rec.mouse.len()
                    ));
                    st.package = Some(ed.package.root.clone());
                    ed.start_export();
                    st.phase = 3;
                }
            }
            3 => {
                if let View::Editor(ed) = &self.view
                    && let Some(res) = &ed.export.result
                {
                    match res {
                        Ok((path, stats)) => {
                            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                            log(&format!("exported {} bytes={size} {stats:?}", path.display()));
                            if size == 0 {
                                fail("empty export".into());
                            }
                            st.phase = 4;
                            st.at = Instant::now();
                            push(Cmd::Back);
                        }
                        Err(e) => fail(format!("export: {e}")),
                    }
                }
            }
            // Back to recording setup; the new recording heads the recents
            // popover; reopen it from there.
            4 if since > Duration::from_millis(300) => {
                let first = self.projects.first().map(|p| p.path.clone());
                log(&format!("back start_view={} recent={:?}", matches!(self.view, View::Start), first.as_ref().map(|p| p.display().to_string())));
                if !matches!(self.view, View::Start) || first.is_none() || first != st.package {
                    fail("back to recording setup / recents".into());
                }
                st.phase = 5;
                st.at = Instant::now();
                push(Cmd::Open(first.unwrap()));
            }
            5 if since > Duration::from_millis(300) => {
                let reopened = matches!(&self.view, View::Editor(ed) if Some(&ed.package.root) == st.package.as_ref());
                log(&format!("reopened from recents={reopened}"));
                if !reopened {
                    fail("reopen from recents".into());
                }
                log("PASS");
                st.phase = 6;
                push(Cmd::Quit);
            }
            _ => {}
        }
        let _ = el;
    }

    fn draw_main(&mut self, ui: &mut egui::Ui, renderer: &mut egui_wgpu::Renderer) {
        if let Some(e) = self.error.clone() {
            egui::Panel::bottom("error").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 69, 58), &e);
                    if ui.small_button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            });
        }
        if self.finishing {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Saving recording…").size(16.0));
                });
            });
            return;
        }
        let was = home::CAPTURING_SHORTCUT.swap(false, Ordering::Relaxed);
        let note = self.permission_note.clone();
        let action = match &mut self.view {
            View::Start => {
                let live = self.live_view(ui.ctx(), renderer);
                home::start_view(ui, &mut self.settings, &mut self.devices, &self.projects, &mut self.thumbs, note.as_deref(), &live)
            }
            View::Editor(ed) => {
                let mut chrome = home::Chrome {
                    settings: &mut self.settings,
                    devices: &mut self.devices,
                    projects: &self.projects,
                    thumbs: &mut self.thumbs,
                    action: home::Action::None,
                };
                ed.ui(ui, renderer, &mut chrome);
                chrome.action
            }
        };
        self.handle_action(action);
        let now = home::CAPTURING_SHORTCUT.load(Ordering::Relaxed);
        if now && !was {
            for slot in 1..=3 {
                hotkey::unbind(slot);
            }
        } else if was && !now {
            self.bind_hotkeys();
        }
    }

    /// Act on a button in the start view or the editor's title bar.
    fn handle_action(&mut self, action: home::Action) {
        match action {
            // Area recordings confirm the region first.
            home::Action::Record if self.settings.source == SourceKind::Area => push(Cmd::SelectArea(true)),
            home::Action::Record if self.settings.source == SourceKind::Window && self.settings.window.is_none() => {
                push(Cmd::SelectWindow(true))
            }
            home::Action::SelectWindow => push(Cmd::SelectWindow(false)),
            home::Action::Record => push(Cmd::ToggleRecording),
            home::Action::SelectArea => push(Cmd::SelectArea(false)),
            home::Action::HotkeysChanged => {
                self.settings.save();
                self.bind_hotkeys();
            }
            home::Action::SaveSettings => {
                if !self.scripted() {
                    self.settings.save();
                }
            }
            home::Action::Open(p) => push(Cmd::Open(p)),
            home::Action::RequestPermissions => {
                std::thread::spawn(|| crate::capture::tcc::request_all(true, false, false, Duration::from_secs(60)));
            }
            home::Action::ResizeWindow { id, width, height } => {
                let note = match crate::capture::winsize::resize(id, width as f64, height as f64) {
                    Ok((w, h)) if (w.round() as u32, h.round() as u32) != (width, height) => {
                        Some(format!("The app limited the window to {w:.0} × {h:.0}"))
                    }
                    Ok(_) => None,
                    Err(e) => Some(format!("{e:#}")),
                };
                home::set_resize_note(note);
                self.devices = home::Devices::load();
            }
            home::Action::RefreshIos => {
                self.devices.ios = crate::capture::camera::ios_devices().into_iter().map(|d| home::Device { id: d.id, name: d.name }).collect();
                self.live.screen.reset_failed();
            }
            home::Action::Back => self.close_project(),
            home::Action::None => {}
        }
    }

    /// Whether recording setup is on screen with nothing being recorded
    /// (when its previews and meters run).
    fn setup_visible(&self) -> bool {
        let visible = self.main.as_ref().is_some_and(|w| w.window.is_visible() != Some(false)) && !self.main_occluded;
        matches!(self.view, View::Start) && visible && self.recording.is_none() && !self.finishing
    }

    /// The source to preview live, if the recording-setup canvas is showing
    /// and nothing is being recorded.
    fn live_source(&self) -> Option<livecap::Source> {
        if !self.setup_visible() {
            return None;
        }
        let s = &self.settings;
        if s.source == SourceKind::Device {
            return home::listed_ios_id(s.ios_device.as_deref(), &self.devices.ios).map(|id| livecap::Source::Device { id });
        }
        if crate::capture::tcc::screen_status() != crate::capture::tcc::Status::Granted {
            return None;
        }
        match s.source {
            SourceKind::Display => Some(livecap::Source::Display { id: s.display }),
            SourceKind::Area if s.area[2] >= 8.0 && s.area[3] >= 8.0 => Some(livecap::Source::Area { display: s.display, rect: s.area }),
            SourceKind::Window => s.window.map(|id| livecap::Source::Window { id }),
            _ => None,
        }
    }

    /// Start, switch or stop the live previews and meters to match what's on
    /// screen. Streams start and stop on background threads.
    fn update_live(&mut self) {
        if self.settings.resolve_window(&self.devices) && !self.scripted() {
            self.settings.save();
        }
        let visible = self.setup_visible();
        // A phone plugged in (or out) after launch changes the list; a
        // failed iPhone preview is tried again every few seconds, since the
        // phone may only have been locked or slow to wake.
        let watch_ios = visible && self.settings.source == SourceKind::Device;
        ioswatch::set_wanted(watch_ios);
        if let Some(list) = ioswatch::take()
            && ioswatch::differs(&list, &self.devices.ios)
        {
            self.devices.ios = list;
            self.live.screen.reset_failed();
        }
        if watch_ios {
            self.live.screen.retry_failed(Instant::now(), IOS_RETRY);
        }
        let s = &self.settings;
        let screen = self.live_source().map(|src| {
            let audio = s.system_audio && !matches!(src, livecap::Source::Device { .. });
            (src, audio)
        });
        let camera = (visible && s.camera).then(|| livecap::Source::Camera { id: s.camera_id.clone() });
        let mic = (visible && s.mic && self.selftest.is_none()).then(|| s.mic_id.as_ref().and_then(|id| self.devices.mics.iter().find(|m| &m.id == id)).map(|m| m.name.clone()));
        let live = &mut self.live;
        // An iPhone preview that reports an error or a disconnect is stopped
        // and shows the text; the retry brings it back.
        if let Some(msg) = live.screen.running.as_ref().and_then(|(_, p)| p.take_runtime_error())
            && live.screen.fail_running("live preview", msg)
        {
            live.tex = None;
        }
        // Screens at native resolution, scaled to the canvas (30 fps, as Rec);
        // in light mode small and at 15 fps (5K frames swamp older Macs).
        if live.screen.update(screen, "live preview", |(src, audio)| {
            let (max_px, fps) = if crate::hw::light() { (crate::hw::LIGHT_PREVIEW_EDGE, 15) } else { (0, 30) };
            livecap::LivePreview::start(src, max_px, fps, *audio, wake).map_err(|e| format!("{e:#}"))
        }) {
            live.tex = None;
        }
        if live.camera.update(camera, "camera preview", |src| livecap::LivePreview::start(src, 720, 30, false, wake).map_err(|e| format!("{e:#}"))) {
            live.cam_tex = None;
        }
        live.mic.update(mic, "mic meter", |name| crate::capture::meter::MicMeter::start(name.as_deref()).map_err(|e| format!("{e:#}")));
    }

    /// Newest preview frames into their textures, levels into the meters,
    /// and what the start view should show.
    fn live_view(&mut self, ctx: &egui::Context, renderer: &mut egui_wgpu::Renderer) -> home::LiveView {
        let live = &mut self.live;
        if let (Some((_, p)), Some(gpu)) = (&live.screen.running, &self.gpu) {
            // Scaled to the canvas the start view last laid out, in pixels.
            let [w, h] = home::canvas_px();
            p.set_target(w, h);
            if live.interop.is_none() {
                live.interop = crate::render::interop::TextureCache::new(&gpu.device);
            }
            if let (Some(pb), Some(cache)) = (p.take_buffer(), &live.interop)
                && let Some(f) = cache.wrap(&pb, wgpu::TextureUsages::TEXTURE_BINDING)
            {
                // Drawn pixel-aligned at its own size: nearest is exact, and
                // avoids a blur while the canvas is mid-resize (as Rec). Light
                // mode's small frames are enlarged, so smoothly.
                let filter = if crate::hw::light() { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
                let id = match live.tex_id {
                    Some(id) => {
                        renderer.update_egui_texture_from_wgpu_texture(&gpu.device, &f.view, filter, id);
                        id
                    }
                    None => renderer.register_native_texture(&gpu.device, &f.view, filter),
                };
                live.tex_id = Some(id);
                live.tex = Some((id, [f.size.0, f.size.1]));
                live.frames.push_back(f);
                while live.frames.len() > 3 {
                    live.frames.pop_front();
                }
                cache.flush();
            }
        }
        let upload = |slot: &mut Option<egui::TextureHandle>, p: &livecap::LivePreview, name: &str| {
            if let Some(f) = p.take_frame() {
                let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.rgba);
                match slot {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => *slot = Some(ctx.load_texture(name, img, egui::TextureOptions::LINEAR)),
                }
            }
        };
        if let Some((_, p)) = &live.camera.running {
            upload(&mut live.cam_tex, p, "camera-preview");
        }
        // Peak hold falling ~20 dB a second, however often this runs.
        let sys = live.screen.running.as_ref().map(|(_, p)| p.take_level());
        let mic = live.mic.running.as_ref().map(|(_, m)| m.take_level());
        let now = Instant::now();
        let dt = live.levels_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(1.0);
        live.levels_at = Some(now);
        let decay = 0.97f32.powf(dt * 60.0);
        for (shown, new) in live.levels.iter_mut().zip([sys, mic]) {
            *shown = new.map_or(0.0, |n| n.max(*shown * decay));
        }
        // Redraw for the meters only while they show something (-40 dB),
        // and at 10 Hz in light mode.
        if live.levels.iter().any(|l| *l > 0.01) {
            ctx.request_repaint_after(Duration::from_millis(if crate::hw::light() { 100 } else { 50 }));
        }
        let s = &self.settings;
        let ppp = ctx.pixels_per_point();
        let screen = live.tex.filter(|_| live.screen.running.is_some()).map(|(id, [w, h])| {
            let size = egui::vec2(w as f32, h as f32) / ppp;
            if !crate::hw::light() {
                return (id, size);
            }
            // Light mode's frames are smaller than the canvas: shown filling it.
            let [cw, ch] = home::canvas_px();
            let up = (cw as f32 / w as f32).min(ch as f32 / h as f32).max(1.0);
            (id, size * up)
        });
        let note = if screen.is_some() {
            None
        } else if let Some(e) = live.screen.error() {
            Some(format!("Preview unavailable: {e}"))
        } else {
            match s.source {
                SourceKind::Window if s.window.is_none() => Some("Choose a window to see it here.".into()),
                SourceKind::Device if self.devices.ios.is_empty() => Some(home::IOS_CONNECT_HINT.into()),
                SourceKind::Device if s.ios_device.is_none() => Some("Connect an iPhone or iPad to see it here.".into()),
                SourceKind::Device => None,
                _ if crate::capture::tcc::screen_status() != crate::capture::tcc::Status::Granted => {
                    Some("Allow Screen Recording for Glide to see a live preview.".into())
                }
                _ => None,
            }
        };
        home::LiveView {
            screen,
            camera: live.cam_tex.as_ref().filter(|_| live.camera.running.is_some()).map(|t| (t.id(), t.size_vec2())),
            // Inputs that are on show a (possibly empty) meter.
            system_level: Some(live.levels[0]),
            mic_level: Some(live.levels[1]),
            note,
            covered: covered_note(s),
        }
    }

    /// Close the open recording (it saves) and show recording setup.
    fn close_project(&mut self) {
        if !self.confirm_leaving(Leaving::CloseProject) {
            return;
        }
        if let View::Editor(ed) = &mut self.view {
            ed.save();
            // Its thumbnail re-renders if the project changed.
            self.thumbs.forget(&ed.package.root);
        }
        self.release_editor_texture();
        self.view = View::Start;
        self.projects = home::scan_projects(&self.settings.dir());
        if let Some(w) = &self.main {
            w.window.set_title("Glide");
        }
    }

    /// Area picker: dim the display, drag a rect, confirm with Return.
    /// Picker overlay on `display`: the area picker, or the window picker.
    fn draw_overlay(&mut self, ui: &mut egui::Ui, display: u32) {
        use egui::{Align2, FontId, Pos2, Rect, Stroke, pos2};
        if self.area.windows.is_some() {
            self.draw_window_picker(ui, display);
            return;
        }
        let full = ui.max_rect();
        let resp = ui.interact(full, ui.id().with("area"), Sense::click_and_drag());
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        // The area lives on one display; dragging on another moves it there.
        if resp.drag_started() && self.area.display != display {
            self.area.display = display;
            self.area.rect = None;
            for w in &self.overlays {
                w.window.request_redraw();
            }
        }
        if self.area.display != display {
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha(120));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            let hint = "Drag to select an area  ·  Esc to cancel";
            let galley = ui.painter().layout_no_wrap(hint.into(), FontId::proportional(14.0), Color32::WHITE);
            let bg = Rect::from_center_size(pos2(full.center().x, full.top() + 60.0), galley.size() + vec2(28.0, 14.0));
            ui.painter().rect_filled(bg, egui::CornerRadius::same(10), Color32::from_black_alpha(170));
            ui.painter().galley(bg.center() - galley.size() / 2.0, galley, Color32::WHITE);
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.finish_area(None);
            }
            return;
        }
        let a = &mut self.area;
        let corners = |r: Rect| [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()];
        if resp.drag_started()
            && let Some(p) = pointer
        {
            a.drag = Some(match a.rect {
                Some(r) => match corners(r).into_iter().find(|c| c.distance(p) < 12.0) {
                    // Resize from the opposite (anchored) corner.
                    Some(c) => AreaDrag::Corner(pos2(r.left() + r.right() - c.x, r.top() + r.bottom() - c.y)),
                    None if r.contains(p) => AreaDrag::Move(p - r.min),
                    None => AreaDrag::New(p),
                },
                None => AreaDrag::New(p),
            });
        }
        if resp.dragged()
            && let (Some(d), Some(p)) = (a.drag, pointer)
        {
            let p = p.clamp(full.min, full.max);
            a.rect = Some(match d {
                AreaDrag::New(start) | AreaDrag::Corner(start) => Rect::from_two_pos(start, p),
                AreaDrag::Move(off) => {
                    let r = a.rect.unwrap_or(Rect::NOTHING);
                    // A rect larger than the display can't move; keep it inside.
                    let hi = (full.max - r.size()).max(full.min);
                    let min = (p - off).clamp(full.min, hi);
                    Rect::from_min_size(min, r.size())
                }
            });
        }
        if resp.drag_stopped() {
            a.drag = None;
            if a.rect.is_some_and(|r| r.width() < 16.0 || r.height() < 16.0) {
                a.rect = None;
            }
        }

        // Dim everything outside the selection.
        let painter = ui.painter();
        let dim = Color32::from_black_alpha(120);
        match a.rect {
            Some(r) => {
                painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, r.top())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.bottom()), full.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.top()), pos2(r.left(), r.bottom())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(r.right(), r.top()), pos2(full.max.x, r.bottom())), 0.0, dim);
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Outside);
                for c in corners(r) {
                    painter.circle(c, 5.0, Color32::WHITE, Stroke::new(1.0, theme::accent()));
                }
                let label = format!("{:.0} × {:.0}", r.width(), r.height());
                let at = if r.top() > 30.0 { r.center_top() - vec2(0.0, 10.0) } else { r.center_top() + vec2(0.0, 18.0) };
                painter.text(at, Align2::CENTER_BOTTOM, label, FontId::proportional(13.0), Color32::WHITE);
                let hover = pointer.map(|p| (corners(r).iter().any(|c| c.distance(p) < 12.0), r.contains(p)));
                ui.ctx().set_cursor_icon(match hover {
                    Some((true, _)) => egui::CursorIcon::ResizeNwSe,
                    Some((false, true)) => egui::CursorIcon::Move,
                    _ => egui::CursorIcon::Crosshair,
                });
            }
            None => {
                painter.rect_filled(full, 0.0, dim);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
        let hint = "Drag to select an area  ·  Return to confirm  ·  Esc to cancel";
        let hint_pos = pos2(full.center().x, full.top() + 60.0);
        let galley = painter.layout_no_wrap(hint.into(), FontId::proportional(14.0), Color32::WHITE);
        let bg = Rect::from_center_size(hint_pos, galley.size() + vec2(28.0, 14.0));
        painter.rect_filled(bg, egui::CornerRadius::same(10), Color32::from_black_alpha(170));
        painter.galley(bg.center() - galley.size() / 2.0, galley, Color32::WHITE);

        // Buttons under the selection.
        let mut confirm = ui.input(|i| i.key_pressed(egui::Key::Enter)) && a.rect.is_some();
        let mut cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(r) = a.rect
            && a.drag.is_none()
        {
            let y = if r.bottom() + 52.0 < full.bottom() { r.bottom() + 12.0 } else { r.bottom() - 44.0 };
            let bar = Rect::from_min_size(Pos2::new(r.right() - 220.0, y), vec2(220.0, 32.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                let primary = if a.record_after { "Record" } else { "Use area" };
                if widgets::primary(ui, primary).clicked() {
                    confirm = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        }
        let outcome = if confirm { a.rect.map(|r| r.translate(-full.min.to_vec2())) } else { None };
        if confirm || cancel {
            self.finish_area(outcome);
        }
    }

    /// Window picker: the windows on this display are outlined; the one under
    /// the pointer (or the clicked one) is highlighted with its name, size,
    /// Resize menu and buttons to use it or adjust an area from it.
    fn draw_window_picker(&mut self, ui: &mut egui::Ui, display: u32) {
        use egui::{Align2, FontId, Rect, Stroke, pos2};
        let full = ui.max_rect();
        let resp = ui.interact(full, ui.id().with("windows"), Sense::click());
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let record_after = self.area.record_after;
        let Some(wm) = self.area.windows.as_mut() else { return };
        // This overlay shows the windows on its own display.
        let mine: Vec<winpick::PickWindow> = wm.list.iter().filter(|w| w.display == display).cloned().collect();
        let hovered = pointer.and_then(|p| winpick::under(&mine, p)).map(|w| w.entry.id);
        if resp.secondary_clicked() {
            wm.menu_for = hovered;
        }
        if resp.clicked() {
            wm.selected = hovered;
            // The previous selection may be drawn on another display.
            for w in &self.overlays {
                w.window.request_redraw();
            }
        }
        let focus = wm.selected.or(hovered).and_then(|id| mine.iter().find(|w| w.entry.id == id)).cloned();
        let visible: Vec<Rect> = mine.iter().filter(|w| !winpick::is_ignored(w.entry.id)).map(|w| w.rect).collect();

        let painter = ui.painter().clone();
        let dim = Color32::from_black_alpha(110);
        match &focus {
            Some(f) => {
                let r = f.rect.intersect(full);
                painter.rect_filled(Rect::from_min_max(full.min, pos2(full.max.x, r.top())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.bottom()), full.max), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(full.min.x, r.top()), pos2(r.left(), r.bottom())), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(pos2(r.right(), r.top()), pos2(full.max.x, r.bottom())), 0.0, dim);
            }
            None => {
                painter.rect_filled(full, 0.0, dim);
            }
        }
        for r in visible.iter().rev() {
            painter.rect_stroke(*r, 8.0, Stroke::new(1.0, Color32::from_white_alpha(60)), egui::StrokeKind::Inside);
        }
        let mut choose = None;
        let mut adjust = None;
        let mut refresh = false;
        let mut whole_display = false;
        if let Some(f) = &focus {
            painter.rect_filled(f.rect, 8.0, theme::accent().gamma_multiply(0.12));
            painter.rect_stroke(f.rect, 8.0, Stroke::new(3.0, theme::accent()), egui::StrokeKind::Inside);
            let size = vec2(340.0, 150.0);
            let mut card = Rect::from_center_size(f.rect.intersect(full).center(), size);
            card = card.translate(vec2(
                (full.left() + 12.0 - card.left()).max(0.0) + (full.right() - 12.0 - card.right()).min(0.0),
                (full.top() + 40.0 - card.top()).max(0.0) + (full.bottom() - 12.0 - card.bottom()).min(0.0),
            ));
            ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                egui::Frame::new().fill(Color32::from_black_alpha(220)).corner_radius(12).inner_margin(14).show(ui, |ui| {
                    ui.set_width(size.x - 28.0);
                    ui.label(RichText::new(&f.app).font(theme::semibold(17.0)).color(Color32::WHITE));
                    if !f.title.is_empty() {
                        let title: String = f.title.chars().take(48).collect();
                        ui.label(RichText::new(title).size(12.0).color(Color32::from_white_alpha(170)));
                    }
                    ui.add_space(4.0);
                    if let Some((w, h)) = home::window_size_row(ui, &mut self.settings, &f.entry) {
                        let note = match crate::capture::winsize::resize(f.entry.id, w as f64, h as f64) {
                            Ok((gw, gh)) if (gw.round() as u32, gh.round() as u32) != (w, h) => Some(format!("The app limited the window to {gw:.0} × {gh:.0}")),
                            Ok(_) => None,
                            Err(e) => Some(format!("{e:#}")),
                        };
                        home::set_resize_note(note);
                        refresh = true;
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, if record_after { "Record window" } else { "Use window" }).clicked() {
                            choose = Some(f.entry.id);
                        }
                        if ui.button("Adjust area").on_hover_text("Record a region starting from this window's frame").clicked() {
                            adjust = Some(f.rect.intersect(full));
                        }
                    });
                });
            });
        }
        resp.context_menu(|ui| {
            let Some(id) = wm.menu_for else {
                ui.close();
                return;
            };
            if ui.button("Bring to front").clicked() {
                if let Err(e) = crate::capture::winsize::raise(id) {
                    home::set_resize_note(Some(format!("{e:#}")));
                }
                refresh = true;
            }
            if ui.button("Ignore this window").clicked() {
                winpick::ignore(id);
                if wm.selected == Some(id) {
                    wm.selected = None;
                }
            }
            if ui.button("Reset ignored windows").clicked() {
                winpick::reset_ignored();
            }
        });
        if visible.is_empty() {
            let r = Rect::from_center_size(full.center(), vec2(320.0, 90.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                *ui.visuals_mut() = egui::Visuals::dark();
                egui::Frame::new().fill(Color32::from_black_alpha(220)).corner_radius(12).inner_margin(14).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No windows to select").font(theme::semibold(16.0)).color(Color32::WHITE));
                        if ui.button("Record entire display").clicked() {
                            whole_display = true;
                        }
                    });
                });
            });
        }
        let hint = "Click a window to choose it  ·  Right-click for more  ·  Esc to cancel";
        let galley = painter.layout_no_wrap(hint.into(), FontId::proportional(14.0), Color32::WHITE);
        let bg = Rect::from_center_size(pos2(full.center().x, full.top() + 60.0), galley.size() + vec2(28.0, 14.0));
        painter.rect_filled(bg, egui::CornerRadius::same(10), Color32::from_black_alpha(170));
        painter.text(bg.center(), Align2::CENTER_CENTER, hint, FontId::proportional(14.0), Color32::WHITE);
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            choose = choose.or(focus.as_ref().map(|f| f.entry.id));
        }
        let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));

        if refresh && let Some(wm) = &mut self.area.windows {
            wm.list = winpick::list_all();
        }
        if let Some(r) = adjust {
            self.area.windows = None;
            self.area.display = display;
            self.area.rect = Some(r);
            for w in &self.overlays {
                w.window.request_redraw();
            }
        } else if whole_display {
            self.settings.source = SourceKind::Display;
            self.settings.display = display;
            self.finish_window(None);
            if record_after {
                push(Cmd::ToggleRecording);
            }
        } else if choose.is_some() || cancel {
            self.finish_window(choose);
        }
    }

    /// Close the window picker; with a window, use it (and record if asked).
    fn finish_window(&mut self, id: Option<u32>) {
        self.area.closed = true;
        self.area.windows = None;
        self.overlays.clear();
        match id {
            Some(id) => {
                self.settings.source = SourceKind::Window;
                self.settings.window = Some(id);
                self.settings.window_label = None;
                if !self.scripted() {
                    self.settings.save();
                }
                self.devices = Devices::load();
                push(if self.area.record_after { Cmd::ToggleRecording } else { Cmd::ShowWindow });
            }
            None => {
                if !self.area.record_after || self.settings.source != SourceKind::Display {
                    push(Cmd::ShowWindow);
                }
            }
        }
    }

    /// Close the area picker; with a rect, save it (and record if asked).
    fn finish_area(&mut self, rect: Option<egui::Rect>) {
        self.area.closed = true;
        self.overlays.clear();
        match rect {
            Some(r) => {
                self.settings.source = SourceKind::Area;
                self.settings.display = self.area.display;
                self.settings.area = [r.left() as f64, r.top() as f64, r.width() as f64, r.height() as f64];
                if !self.scripted() {
                    self.settings.save();
                }
                push(if self.area.record_after { Cmd::ToggleRecording } else { Cmd::ShowWindow });
            }
            None => push(Cmd::ShowWindow),
        }
    }

    fn which(&self, id: WindowId) -> Option<Which> {
        let is = |w: &Option<Win>| w.as_ref().is_some_and(|w| w.window.id() == id);
        if is(&self.main) {
            Some(Which::Main)
        } else {
            self.overlays.iter().find(|w| w.window.id() == id).map(|w| Which::Overlay(w.display))
        }
    }

    /// Take a window out for drawing (put back with `put_back`).
    fn take_window(&mut self, which: Which) -> Option<Win> {
        match which {
            Which::Main => self.main.take(),
            Which::Overlay(d) => {
                let i = self.overlays.iter().position(|w| w.display == d)?;
                Some(self.overlays.remove(i))
            }
        }
    }

    fn window_mut(&mut self, which: Which) -> Option<&mut Win> {
        match which {
            Which::Main => self.main.as_mut(),
            Which::Overlay(d) => self.overlays.iter_mut().find(|w| w.display == d),
        }
    }

    fn put_back(&mut self, which: Which, win: Win) {
        match which {
            Which::Main => self.main = Some(win),
            Which::Overlay(_) => self.overlays.push(win),
        }
    }

    fn redraw(&mut self, id: WindowId) {
        let Some(which) = self.which(id) else { return };
        let Some(gpu) = self.gpu.clone() else { return };
        let Some(mut win) = self.take_window(which) else { return };
        if which == Which::Main {
            for id in self.stale_textures.drain(..) {
                win.renderer.free_texture(&id);
            }
            let fs = macos::fullscreen_display(&win.window);
            theme::FULLSCREEN.store(fs.is_some() || macos::is_fullscreen(&win.window), std::sync::atomic::Ordering::Relaxed);
            theme::FULLSCREEN_DISPLAY.store(fs.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        }
        let input = win.egui.take_egui_input(&win.window);
        let ctx = win.ctx.clone();
        let renderer = &mut win.renderer;
        // The overlay may close itself while drawing; remember that.
        let mut keep = true;
        let mut out = ctx.run_ui(input, |ui| match which {
            Which::Main => self.draw_main(ui, renderer),
            Which::Overlay(display) => {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    self.draw_overlay(ui, display);
                });
                keep = !self.area.closed;
            }
        });
        win.egui.handle_platform_output(&win.window, out.platform_output);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(Duration::MAX, |v| v.repaint_delay);
        win.repaint_at = if delay < Duration::from_secs(60) { Some(Instant::now() + delay) } else { None };
        let ppp = out.pixels_per_point;
        let prims = win.ctx.tessellate(out.shapes, ppp);
        for (id, deltas) in &out.textures_delta.set {
            for d in deltas {
                win.renderer.update_texture(&gpu.device, &gpu.queue, *id, d);
            }
        }
        let frame = match win.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            _ => {
                win.surface.configure(&gpu.device, &win.config);
                None
            }
        };
        if let Some(frame) = frame {
            if !win.overlay {
                let (x, y) = theme::TRAFFIC_LIGHTS;
                macos::inset_traffic_lights(&win.window, x, y);
            }
            let view = frame.texture.create_view(&Default::default());
            let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: [win.config.width, win.config.height], pixels_per_point: ppp };
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            let extra = win.renderer.update_buffers(&gpu.device, &gpu.queue, &mut enc, &prims, &sd);
            {
                let bg = theme::palette(win.ctx.theme() == egui::Theme::Dark).window;
                let [r, g, b, _] = bg.to_normalized_gamma_f32();
                let mut pass = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(if win.overlay {
                                    wgpu::Color::TRANSPARENT
                                } else {
                                    wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                win.renderer.render(&mut pass, &prims, &sd);
            }
            gpu.queue.submit(extra.into_iter().chain([enc.finish()]));
            win.window.pre_present_notify();
            gpu.queue.present(frame);
        }
        for id in &out.textures_delta.free {
            win.renderer.free_texture(id);
        }
        // Applied above. egui debug-asserts when a frame drops its texture deltas.
        out.textures_delta.clear();
        if DRAG.swap(false, Ordering::Relaxed) {
            let _ = win.window.drag_window();
        }
        if keep {
            self.put_back(which, win);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.menubar.is_some() {
            return;
        }
        let mtm = objc2::MainThreadMarker::new().expect("main thread");
        self.menubar = Some(menubar::MenuBar::new(mtm));
        self.bind_hotkeys();
        self.show_main(el);
        if let Some(p) = self.open_at_launch.take() {
            self.open_project(el, p, Leaving::OpenProject);
        }
        if !self.scripted() {
            push(Cmd::StartUpdater);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, _: ()) {
        self.tick(el);
        for w in self.main.iter().chain(&self.overlays) {
            w.window.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(which) = self.which(id) else { return };
        if let (Which::Main, WindowEvent::Occluded(o)) = (which, &event) {
            self.main_occluded = *o;
            self.update_live();
        }
        if matches!(event, WindowEvent::CloseRequested) {
            match which {
                Which::Main => {
                    if let View::Editor(ed) = &mut self.view {
                        ed.save();
                    }
                    push(Cmd::Quit);
                }
                Which::Overlay(_) => {
                    self.area.closed = true;
                    self.area.windows = None;
                    self.overlays.clear();
                    push(Cmd::ShowWindow);
                }
            }
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.redraw(id);
            return;
        }
        let gpu = self.gpu.clone();
        let Some(win) = self.window_mut(which) else { return };
        let resp = win.egui.on_window_event(&win.window, &event);
        match event {
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                let s = win.window.inner_size();
                win.config.width = s.width.max(1);
                win.config.height = s.height.max(1);
                if let Some(g) = &gpu {
                    win.surface.configure(&g.device, &win.config);
                }
                win.window.request_redraw();
            }
            WindowEvent::ThemeChanged(_) => {
                theme::set_accent(theme::system_accent());
                theme::apply(&win.ctx);
                win.window.request_redraw();
            }
            WindowEvent::DroppedFile(path) => {
                if path.extension().and_then(|e| e.to_str()) == Some(crate::project::EXTENSION) {
                    push(Cmd::Open(path));
                }
            }
            _ => {
                if resp.repaint {
                    win.window.request_redraw();
                }
            }
        }
        let _ = el;
    }

    /// ⌘Q from the app menu, logout or our own quit: never lose work.
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        if let View::Editor(ed) = &mut self.view {
            if ed.voice.is_some() {
                ed.stop_voiceover();
            }
            ed.cancel_export();
            ed.save();
        }
        if let Some(Recording { rec: Some(r), .. }) = self.recording.take()
            && let Err(e) = objc2::rc::autoreleasepool(|_| r.stop())
        {
            eprintln!("glide: saving the recording on quit failed: {e:#}");
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.winding_down() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !self.scripted() {
            self.settings.save();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.last_tick.elapsed() >= Duration::from_millis(100) {
            self.last_tick = Instant::now();
            self.tick(el);
        }
        let (animating, busy) = match &mut self.view {
            View::Editor(ed) => (ed.tick(), ed.background_busy()),
            View::Start => (false, false),
        };
        let live = self.recording.is_some() || self.finishing || self.restart.busy();
        let now = Instant::now();
        let mut next = now + Duration::from_millis(if live { 200 } else { 500 });
        // Nothing to show while the window is hidden or covered.
        let shown = self.main.as_ref().is_some_and(|w| w.window.is_visible() != Some(false)) && !self.main_occluded;
        if animating && shown && let Some(w) = &self.main {
            // One redraw per preview frame (30 fps in light mode). Every
            // redraw wakes this loop again, so redraw only once it's due.
            let period = Duration::from_millis(if crate::hw::light() { 33 } else { 16 });
            let due = self.next_frame_at.unwrap_or(now);
            if due <= now {
                w.window.request_redraw();
                // Keep the cadence; restart it after a stall.
                let after = due + period;
                self.next_frame_at = Some(if after > now { after } else { now + period });
            }
            next = next.min(self.next_frame_at.unwrap_or(now + period));
        } else if busy && shown && let Some(w) = &mut self.main {
            // Export/transcription progress: a few redraws a second.
            if w.repaint_at.is_none() {
                w.repaint_at = Some(now + Duration::from_millis(250));
            }
        }
        for w in self.main.iter_mut().chain(self.overlays.iter_mut()) {
            if let Some(t) = w.repaint_at {
                if t <= now {
                    w.repaint_at = None;
                    w.window.request_redraw();
                } else {
                    next = next.min(t);
                }
            }
        }
        el.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

/// Run the app (blocks until quit). `open` = a project to open at launch.
pub fn run(open: Option<PathBuf>) -> Result<()> {
    theme::set_accent(theme::system_accent());
    // Let connected iPhones/iPads appear as recordable devices.
    crate::capture::camera::enable_screen_capture_devices();
    std::thread::spawn(player::sweep_stale);
    let el = EventLoop::new()?;
    let _ = PROXY.set(Mutex::new(el.create_proxy()));
    macos::install_open_handler();
    let mut app = App::new(open);
    el.run_app(&mut app)?;
    Ok(())
}

/// When Glide is full screen on the display it would record, that display
/// shows nothing but Glide (left out of the capture): say so over the preview.
fn covered_note(s: &home::Settings) -> Option<String> {
    let fs = theme::FULLSCREEN_DISPLAY.load(std::sync::atomic::Ordering::Relaxed);
    let target = if s.display == 0 { macos::main_display() } else { s.display };
    (fs != 0 && matches!(s.source, SourceKind::Display | SourceKind::Area) && fs == target)
        .then(|| "Glide is full screen on this display, so there's nothing else to show.\nExit full screen to see what you'll record.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    type TestSlot = Slot<u8, u8>;

    /// Let the wanted key's 300 ms settle time pass without sleeping.
    fn settled(slot: &mut TestSlot) {
        slot.want_since = Some(Instant::now() - Duration::from_secs(1));
    }

    /// Call `update` until `done` holds (the start runs on another thread).
    fn pump(slot: &mut TestSlot, want: Option<u8>, start: fn(&u8) -> Result<u8, String>, done: impl Fn(&TestSlot) -> bool) {
        for _ in 0..400 {
            slot.update(want, "test", start);
            if done(slot) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("slot did not settle");
    }

    static FAIL_CALLS: AtomicUsize = AtomicUsize::new(0);
    fn always_fails(_: &u8) -> Result<u8, String> {
        FAIL_CALLS.fetch_add(1, Ordering::SeqCst);
        Err("no device".into())
    }

    #[test]
    fn slot_does_not_retry_a_failed_key_by_itself() {
        let mut slot = TestSlot::default();
        slot.update(Some(1), "test", always_fails);
        settled(&mut slot);
        pump(&mut slot, Some(1), always_fails, |s| s.error().is_some());
        assert_eq!(slot.error(), Some("no device"));
        for _ in 0..20 {
            slot.update(Some(1), "test", always_fails);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(FAIL_CALLS.load(Ordering::SeqCst), 1);
    }

    static RESET_CALLS: AtomicUsize = AtomicUsize::new(0);
    fn fails_once(_: &u8) -> Result<u8, String> {
        if RESET_CALLS.fetch_add(1, Ordering::SeqCst) == 0 { Err("first try".into()) } else { Ok(7) }
    }

    #[test]
    fn slot_reset_failed_starts_the_same_key_again() {
        let mut slot = TestSlot::default();
        slot.update(Some(1), "test", fails_once);
        settled(&mut slot);
        pump(&mut slot, Some(1), fails_once, |s| s.error().is_some());
        slot.reset_failed();
        assert_eq!(slot.error(), None);
        pump(&mut slot, Some(1), fails_once, |s| s.running.is_some());
        assert_eq!(slot.running.as_ref().map(|(_, v)| *v), Some(7));
        assert_eq!(RESET_CALLS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn slot_retry_failed_waits_for_the_interval() {
        let t0 = Instant::now();
        let mut slot = TestSlot { want: Some(1), failed: Some((1, "locked".into())), failed_at: Some(t0), ..Default::default() };
        assert!(!slot.retry_failed(t0 + Duration::from_secs(1), Duration::from_secs(2)));
        assert_eq!(slot.error(), Some("locked"));
        assert!(slot.retry_failed(t0 + Duration::from_secs(3), Duration::from_secs(2)));
        assert_eq!(slot.error(), None);
        assert!(!slot.retry_failed(t0 + Duration::from_secs(9), Duration::from_secs(2)), "nothing left to retry");
    }

    fn starts_fine(_: &u8) -> Result<u8, String> {
        Ok(3)
    }

    #[test]
    fn slot_fail_running_stops_the_value_and_shows_the_message() {
        let mut slot = TestSlot::default();
        slot.update(Some(1), "test", starts_fine);
        settled(&mut slot);
        pump(&mut slot, Some(1), starts_fine, |s| s.running.is_some());
        assert!(slot.fail_running("test", "The iPhone/iPad was disconnected.".into()));
        assert!(slot.running.is_none());
        assert_eq!(slot.error(), Some("The iPhone/iPad was disconnected."));
        // Nothing is running any more, so a second report changes nothing.
        assert!(!slot.fail_running("test", "again".into()));
        assert_eq!(slot.error(), Some("The iPhone/iPad was disconnected."));
    }

    #[test]
    fn slot_starts_again_after_fail_running_once_the_retry_interval_passes() {
        let mut slot = TestSlot::default();
        slot.update(Some(1), "test", starts_fine);
        settled(&mut slot);
        pump(&mut slot, Some(1), starts_fine, |s| s.running.is_some());
        slot.fail_running("test", "locked".into());
        // Failed keys are not restarted until the retry clears them.
        slot.update(Some(1), "test", starts_fine);
        assert!(slot.starting.is_none() && slot.running.is_none());
        let later = slot.failed_at.unwrap() + Duration::from_secs(3);
        assert!(slot.retry_failed(later, IOS_RETRY));
        pump(&mut slot, Some(1), starts_fine, |s| s.running.is_some());
    }

    /// A value whose drop takes 100 ms and then sets a flag.
    struct SlowDrop(&'static std::sync::atomic::AtomicBool);
    impl Drop for SlowDrop {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(100));
            self.0.store(true, Ordering::SeqCst);
        }
    }

    static JOIN_DROPPED: AtomicBool = AtomicBool::new(false);
    #[test]
    fn slot_stop_and_join_waits_for_a_drop_already_running_in_the_background() {
        let mut slot: Slot<u8, SlowDrop> = Slot::default();
        slot.update(Some(1), "test", |_| Ok(SlowDrop(&JOIN_DROPPED)));
        slot.want_since = Some(Instant::now() - Duration::from_secs(1));
        for _ in 0..400 {
            slot.update(Some(1), "test", |_| Ok(SlowDrop(&JOIN_DROPPED)));
            if slot.running.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(slot.running.is_some());
        // Nothing wanted: the value is handed to a background thread.
        slot.update(None, "test", |_| Ok(SlowDrop(&JOIN_DROPPED)));
        assert!(slot.running.is_none());
        assert!(!JOIN_DROPPED.load(Ordering::SeqCst), "the drop should still be running");
        slot.stop_and_join();
        assert!(JOIN_DROPPED.load(Ordering::SeqCst));
        assert!(slot.dropping.is_empty());
    }

    static STARTING_DROPPED: AtomicBool = AtomicBool::new(false);
    #[test]
    fn slot_stop_and_join_waits_for_a_value_that_is_still_starting() {
        let mut slot: Slot<u8, SlowDrop> = Slot::default();
        let start: fn(&u8) -> Result<SlowDrop, String> = |_| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(SlowDrop(&STARTING_DROPPED))
        };
        slot.update(Some(1), "test", start);
        slot.want_since = Some(Instant::now() - Duration::from_secs(1));
        slot.update(Some(1), "test", start);
        assert!(slot.starting.is_some());
        slot.stop_and_join();
        assert!(slot.running.is_none() && slot.starting.is_none() && slot.want.is_none());
        assert!(STARTING_DROPPED.load(Ordering::SeqCst), "the late value should be dropped before returning");
    }

    static STARTING_FINISHED: AtomicBool = AtomicBool::new(false);
    #[test]
    fn slot_stop_and_join_joins_startup_thread_completely() {
        let mut slot: Slot<u8, u8> = Slot::default();
        let start: fn(&u8) -> Result<u8, String> = |_| {
            std::thread::sleep(Duration::from_millis(50));
            STARTING_FINISHED.store(true, Ordering::SeqCst);
            Err("failed".into())
        };
        slot.update(Some(1), "test", start);
        slot.want_since = Some(Instant::now() - Duration::from_secs(1));
        slot.update(Some(1), "test", start);
        assert!(slot.starting.is_some());
        slot.stop_and_join();
        assert!(STARTING_FINISHED.load(Ordering::SeqCst), "startup thread must be joined before returning");
        assert!(slot.starting.is_none());
    }

    #[test]
    fn slot_stop_and_join_on_an_idle_slot_returns_at_once() {
        let mut slot = TestSlot::default();
        let t = Instant::now();
        slot.stop_and_join();
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn slot_changed_key_clears_the_failure() {
        let mut slot = TestSlot { want: Some(1), failed: Some((1, "locked".into())), failed_at: Some(Instant::now()), ..Default::default() };
        slot.update(Some(2), "test", always_fails);
        assert_eq!(slot.error(), None);
        assert!(slot.failed_at.is_none());
    }
}
