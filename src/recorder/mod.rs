//! Recording into a `.glide` package: the screen (without the cursor) with
//! system audio and microphone stems, the camera as its own file, and
//! input events for re-rendering the cursor, zooms and key badges.

pub mod input;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::capture::camera::Camera;
use crate::capture::compose::{Corner, PipSpec, Scene as CapScene, Shape};
use crate::capture::record::{MicBackend, Options as CapOptions, Session, Shared};
use crate::capture::screen;
use crate::capture::writer::{AudioLayout, Codec as CapCodec, Sink, VideoSpec};
use crate::engine::geom::Rect;
use crate::project::recording::{
    AudioTrack, CursorImage, KeyEvent, MouseEvent, RecordingMeta, SourceKind, VideoTrack,
};
use crate::project::{self, Package};
use crate::util::*;
use input::{InputLog, InputLogger};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Display { id: u32 },
    Window { id: u32 },
    /// A region of a display, in display-local points (top-left origin).
    Area { display: u32, rect: Rect },
    /// Generated test pattern (no permission needed).
    Pattern,
    /// A USB-connected iPhone or iPad screen (unique id from `glide devices`).
    Device { id: String },
}

#[derive(Clone, Debug)]
pub struct Options {
    pub target: Target,
    pub system_audio: bool,
    /// None = off; Some(None) = default microphone.
    pub mic: Option<Option<String>>,
    /// None = off; Some(None) = default camera.
    pub camera: Option<Option<String>>,
    /// The camera bubble's look for the new project (None = the default).
    pub camera_style: Option<crate::project::model::CameraStyle>,
    pub fps: u32,
    /// Log key presses (needs Input Monitoring permission).
    pub keys: bool,
    /// Folder for the new package.
    pub dir: PathBuf,
    /// Package file name stem (default: "Recording <date>").
    pub name: Option<String>,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    /// Leave this app's own sounds out of the system audio. ScreenCaptureKit
    /// treats every running copy of Glide as this app, so the command line
    /// (which makes no sound) turns it off.
    pub exclude_own_audio: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            target: Target::Display { id: 0 },
            system_audio: true,
            mic: None,
            camera: None,
            camera_style: None,
            fps: 60,
            keys: true,
            dir: project::projects_dir(),
            name: None,
            hide_desktop_icons: false,
            hide_dock: false,
            exclude_own_audio: true,
        }
    }
}

/// A camera-only constant-frame-rate recording (its own file).
struct CameraRec {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    pacer: Option<std::thread::JoinHandle<()>>,
    cam: Camera,
}

impl CameraRec {
    fn start(sel: Option<&str>, path: &std::path::Path) -> Result<Self> {
        let spec = VideoSpec { width: 1280, height: 720, fps: 30, codec: CapCodec::H264, bitrate: 8_000_000, quality: None, pixel_format: PIX_BGRA };
        Self::start_spec(path, &spec, |shared| Camera::start_with(sel, shared, true))
    }

    /// An iOS device's screen at its native size.
    fn start_ios(id: &str, path: &std::path::Path, size: (usize, usize)) -> Result<Self> {
        let bitrate = ((size.0 * size.1) as f64 * 60.0 * 0.08) as u64;
        let spec = VideoSpec { width: size.0, height: size.1, fps: 60, codec: CapCodec::Hevc, bitrate: bitrate.max(4_000_000), quality: None, pixel_format: PIX_BGRA };
        Self::start_spec(path, &spec, |shared| Camera::start_ios(id, shared, size))
    }

    fn start_spec(path: &std::path::Path, spec: &VideoSpec, open: impl FnOnce(Arc<Shared>) -> Result<Camera>) -> Result<Self> {
        let sink = Sink::new(Some(path), spec, false, false, AudioLayout::Stems, [0.0; 2], None, CapScene::Screen, None)?;
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared { sink: Mutex::new(Some(sink)), error: Mutex::new(None), stop: stop.clone() });
        let cam = open(shared.clone())?;
        let (ps, pstop) = (shared.clone(), stop.clone());
        let pacer = std::thread::spawn(move || {
            while !pstop.load(Ordering::SeqCst) {
                let now = secs(host_now());
                objc2::rc::autoreleasepool(|_| ps.with(|s| s.emit_until(now - s.lag())));
                std::thread::sleep(Duration::from_millis(16));
            }
        });
        Ok(CameraRec { shared, stop, pacer: Some(pacer), cam })
    }

    fn t0(&self) -> Option<f64> {
        let mut t = None;
        self.shared.with(|s| t = s.t0_secs());
        t
    }

    fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    fn pause(&self, now: f64) {
        self.shared.with(|s| {
            s.pause(now);
        });
    }

    fn resume(&self, now: f64) {
        self.shared.with(|s| {
            s.resume(now);
        });
    }

    fn finish(mut self) -> Result<()> {
        let end = secs(host_now());
        self.cam.stop();
        self.stop.store(true, Ordering::SeqCst);
        if let Some(p) = self.pacer.take() {
            let _ = p.join();
        }
        let sink = self.shared.sink.lock().unwrap().take();
        match sink {
            Some(s) => objc2::rc::autoreleasepool(|_| s.finish(end)).map(|_| ()),
            None => Ok(()),
        }
    }
}

