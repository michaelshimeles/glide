//! ScreenCaptureKit: display video, system audio, and microphone.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, kCGColorSpaceSRGB, kCGDisplayStreamYCbCrMatrix_ITU_R_709_2};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::*;
use dispatch2::DispatchQueue;

use crate::capture::record::Shared;
use crate::util::*;
use crate::capture::writer::AudioKind;

pub struct DisplayInfo {
    pub id: u32,
    pub points: (i64, i64),
    pub pixels: (usize, usize),
    pub display: Retained<SCDisplay>,
}

fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = if let Some(c) = unsafe { Retained::retain(content) } {
            Ok(SendBox(c))
        } else {
            let msg = unsafe { err.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "unknown error".into());
            Err(msg)
        };
        let _ = tx.send(r);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(c)) => Ok(c.0),
        Ok(Err(e)) => bail!(
            "ScreenCaptureKit refused: {e}\n  -> grant Screen & System Audio Recording to DaisyCap (see `daisycap permissions`)"
        ),
        Err(_) => bail!("timed out waiting for ScreenCaptureKit shareable content"),
    }
}

/// This process as ScreenCaptureKit sees it (None before it has a window), to
/// exclude every window it owns, including ones it opens later (AppKit's
/// full-screen titlebar window, menus, popovers).
pub fn own_app() -> Result<Option<Retained<SCRunningApplication>>> {
    let me = std::process::id() as i32;
    Ok(unsafe { shareable_content()?.applications() }.iter().find(|a| unsafe { a.processID() } == me))
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    let content = shareable_content()?;
    let mut out = vec![];
    for d in unsafe { content.displays() }.iter() {
        let id = unsafe { d.displayID() };
        let mode = CGDisplayCopyDisplayMode(id);
        let pixels = (
            CGDisplayMode::pixel_width(mode.as_deref()),
            CGDisplayMode::pixel_height(mode.as_deref()),
        );
        out.push(DisplayInfo { id, points: unsafe { (d.width() as i64, d.height() as i64) }, pixels, display: d });
    }
    Ok(out)
}

pub struct WindowInfo {
    pub id: u32,
    pub app: String,
    pub title: String,
    /// Size in points.
    pub size: (f64, f64),
    pub window: Retained<SCWindow>,
    /// The display it's on (from the same lookup), for system audio.
    pub display: Option<Retained<SCDisplay>>,
}

/// On-screen, normal-layer windows that have a title or an owning app.
pub fn windows() -> Result<Vec<WindowInfo>> {
    let content = shareable_content()?;
    let displays = unsafe { content.displays() };
    let mut out = vec![];
    for w in unsafe { content.windows() }.iter() {
        unsafe {
            if !w.isOnScreen() || w.windowLayer() != 0 {
                continue;
            }
            let app = w.owningApplication().map(|a| a.applicationName().to_string()).unwrap_or_default();
            let title = w.title().map(|t| t.to_string()).unwrap_or_default();
            let f = w.frame();
            if (app.is_empty() && title.is_empty()) || f.size.width < 40.0 || f.size.height < 40.0 {
                continue;
            }
            let (cx, cy) = (f.origin.x + f.size.width / 2.0, f.origin.y + f.size.height / 2.0);
            let display = displays
                .iter()
                .find(|d| {
                    let r = d.frame();
                    (r.origin.x..r.origin.x + r.size.width).contains(&cx) && (r.origin.y..r.origin.y + r.size.height).contains(&cy)
                })
                .or_else(|| displays.firstObject());
            out.push(WindowInfo { id: w.windowID(), app, title, size: (f.size.width, f.size.height), window: w, display });
        }
    }
    Ok(out)
}

/// Find a window by numeric ID, or case-insensitive substring of
/// "<app> <title>". Prefers the largest match.
pub fn find_window(sel: &str) -> Result<WindowInfo> {
    // An empty selector would match every window.
    if sel.trim().is_empty() {
        bail!("no window given (use an id from `daisycap devices` or part of its name)");
    }
    let ws = windows()?;
    if let Ok(id) = sel.parse::<u32>() {
        if let Some(w) = ws.into_iter().find(|w| w.id == id) {
            return Ok(w);
        }
        bail!("no on-screen window with id {id} (see `daisycap devices`)");
    }
    let lower = sel.to_lowercase();
    ws.into_iter()
        .filter(|w| format!("{} {}", w.app, w.title).to_lowercase().contains(&lower))
        .max_by(|a, b| (a.size.0 * a.size.1).total_cmp(&(b.size.0 * b.size.1)))
        .ok_or_else(|| anyhow!("no on-screen window matches {sel:?} (see `daisycap devices`)"))
}