pub struct Recorder {
    pub package: Package,
    session: Option<Session>,
    camera: Option<CameraRec>,
    /// An iOS device recorded as the main video (instead of `session`).
    device: Option<CameraRec>,
    input: Option<InputLogger>,
    kind: SourceKind,
    /// Captured region in global points at the start.
    bounds: Rect,
    /// Video pixels per point.
    scale: f64,
    size: (usize, usize),
    window: Option<u32>,
    pauses: Vec<(f64, Option<f64>)>,
    pub started: Instant,
    opts: Options,
    app_name: Option<String>,
}

fn display_bounds(id: u32) -> Rect {
    let r = unsafe { input::CGDisplayBounds(id) };
    Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
}

/// VideoToolbox quality for screen recordings (see `Quality::encoder_quality`).
const RECORD_QUALITY: f64 = 0.95;

fn even(v: f64) -> usize {
    ((v / 2.0).round() as usize * 2).max(2)
}

impl Recorder {
    pub fn start(opts: Options) -> Result<Self> {
        let stamp = crate::util::timestamp_name();
        std::fs::create_dir_all(&opts.dir).with_context(|| format!("creating {}", opts.dir.display()))?;
        let path = match &opts.name {
            Some(n) => opts.dir.join(format!("{n}.{}", project::EXTENSION)),
            None => project::new_package_path(&opts.dir, &stamp),
        };
        let package = Package::new(&path);

        // Resolve the target: what to capture, at what size, where it is.
        let (cap_target, kind, bounds, size, window, app_name) = match &opts.target {
            Target::Pattern => (None, SourceKind::Display, Rect::new(0.0, 0.0, 1440.0, 900.0), (2880, 1800), None, None),
            Target::Device { id } => {
                crate::capture::tcc::check_camera_now()?;
                let (w, h) = crate::capture::camera::ios_dimensions(id).context("iPhone/iPad not found — connect it with a cable and unlock it")?;
                let px = (even(w as f64), even(h as f64));
                (None, SourceKind::Device, Rect::new(0.0, 0.0, px.0 as f64, px.1 as f64), px, None, None)
            }
            Target::Display { id } => {
                let ds = screen::displays()?;
                let d = ds.into_iter().find(|d| d.id == *id || *id == 0).context("display not found")?;
                let b = display_bounds(d.id);
                let px = (even(d.pixels.0 as f64), even(d.pixels.1 as f64));
                (Some(screen::Target::Display(d, None)), SourceKind::Display, b, px, None, None)
            }
            Target::Area { display, rect } => {
                let ds = screen::displays()?;
                let d = ds.into_iter().find(|d| d.id == *display || *display == 0).context("display not found")?;
                let db = display_bounds(d.id);
                // Keep the area on the display (a saved area may be from a bigger one).
                let rect = &rect.intersect(&Rect::new(0.0, 0.0, d.points.0 as f64, d.points.1 as f64));
                anyhow::ensure!(rect.w >= 8.0 && rect.h >= 8.0, "the selected area is outside this display");
                let scale = d.pixels.0 as f64 / d.points.0 as f64;
                let px = (even(rect.w * scale), even(rect.h * scale));
                let cg = objc2_core_foundation::CGRect::new(
                    objc2_core_foundation::CGPoint::new(rect.x, rect.y),
                    objc2_core_foundation::CGSize::new(rect.w, rect.h),
                );
                let global = rect.translate(db.min());
                (Some(screen::Target::Display(d, Some(cg))), SourceKind::Area, global, px, None, None)
            }
            Target::Window { id } => {
                let w = screen::find_window(&id.to_string())?;
                let b = input::window_bounds(*id).unwrap_or(Rect::new(0.0, 0.0, w.size.0, w.size.1));
                // Native pixels of the display the window is on.
                let on = w.display.as_ref().map(|d| unsafe { d.displayID() });
                let ds = screen::displays()?;
                let d = ds.iter().find(|d| Some(d.id) == on).or(ds.first());
                let scale = d.map_or(2.0, |d| d.pixels.0 as f64 / d.points.0 as f64);
                let px = (even(w.size.0 * scale), even(w.size.1 * scale));
                let app = Some(w.app.clone());
                (Some(screen::Target::Window(w)), SourceKind::Window, b, px, Some(*id), app)
            }
        };
        // Near-transparent constant-quality HEVC at native pixels, so fine
        // text survives scrolling and re-encoding on export stays clean. In
        // light mode (older Macs) H.264 at a fixed bitrate: the cheapest
        // encode and decode for them, and the encoder they all have.
        let codec = if crate::hw::light() { CapCodec::H264 } else { CapCodec::Hevc };
        // Never more pixels than the hardware encoder takes (else it falls
        // back to software and the recording stalls).
        let size = match opts.target {
            Target::Pattern | Target::Device { .. } => size,
            _ => {
                let max_edge = if crate::hw::light() { crate::hw::LIGHT_MAX_EDGE } else { usize::MAX };
                crate::hw::fit_encoder(if codec == CapCodec::H264 { crate::hw::H264 } else { crate::hw::HEVC }, size, max_edge)
            }
        };
        let scale = size.0 as f64 / bounds.w.max(1.0);
        // Only now (the target resolved) create the package folder.
        std::fs::create_dir_all(package.recording_dir())?;
        if let Target::Device { id } = &opts.target {
            let device = CameraRec::start_ios(id, &package.rec("screen.mov"), size).inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&package.root);
            })?;
            return Ok(Recorder {
                package,
                session: None,
                camera: None,
                device: Some(device),
                input: None,
                kind,
                bounds,
                scale,
                size,
                window,
                pauses: vec![],
                started: Instant::now(),
                opts,
                app_name: Some("iPhone".into()),
            });
        }
        // H.264 needs more bits than HEVC for the same sharpness.
        let bits_per_pixel = if codec == CapCodec::H264 { 0.15 } else { 0.08 };
        let bitrate = ((size.0 * size.1) as f64 * opts.fps as f64 * bits_per_pixel) as u64;
        let cap = CapOptions {
            output: package.rec("screen.mov"),
            size,
            fps: opts.fps,
            codec,
            bitrate: bitrate.max(4_000_000),
            quality: (codec == CapCodec::Hevc).then_some(RECORD_QUALITY),
            system_audio: opts.system_audio,
            mic: opts.mic.clone(),
            webcam: None,
            pip: PipSpec { size: 0.2, corner: Corner::BottomRight, shape: Shape::Rounded, mirror: false },
            cursor: false,
            clicks: false,
            duration: None,
            quiet: true,
            test_pattern: matches!(opts.target, Target::Pattern).then_some(false),
            audio_layout: AudioLayout::Stems,
            gains_db: [0.0, 0.0],
            mic_backend: if screen::captures_microphone() { MicBackend::Sck } else { MicBackend::Avcapture },
            scene: CapScene::Screen,
            hide_desktop_icons: opts.hide_desktop_icons,
            hide_dock: opts.hide_dock,
            exclude_own_audio: opts.exclude_own_audio,
        };
        if let Err(e) = crate::capture::record::check_permissions(&cap) {
            let _ = std::fs::remove_dir_all(&package.root);
            anyhow::bail!("{e}");
        }
        let input = InputLogger::start(package.rec("cursors"), window, opts.keys);
        let session = match Session::start(&cap, cap_target, true, None) {
            Ok(s) => s,
            Err(e) => {
                input.stop();
                let _ = std::fs::remove_dir_all(&package.root);
                return Err(e);
            }
        };
        let camera = match &opts.camera {
            Some(sel) => match CameraRec::start(sel.as_deref(), &package.rec("camera.mov")) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("glide: camera unavailable: {e}");
                    None
                }
            },
            None => None,
        };
        Ok(Recorder {
            package,
            session: Some(session),
            camera,
            device: None,
            input: Some(input),
            kind,
            bounds,
            scale,
            size,
            window,
            pauses: vec![],
            started: Instant::now(),
            opts,
            app_name,
        })
    }

    pub fn is_paused(&self) -> bool {
        self.pauses.last().is_some_and(|p| p.1.is_none())
    }

    fn paused_secs(&self, now: f64) -> f64 {
        self.pauses.iter().map(|&(a, b)| b.unwrap_or(now) - a).sum()
    }

    pub fn pause(&mut self) {
        if self.is_paused() || !self.has_frames() {
            return;
        }
        let now = secs(host_now());
        let paused = match (&self.session, &self.device) {
            (Some(s), _) => s.pause(),
            (None, Some(d)) => {
                d.pause(now);
                true
            }
            _ => false,
        };
        if paused {
            if let Some(c) = &self.camera {
                c.pause(now);
            }
            self.pauses.push((now, None));
        }
    }

    pub fn resume(&mut self) {
        if !self.is_paused() {
            return;
        }
        let now = secs(host_now());
        let resumed = match (&self.session, &self.device) {
            (Some(s), _) => s.resume(),
            (None, Some(d)) => {
                d.resume(now);
                true
            }
            _ => false,
        };
        if resumed {
            if let Some(c) = &self.camera {
                c.resume(now);
            }
            if let Some(p) = self.pauses.last_mut() {
                p.1 = Some(now);
            }
        }
    }

    /// Recorded seconds (excluding pauses).
    pub fn elapsed(&self) -> f64 {
        match (&self.session, &self.device) {
            (Some(s), _) => s.recorded_secs(),
            (None, Some(d)) => d.t0().map_or(0.0, |t0| {
                let now = secs(host_now());
                (now - t0 - self.paused_secs(now)).max(0.0)
            }),
            _ => 0.0,
        }
    }

    pub fn error(&self) -> Option<String> {
        match (&self.session, &self.device) {
            (Some(s), _) => s.failed().then(|| s.error()).flatten(),
            (None, Some(d)) => d.error(),
            _ => None,
        }
    }

    pub fn has_frames(&self) -> bool {
        match (&self.session, &self.device) {
            (Some(s), _) => s.has_frames(),
            (None, Some(d)) => d.t0().is_some(),
            _ => false,
        }
    }

    /// Discard the recording.
    pub fn cancel(mut self) {
        if let Some(i) = self.input.take() {
            i.stop();
        }
        if let Some(c) = self.camera.take() {
            let _ = c.finish();
        }
        if let Some(d) = self.device.take() {
            let _ = d.finish();
        }
        if let Some(s) = self.session.take() {
            let _ = s.finish();
        }
        let _ = std::fs::remove_dir_all(&self.package.root);
    }

    /// True unless this is an iOS device recording (which has no audio).
    fn has_audio_tracks(&self) -> bool {
        self.kind != SourceKind::Device
    }

    fn elapsed_with(&self, t0: Option<f64>) -> f64 {
        t0.map_or(0.0, |t0| {
            let now = secs(host_now());
            (now - t0 - self.paused_secs(now)).max(0.0)
        })
    }

    /// Recording time (ms) of a host time, or None if it falls in a pause.
    fn to_ms(&self, host: f64, t0: f64) -> Option<f64> {
        let mut paused = 0.0;
        for &(a, b) in &self.pauses {
            let b = b.unwrap_or(f64::INFINITY);
            if host >= b {
                paused += b - a;
            } else if host >= a {
                return None;
            }
        }
        Some((host - t0 - paused) * 1000.0)
    }

    /// Stop and write the package. Returns its path.
    pub fn stop(mut self) -> Result<PathBuf> {
        if self.is_paused() {
            self.resume();
        }
        let input = self.input.take().map(|i| i.stop()).unwrap_or_default();
        let cam_t0 = self.camera.as_ref().and_then(|c| c.t0());
        if let Some(c) = self.camera.take()
            && let Err(e) = c.finish()
        {
            eprintln!("glide: camera file: {e}");
        }
        if let Some(d) = self.device.take() {
            let t0 = d.t0();
            let duration_ms = self.elapsed_with(t0) * 1000.0;
            d.finish().context("finalizing device recording")?;
            let Some(t0) = t0 else {
                let _ = std::fs::remove_dir_all(&self.package.root);
                bail!("no frames came from the device (is it unlocked?)");
            };
            self.write_package(InputLog::default(), t0, duration_ms, None)?;
            return Ok(self.package.root.clone());
        }
        let session = self.session.take().expect("session");
        let t0 = session.t0_secs();
        let duration_ms = session.recorded_secs() * 1000.0;
        let stats = session.finish();
        let Some(t0) = t0 else {
            let _ = std::fs::remove_dir_all(&self.package.root);
            bail!("no frames were captured (is Screen Recording permission granted?)");
        };
        stats.context("finalizing screen recording")?;
        self.write_package(input, t0, duration_ms, cam_t0)?;
        Ok(self.package.root.clone())
    }

    fn write_package(&self, input: InputLog, t0: f64, duration_ms: f64, cam_t0: Option<f64>) -> Result<()> {
        let pkg = &self.package;
        // Window recordings: origin follows the window as it moves.
        let origin_at = |host: f64| -> (f64, f64) {
            if self.window.is_some()
                && let Some((_, b)) = input.bounds.iter().rev().find(|(h, _)| *h <= host).or(input.bounds.first())
            {
                return (b.x, b.y);
            }
            (self.bounds.x, self.bounds.y)
        };
        // The logger starts before the first video frame: keep the last
        // event from before the start so the cursor has a position at 0.
        let first_inside = input.mouse.iter().position(|m| self.to_ms(m.host, t0).is_some_and(|t| t >= 0.0));
        let lead = first_inside.unwrap_or(input.mouse.len()).saturating_sub(1);
        let mouse: Vec<MouseEvent> = input
            .mouse
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let mut t = self.to_ms(m.host, t0)?;
                if t < 0.0 && i == lead && m.kind != crate::project::recording::MouseKind::Up {
                    t = 0.0;
                }
                if t < 0.0 || t > duration_ms + 50.0 {
                    return None;
                }
                let (ox, oy) = origin_at(m.host);
                Some(MouseEvent {
                    t: t.max(0.0),
                    x: (m.x - ox) * self.scale,
                    y: (m.y - oy) * self.scale,
                    kind: m.kind,
                    button: m.button,
                    cursor: m.cursor,
                })
            })
            .collect();
        let keys: Vec<KeyEvent> = input
            .keys
            .iter()
            .filter_map(|k| {
                let t = self.to_ms(k.host, t0)?;
                (t >= 0.0 && t <= duration_ms).then(|| KeyEvent { t, code: k.code, key: k.key.clone(), mods: k.mods, down: k.down })
            })
            .collect();
        let cursors: Vec<CursorImage> = input.cursors.clone();
        let rec_dir = pkg.recording_dir();
        let has_camera = rec_dir.join("camera.mov").exists();
        let meta = RecordingMeta {
            version: 1,
            kind: self.kind,
            duration_ms,
            screen: VideoTrack {
                file: "screen.mov".into(),
                width: self.size.0 as u32,
                height: self.size.1 as u32,
                fps: self.opts.fps as f64,
                offset_ms: 0.0,
            },
            scale: self.scale,
            bounds: self.bounds,
            camera: has_camera.then(|| VideoTrack {
                file: "camera.mov".into(),
                width: 1280,
                height: 720,
                fps: 30.0,
                offset_ms: cam_t0.map_or(0.0, |c| (c - t0) * 1000.0),
            }),
            // Device recordings carry no audio tracks.
            system_audio: (self.opts.system_audio && self.has_audio_tracks()).then(|| AudioTrack { file: "screen.mov".into(), track: 0 }),
            mic: (self.opts.mic.is_some() && self.has_audio_tracks()).then(|| AudioTrack {
                file: "screen.mov".into(),
                track: if self.opts.system_audio { 1 } else { 0 },
            }),
            app_name: self.app_name.clone(),
            created_ms: now_ms(),
        };
        project::write_json(&pkg.rec("meta.json"), &meta)?;
        project::write_json(&pkg.rec("mouse.json"), &mouse)?;
        project::write_json(&pkg.rec("keys.json"), &keys)?;
        project::write_json(&pkg.rec("cursors.json"), &cursors)?;
        let rec = pkg.load_recording()?;
        let mut proj = project::new_project(&pkg.name(), &rec);
        // The user's default style preset, if any; auto zooms take its zoom level.
        if let Some(style) = project::presets::default_style() {
            project::presets::apply(&mut proj.style, &style);
            for z in &mut proj.timeline.zooms {
                z.scale = proj.style.default_zoom;
            }
        }
        // The camera bubble as set up before recording.
        if let Some(cs) = self.opts.camera_style {
            proj.style.camera = crate::project::model::CameraStyle { zoom_scale: proj.style.camera.zoom_scale, hide: false, ..cs };
        }
        pkg.save_project(&proj)?;
        Ok(())
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}