/// ScreenCaptureKit captures the microphone itself only on macOS 15+.
pub fn captures_microphone() -> bool {
    unsafe { SCStreamConfiguration::new() }.respondsToSelector(sel!(setCaptureMicrophone:))
}

/// CGWindowLevel of the Dock (kCGDockWindowLevel).
const DOCK_LAYER: isize = 20;

pub enum Target {
    /// A display, optionally cropped to a region (points, top-left origin).
    Display(DisplayInfo, Option<objc2_core_foundation::CGRect>),
    Window(WindowInfo),
}

impl Target {
    pub fn describe(&self) -> String {
        match self {
            Target::Display(d, None) => format!("display {}", d.id),
            Target::Display(d, Some(r)) => format!(
                "display {} region {}x{}+{}+{}",
                d.id, r.size.width, r.size.height, r.origin.x, r.origin.y
            ),
            Target::Window(w) => format!("window \"{} {}\" ({})", w.app, w.title, w.id),
        }
    }
}

pub struct ScreenConfig {
    pub target: Target,
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    pub pixel_format: u32,
    pub cursor: bool,
    pub clicks: bool,
    pub system_audio: bool,
    /// None = no mic; Some(None) = default mic; Some(Some(id)) = specific device.
    pub mic: Option<Option<String>>,
    pub hide_desktop_icons: bool,
    pub hide_dock: bool,
    pub exclude_own_audio: bool,
}

pub struct Ivars {
    shared: Arc<Shared>,
    /// The audio-only companion stream: its (tiny) video frames are dropped.
    audio_only: bool,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "GlideScreenOutput"]
    #[ivars = Ivars]
    pub struct ScreenOutput;

    unsafe impl NSObjectProtocol for ScreenOutput {}

    unsafe impl SCStreamOutput for ScreenOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
            self.handle(sb, kind);
        }
    }

    unsafe impl SCStreamDelegate for ScreenOutput {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let msg = format!("screen capture stopped: {} [{} {}]", error.localizedDescription(), error.domain(), error.code());
            self.ivars().shared.fail(msg);
        }
    }
);

impl ScreenOutput {
    fn create(shared: Arc<Shared>, audio_only: bool) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { shared, audio_only });
        unsafe { msg_send![super(this), init] }
    }

    fn handle(&self, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
        unsafe {
            if !sb.is_valid() {
                return;
            }
        }
        let shared = &self.ivars().shared;
        if kind == SCStreamOutputType::Screen {
            if self.ivars().audio_only || !frame_complete(sb) {
                return;
            }
            let Some(pb) = (unsafe { sb.image_buffer() }) else { return };
            let pts: CMTime = unsafe { sb.presentation_time_stamp() };
            let pb: objc2_core_foundation::CFRetained<CVPixelBuffer> = pb;
            shared.with(|s| s.push_screen(pts, pb));
        } else if kind == SCStreamOutputType::Audio {
            if std::env::var_os("REC_DEBUG").is_some() {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n.is_multiple_of(25) {
                    let pts = unsafe { sb.presentation_time_stamp() };
                    let peak = crate::capture::mixer::decode(sb).map_or(-1.0, |(_, _, f)| f.iter().fold(0f32, |m, x| m.max(x[0].abs())));
                    let now = crate::util::secs(crate::util::host_now());
                    eprintln!("daisycap: debug: system audio buffer {n} (companion={}) pts={:.3} now={now:.3} peak={peak:.3}", self.ivars().audio_only, pts.value as f64 / pts.timescale as f64);
                }
            }
            shared.with(|s| s.push_audio(AudioKind::System, sb));
        } else if kind == SCStreamOutputType::Microphone {
            shared.with(|s| s.push_audio(AudioKind::Mic, sb));
        }
    }
}

/// Only `Complete` frames carry new pixels; idle/blank/suspended do not.
fn frame_complete(sb: &CMSampleBuffer) -> bool {
    unsafe {
        let Some(arr) = sb.sample_attachments_array(false) else { return false };
        let arr: &NSArray<NSDictionary<NSString, AnyObject>> =
            &*(&*arr as *const objc2_core_foundation::CFArray as *const NSArray<NSDictionary<NSString, AnyObject>>);
        let Some(info) = arr.firstObject() else { return false };
        let Some(status) = info.objectForKey(SCStreamFrameInfoStatus) else { return false };
        let Ok(n) = status.downcast::<NSNumber>() else { return false };
        n.integerValue() == SCFrameStatus::Complete.0
    }
}

pub struct ScreenCapture {
    stream: Retained<SCStream>,
    /// System audio for window recordings (see `audio_stream`).
    audio: Option<AudioStream>,
    _output: Retained<ScreenOutput>,
    _queues: Vec<dispatch2::DispatchRetained<DispatchQueue>>,
}

unsafe impl Send for ScreenCapture {}

impl ScreenCapture {
    pub fn start(cfg: &ScreenConfig, shared: Arc<Shared>) -> Result<Self> {
        unsafe {
            let conf = SCStreamConfiguration::new();
            let filter = match &cfg.target {
                Target::Display(d, region) => {
                    if let Some(r) = region {
                        conf.setSourceRect(*r);
                    }
                    // Exclude our own windows (recording widget, editor), and
                    // optionally the desktop icons and the Dock, by window so
                    // the wallpaper (also owned by the Dock) stays.
                    let me = std::process::id() as i32;
                    let content = shareable_content().ok();
                    let excluded: Vec<Retained<SCWindow>> = content
                        .as_ref()
                        .map(|c| {
                            c.windows()
                                .iter()
                                .filter(|w| {
                                    let Some(app) = w.owningApplication() else { return false };
                                    let bundle = app.bundleIdentifier().to_string();
                                    let layer = w.windowLayer();
                                    app.processID() == me
                                        || (cfg.hide_desktop_icons && bundle == "com.apple.finder" && layer < 0)
                                        || (cfg.hide_dock && bundle == "com.apple.dock" && layer == DOCK_LAYER)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if std::env::var_os("REC_DEBUG").is_some() {
                        eprintln!("daisycap: debug: excluding {} windows from capture", excluded.len());
                    }
                    SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        &d.display,
                        &NSArray::from_retained_slice(&excluded),
                    )
                }
                Target::Window(w) => {
                    SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w.window)
                }
            };
            conf.setWidth(cfg.width);
            conf.setHeight(cfg.height);
            // Slightly under the frame period, so capture jitter never makes
            // ScreenCaptureKit skip a frame the pacer needs (as OBS does).
            // Light mode takes exactly the frame rate: fewer frames to
            // composite and deliver; a late one repeats the previous frame.
            if crate::hw::light() {
                conf.setMinimumFrameInterval(CMTime::new(1, cfg.fps as i32));
            } else {
                conf.setMinimumFrameInterval(CMTime::new(9, cfg.fps as i32 * 10));
            }
            conf.setPixelFormat(cfg.pixel_format);
            conf.setQueueDepth(8);
            conf.setShowsCursor(cfg.cursor);
            // showMouseClicks is macOS 15+; earlier systems have no selector.
            if conf.respondsToSelector(sel!(setShowMouseClicks:)) {
                conf.setShowMouseClicks(cfg.clicks);
            }
            conf.setScalesToFit(true);
            conf.setPreservesAspectRatio(true);
            conf.setCaptureResolution(SCCaptureResolutionType::Best);
            conf.setColorSpaceName(kCGColorSpaceSRGB);
            if cfg.pixel_format == PIX_420V {
                conf.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
            }
            // A window filter only hears the window's own app, so window
            // recordings take the Mac's audio from a companion stream.
            let window = matches!(cfg.target, Target::Window(_));
            let main_audio = cfg.system_audio && !window;
            conf.setCapturesAudio(main_audio);
            conf.setSampleRate(48000);
            conf.setChannelCount(2);
            conf.setExcludesCurrentProcessAudio(cfg.exclude_own_audio);
            if let Some(dev) = &cfg.mic {
                conf.setCaptureMicrophone(true);
                if let Some(id) = dev {
                    conf.setMicrophoneCaptureDeviceID(Some(&NSString::from_str(id)));
                }
            }

            let audio = match &cfg.target {
                Target::Window(w) if cfg.system_audio => {
                    let display = w.display.as_ref().context("no display to take system audio from")?;
                    Some(audio_stream(display, cfg.exclude_own_audio, shared.clone())?)
                }
                _ => None,
            };
            let output = ScreenOutput::create(shared, false);
            let delegate = ProtocolObject::from_ref(&*output);
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &conf, Some(delegate));
            let out_proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
            let vq = DispatchQueue::new("rec.screen.video", None);
            let aq = DispatchQueue::new("rec.screen.audio", None);
            let mq = DispatchQueue::new("rec.screen.mic", None);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(out_proto, SCStreamOutputType::Screen, Some(&vq))
                .map_err(|e| anyhow!("add screen output: {}", e.localizedDescription()))?;
            if main_audio {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(out_proto, SCStreamOutputType::Audio, Some(&aq))
                    .map_err(|e| anyhow!("add audio output: {}", e.localizedDescription()))?;
            }
            if cfg.mic.is_some() {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(out_proto, SCStreamOutputType::Microphone, Some(&mq))
                    .map_err(|e| anyhow!("add microphone output: {}", e.localizedDescription()))?;
            }
            if let Some(a) = &audio {
                a.start()?;
            }
            if let Err(e) = wait_err(|b| stream.startCaptureWithCompletionHandler(Some(b))) {
                if let Some(a) = &audio {
                    let _ = a.stop();
                }
                return Err(e).context("starting screen capture");
            }
            Ok(ScreenCapture { stream, audio, _output: output, _queues: vec![vq, aq, mq] })
        }
    }

    pub fn stop(&self) -> Result<()> {
        let audio = self.audio.as_ref().map(AudioStream::stop);
        unsafe { wait_err(|b| self.stream.stopCaptureWithCompletionHandler(Some(b)))? };
        audio.unwrap_or(Ok(()))
    }
}

struct AudioStream {
    stream: Retained<SCStream>,
    _output: Retained<ScreenOutput>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
}

impl AudioStream {
    fn start(&self) -> Result<()> {
        unsafe { wait_err(|b| self.stream.startCaptureWithCompletionHandler(Some(b))) }.context("starting system audio capture")
    }

    fn stop(&self) -> Result<()> {
        unsafe { wait_err(|b| self.stream.stopCaptureWithCompletionHandler(Some(b))) }
    }
}

/// An audio-only stream over a whole display: everything the Mac plays
/// (other than DaisyCap itself), whatever the video source. Its video is the
/// smallest ScreenCaptureKit allows and is dropped.
fn audio_stream(display: &SCDisplay, exclude_own_audio: bool, shared: Arc<Shared>) -> Result<AudioStream> {
    unsafe {
        let filter = SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &NSArray::new());
        let conf = SCStreamConfiguration::new();
        conf.setWidth(2);
        conf.setHeight(2);
        // Its frames are thrown away: as few as possible, without a cursor.
        conf.setMinimumFrameInterval(CMTime::new(1, 2));
        conf.setShowsCursor(false);
        conf.setCapturesAudio(true);
        conf.setSampleRate(48000);
        conf.setChannelCount(2);
        conf.setExcludesCurrentProcessAudio(exclude_own_audio);
        let output = ScreenOutput::create(shared, true);
        let delegate = ProtocolObject::from_ref(&*output);
        let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &conf, Some(delegate));
        let proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
        let aq = DispatchQueue::new("rec.screen.system-audio", None);
        // A screen output too, so ScreenCaptureKit has somewhere to put
        // the frames it insists on making.
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Screen, Some(&aq))
            .map_err(|e| anyhow!("add screen output: {}", e.localizedDescription()))?;
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Audio, Some(&aq))
            .map_err(|e| anyhow!("add audio output: {}", e.localizedDescription()))?;
        Ok(AudioStream { stream, _output: output, _queue: aq })
    }
}

fn wait_err(f: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>)) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |err: *mut NSError| {
        let r = match unsafe { err.as_ref() } {
            None => Ok(()),
            Some(e) => Err(format!("{} [{} {}]", e.localizedDescription(), e.domain(), e.code())),
        };
        let _ = tx.send(r);
    });
    f(&block);
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => bail!("{e}"),
        Err(_) => bail!("timed out"),
    }
}
