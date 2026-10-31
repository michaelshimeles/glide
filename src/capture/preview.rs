//! Live preview of a capture source for the recording-setup canvas: a
//! separate ScreenCaptureKit stream, or an AVCaptureSession for a camera or
//! iPhone, that only keeps the newest frame. Screen previews can also report
//! the level of the audio they'd record.
//!
//! Screens are sharp the way Rec's preview is: captured at their native
//! pixel size (Retina), scaled on the GPU with Metal Performance Shaders'
//! Lanczos to exactly the canvas's size in pixels (`set_target`), and kept as
//! IOSurface-backed pixel buffers the UI draws 1:1 without copies
//! (`take_buffer`). ScreenCaptureKit's own downscale is soft.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_av_foundation::*;
use objc2_core_video::*;
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_metal::*;
use objc2_metal_performance_shaders::{MPSImageEdgeMode, MPSImageLanczosScale, MPSScaleTransform, MPSUnaryImageKernel};
use objc2_core_foundation::CFRetained;
use std::sync::atomic::AtomicU64;

use crate::util::{PIX_BGRA, SendBox, make_pool};
use objc2_screen_capture_kit::*;
use dispatch2::DispatchQueue;

use super::{camera, screen};

/// What to preview (display ids as CoreGraphics reports them; 0 = main).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Display { id: u32 },
    /// A region of a display in points (x, y, w, h, top-left origin).
    Area { display: u32, rect: [f64; 4] },
    Window { id: u32 },
    /// A camera (None = the default one).
    Camera { id: Option<String> },
    /// A USB-connected iPhone or iPad's screen.
    Device { id: String },
}

/// One preview frame, RGBA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

type Callback = Box<dyn Fn() + Send + Sync>;
type Buffer = SendBox<CFRetained<CVPixelBuffer>>;

pub struct Ivars {
    latest: Arc<Mutex<Option<Buffer>>>,
    on_frame: Callback,
    /// Peak audio level (f32 bits, 0..1) since it was last read.
    level: Arc<AtomicU32>,
    /// Canvas size in pixels (w << 32 | h); 0 = keep the frame's size.
    target: Arc<AtomicU64>,
    /// Lanczos scaling (screens only; None = frames as captured).
    scaler: Option<Mutex<Scaler>>,
    /// Device ID if this output previews an iOS device screen.
    ios_device_id: Option<String>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "GlidePreviewOutput"]
    #[ivars = Ivars]
    pub struct PreviewOutput;

    unsafe impl NSObjectProtocol for PreviewOutput {}

    unsafe impl SCStreamOutput for PreviewOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen {
                self.handle(sb);
            } else if kind == SCStreamOutputType::Audio
                && let Some((_, _, frames)) = super::mixer::decode(sb)
            {
                let peak = frames.iter().fold(0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
                // Non-negative f32 bits order like the floats.
                self.ivars().level.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
            }
        }
    }

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for PreviewOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn camera_output(&self, _o: &AVCaptureOutput, sb: &CMSampleBuffer, _c: &AVCaptureConnection) {
            self.handle(sb);
        }
    }
);

impl PreviewOutput {
    fn create(latest: Arc<Mutex<Option<Buffer>>>, level: Arc<AtomicU32>, target: Arc<AtomicU64>, scale: bool, ios_device_id: Option<String>, on_frame: Callback) -> Retained<Self> {
        let scaler = scale.then(Scaler::new).flatten().map(Mutex::new);
        let this = Self::alloc().set_ivars(Ivars { latest, on_frame, level, target, scaler, ios_device_id });
        unsafe { msg_send![super(this), init] }
    }

    fn handle(&self, sb: &CMSampleBuffer) {
        // SAFETY: a valid sample buffer from the capture queue.
        let Some(pb) = (unsafe { sb.is_valid().then(|| sb.image_buffer()).flatten() }) else { return };
        let iv = self.ivars();
        let (fw, fh) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
        if fw < 2 || fh < 2 {
            return;
        }
        if let Some(id) = &iv.ios_device_id {
            camera::record_ios_frame_dimensions(id, fw as u32, fh as u32);
        }
        let t = iv.target.load(Ordering::Relaxed);
        let (w, h) = target_size(fw, fh, ((t >> 32) as u32, t as u32));
        // Scaled into our own pool, which also hands the capture's buffer
        // straight back; unscaled frames are kept as they came.
        let out = match &iv.scaler {
            Some(sc) if (w, h) != (fw, fh) => objc2::rc::autoreleasepool(|_| sc.lock().unwrap_or_else(|e| e.into_inner()).scale(&pb, w, h)),
            _ => Some(pb),
        };
        if let Some(out) = out {
            *iv.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(SendBox(out));
            (iv.on_frame)();
        }
    }
}

/// The pixel size to show a `fw`x`fh` frame at in a canvas of `area` pixels:
/// its aspect ratio fitted into the area, never larger than the frame.
pub fn target_size(fw: usize, fh: usize, area: (u32, u32)) -> (usize, usize) {
    if area.0 < 2 || area.1 < 2 {
        return (fw, fh);
    }
    let s = (area.0 as f64 / fw as f64).min(area.1 as f64 / fh as f64).min(1.0);
    (((fw as f64 * s).round() as usize).max(2) & !1, ((fh as f64 * s).round() as usize).max(2) & !1)
}

/// The long edge of a native-size device preview (`max_px` 0) is capped here.
pub const NATIVE_PREVIEW_EDGE: u32 = 1280;

/// The pixel size to ask a camera or device output for when its format is
/// `dims`: the aspect ratio kept, at most `max_px` on the long edge (0 means
/// native, up to `NATIVE_PREVIEW_EDGE`), never enlarged, each side even and
/// at least 2.
pub fn camera_output_size(dims: (i32, i32), max_px: u32) -> (i64, i64) {
    let cap = if max_px == 0 { NATIVE_PREVIEW_EDGE } else { max_px };
    let scale = (cap as f64 / dims.0.max(dims.1).max(1) as f64).min(1.0);
    let even = |v: i32| ((v as f64 * scale) as i64 / 2 * 2).max(2);
    (even(dims.0), even(dims.1))
}

/// Lanczos downscaling of BGRA frames on the GPU (as Rec scales recordings
/// and its preview), IOSurface to IOSurface.
struct Scaler {
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    cache: CFRetained<CVMetalTextureCache>,
    lanczos: Retained<MPSImageLanczosScale>,
    pool: Option<((usize, usize), CFRetained<CVPixelBufferPool>)>,
}

impl Scaler {
    fn new() -> Option<Scaler> {
        // SAFETY: Metal setup on objects we own.
        unsafe {
            let device = MTLCreateSystemDefaultDevice()?;
            let queue = device.newCommandQueue()?;
            let mut cache = std::ptr::null_mut();
            if CVMetalTextureCache::create(None, None, &device, None, std::ptr::NonNull::from(&mut cache)) != 0 {
                return None;
            }
            let cache = CFRetained::from_raw(std::ptr::NonNull::new(cache)?);
            let lanczos = MPSImageLanczosScale::initWithDevice(MPSImageLanczosScale::alloc(), &device);
            // Taps past the edges repeat the edge pixel (the default, zero,
            // darkens the outer rows and columns).
            lanczos.setEdgeMode(MPSImageEdgeMode::Clamp);
            Some(Scaler { queue, cache, lanczos, pool: None })
        }
    }

    fn texture(&self, buf: &CVPixelBuffer) -> Option<(CFRetained<CVMetalTexture>, Retained<ProtocolObject<dyn MTLTexture>>)> {
        let (w, h) = (CVPixelBufferGetWidth(buf), CVPixelBufferGetHeight(buf));
        // SAFETY: wrapping an IOSurface-backed BGRA buffer.
        unsafe {
            let mut t = std::ptr::null_mut();
            if CVMetalTextureCache::create_texture_from_image(None, &self.cache, buf, None, MTLPixelFormat::BGRA8Unorm, w, h, 0, std::ptr::NonNull::from(&mut t)) != 0 {
                return None;
            }
            let cv = CFRetained::from_raw(std::ptr::NonNull::new(t)?);
            let tex = CVMetalTextureGetTexture(&cv)?;
            Some((cv, tex))
        }
    }

    /// `src` scaled to `w`x`h` (its own aspect ratio) in a new buffer.
    fn scale(&mut self, src: &CVPixelBuffer, w: usize, h: usize) -> Option<CFRetained<CVPixelBuffer>> {
        if CVPixelBufferGetPixelFormatType(src) != PIX_BGRA {
            return None;
        }
        if self.pool.as_ref().is_none_or(|(sz, _)| *sz != (w, h)) {
            self.pool = Some(((w, h), make_pool(w, h, PIX_BGRA)));
        }
        let pool = &self.pool.as_ref()?.1;
        // SAFETY: GPU work on buffers we hold until it completes.
        unsafe {
            let mut b = std::ptr::null_mut();
            CVPixelBufferPool::create_pixel_buffer(None, pool, std::ptr::NonNull::from(&mut b));
            let out = CFRetained::from_raw(std::ptr::NonNull::new(b)?);
            let (st, dt) = (self.texture(src)?, self.texture(&out)?);
            let (sw, sh) = (CVPixelBufferGetWidth(src) as f64, CVPixelBufferGetHeight(src) as f64);
            let t = MPSScaleTransform { scaleX: w as f64 / sw, scaleY: h as f64 / sh, translateX: 0.0, translateY: 0.0 };
            self.lanczos.setScaleTransform(&t);
            let cmd = self.queue.commandBuffer()?;
            let kernel: &MPSUnaryImageKernel = &self.lanczos;
            kernel.encodeToCommandBuffer_sourceTexture_destinationTexture(&cmd, &st.1, &dt.1);
            cmd.commit();
            cmd.waitUntilCompleted();
            drop((st, dt));
            self.cache.flush(0);
            (cmd.status() == MTLCommandBufferStatus::Completed).then_some(out)
        }
    }
}

enum Capture {
    Screen(Retained<SCStream>),
    Camera(Retained<AVCaptureSession>),
}

/// A running preview stream; stops when dropped.
pub struct LivePreview {
    /// First, so the observers go before the session does.
    _watch: Option<camera::SessionWatch>,
    /// A runtime error or disconnect of an iPhone/iPad session, until read.
    runtime_error: Arc<Mutex<Option<String>>>,
    capture: Capture,
    _output: Retained<PreviewOutput>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
    latest: Arc<Mutex<Option<Buffer>>>,
    level: Arc<AtomicU32>,
    target: Arc<AtomicU64>,
}

// SAFETY: the stream is only started and stopped through thread-safe
// ScreenCaptureKit calls; frames arrive on its own queue.
unsafe impl Send for LivePreview {}

impl LivePreview {
    /// Start previewing `source` at `fps` frames per second, calling
    /// `on_frame` (from a capture queue) after each new frame. Screens are
    /// captured at their native pixel size and scaled to `set_target`, or
    /// with `max_px` > 0 captured at most that long edge and not rescaled;
    /// cameras and iPhones to at most `max_px` on the long edge, where 0
    /// means native up to `NATIVE_PREVIEW_EDGE`. Glide's own windows are
    /// left out. With `audio`, a screen preview also measures the system
    /// audio it would record (`level`).
    pub fn start(source: &Source, max_px: u32, fps: u32, audio: bool, on_frame: impl Fn() + Send + Sync + 'static) -> Result<LivePreview> {
        match source {
            Source::Camera { id } => return Self::start_camera(camera::find_camera(id.as_deref())?, max_px, None, on_frame),
            Source::Device { id } => {
                // The one place that may ask, on the Slot's background thread.
                super::tcc::require_camera(std::time::Duration::from_secs(60))?;
                return Self::start_camera(camera::find_ios(id)?, max_px, Some(id.clone()), on_frame);
            }
            _ => {}
        }
        let displays = screen::displays()?;
        let display_for = |id: u32| displays.iter().find(|d| d.id == id || id == 0).context("display not found");
        let backing = |d: &screen::DisplayInfo| (d.pixels.0 as f64 / d.points.0.max(1) as f64).max(1.0);
        // SAFETY: ScreenCaptureKit configuration on objects we own.
        unsafe {
            let conf = SCStreamConfiguration::new();
            // Size in points and the display's pixels per point.
            let (filter, size, px) = match source {
                Source::Display { id } | Source::Area { display: id, .. } => {
                    let d = display_for(*id)?;
                    // Our own windows stay out, by application so windows
                    // opened later (the full-screen titlebar) do too.
                    let filter = match screen::own_app()? {
                        Some(app) => SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                            SCContentFilter::alloc(),
                            &d.display,
                            &NSArray::from_retained_slice(&[app]),
                            &NSArray::new(),
                        ),
                        None => SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &d.display, &NSArray::new()),
                    };
                    let size = match source {
                        Source::Area { rect, .. } => {
                            conf.setSourceRect(CGRect::new(CGPoint::new(rect[0], rect[1]), CGSize::new(rect[2], rect[3])));
                            (rect[2], rect[3])
                        }
                        _ => (d.points.0 as f64, d.points.1 as f64),
                    };
                    (filter, size, backing(d))
                }
                Source::Window { id } => {
                    let w = screen::windows()?.into_iter().find(|w| w.id == *id).context("that window is no longer on screen")?;
                    let on = w.display.as_ref().map(|d| d.displayID());
                    let px = displays.iter().find(|d| Some(d.id) == on).or(displays.first()).map_or(2.0, backing);
                    (SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &w.window), w.size, px)
                }
                Source::Camera { .. } | Source::Device { .. } => unreachable!(),
            };
            if size.0 < 2.0 || size.1 < 2.0 {
                bail!("nothing to preview");
            }
            // Native pixels (Retina), as Rec captures; scaled down later. With
            // `max_px`, at most that long edge, scaled by ScreenCaptureKit and
            // from point-sized frames when that's enough (light mode).
            let cap = if max_px > 0 { (max_px as f64 / (size.0.max(size.1) * px)).min(1.0) } else { 1.0 };
            let even = |v: f64| ((v * px * cap).round() as usize / 2 * 2).max(2);
            conf.setWidth(even(size.0));
            conf.setHeight(even(size.1));
            let nominal = max_px > 0 && even(size.0) as f64 <= size.0;
            conf.setCaptureResolution(if nominal { SCCaptureResolutionType::Nominal } else { SCCaptureResolutionType::Best });
            conf.setColorSpaceName(objc2_core_graphics::kCGColorSpaceSRGB);
            conf.setMinimumFrameInterval(CMTime::new(1, fps.max(1) as i32));
            conf.setPixelFormat(kCVPixelFormatType_32BGRA);
            // Room for the frame the UI holds while the next one arrives.
            conf.setQueueDepth(5);
            conf.setShowsCursor(true);
            conf.setScalesToFit(true);
            conf.setPreservesAspectRatio(true);
            conf.setCapturesAudio(audio);
            conf.setExcludesCurrentProcessAudio(true);

            let latest = Arc::new(Mutex::new(None));
            let level = Arc::new(AtomicU32::new(0));
            let target = Arc::new(AtomicU64::new(0));
            let output = PreviewOutput::create(latest.clone(), level.clone(), target.clone(), max_px == 0, None, Box::new(on_frame));
            let stream = SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &conf, None);
            let queue = DispatchQueue::new("glide.preview", None);
            let proto: &ProtocolObject<dyn SCStreamOutput> = ProtocolObject::from_ref(&*output);
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Screen, Some(&queue))
                .map_err(|e| anyhow::anyhow!("preview output: {}", e.localizedDescription()))?;
            if audio {
                stream
                    .addStreamOutput_type_sampleHandlerQueue_error(proto, SCStreamOutputType::Audio, Some(&queue))
                    .map_err(|e| anyhow::anyhow!("preview audio: {}", e.localizedDescription()))?;
            }
            wait(|b| stream.startCaptureWithCompletionHandler(Some(b))).context("starting the preview")?;
            Ok(LivePreview { _watch: None, runtime_error: Arc::default(), capture: Capture::Screen(stream), _output: output, _queue: queue, latest, level, target })
        }
    }

    /// Preview a camera (720p preset) or an iPhone/iPad (`ios_id`, the device's
    /// own format) through its own capture session.
    fn start_camera(dev: Retained<AVCaptureDevice>, max_px: u32, ios_id: Option<String>, on_frame: impl Fn() + Send + Sync + 'static) -> Result<LivePreview> {
        // SAFETY: AVFoundation configuration on objects we own.
        unsafe {
            let ios = ios_id.is_some();
            let name = dev.localizedName().to_string();
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if ios {
                camera::use_native_format(&session);
            } else if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                session.setSessionPreset(AVCaptureSessionPreset1280x720);
            }
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&dev)
                .map_err(|e| anyhow::anyhow!("{name}: {}", e.localizedDescription()))?;
            if !session.canAddInput(&input) {
                bail!("cannot use {name}");
            }
            session.addInput(&input);
            let out = AVCaptureVideoDataOutput::new();
            // Small frames: the output scales (keeping the aspect ratio).
            // For iOS devices, keep preview frames native so delivered sizes match recording.
            let dims = dev.activeFormat().formatDescription();
            let mut d = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&dims);
            if (d.width == 0 || d.height == 0)
                && let Some((dw, dh)) = camera::device_dimensions(&dev)
            {
                d = objc2_core_media::CMVideoDimensions { width: dw as i32, height: dh as i32 };
            }
            let mut settings = vec![
                (crate::util::cf_ns(kCVPixelBufferPixelFormatTypeKey), crate::util::num_i(crate::util::PIX_BGRA as i64)),
            ];
            if !ios && d.width > 0 && d.height > 0 {
                let (w, h) = camera_output_size((d.width, d.height), max_px);
                settings.push((crate::util::cf_ns(kCVPixelBufferWidthKey), crate::util::num_i(w)));
                settings.push((crate::util::cf_ns(kCVPixelBufferHeightKey), crate::util::num_i(h)));
            }
            out.setVideoSettings(Some(&crate::util::dict(settings)));
            out.setAlwaysDiscardsLateVideoFrames(true);
            let latest = Arc::new(Mutex::new(None));
            let target = Arc::new(AtomicU64::new(0));
            // Also called when the session reports an error, so the UI looks.
            let on_frame = Arc::new(on_frame);
            let wake = on_frame.clone();
            let output = PreviewOutput::create(latest.clone(), Arc::new(AtomicU32::new(0)), target.clone(), ios, ios_id, Box::new(move || on_frame()));
            let queue = DispatchQueue::new("glide.preview.camera", None);
            out.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*output)), Some(&queue));
            if !session.canAddOutput(&out) {
                bail!("cannot preview {name}");
            }
            session.addOutput(&out);
            session.commitConfiguration();
            let runtime_error = Arc::new(Mutex::new(None));
            // Only iPhones are watched: a webcam preview that stops just
            // freezes, as before.
            let watch = ios.then(|| {
                let (slot, name) = (runtime_error.clone(), name.clone());
                camera::SessionWatch::new(&session, &dev, camera::IOS_DISCONNECTED, Arc::new(move |m| {
                    crate::diag::note(&format!("glide: ios preview {name}: {m}"));
                    slot.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(m);
                    wake();
                }))
            });
            session.startRunning();
            if !session.isRunning() {
                bail!("{name} failed to start");
            }
            Ok(LivePreview { _watch: watch, runtime_error, capture: Capture::Camera(session), _output: output, _queue: queue, latest, level: Arc::new(AtomicU32::new(0)), target })
        }
    }

    /// The runtime error or disconnect an iPhone/iPad session reported since
    /// the last call.
    pub fn take_runtime_error(&self) -> Option<String> {
        self.runtime_error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Peak audio level (0..1) since the last call; 0 without audio.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }

    /// Show screen frames at `w`x`h` pixels at most (the canvas; aspect
    /// ratio kept, never enlarged). 0 keeps them at their native size.
    pub fn set_target(&self, w: u32, h: u32) {
        self.target.store((w as u64) << 32 | h as u64, Ordering::Relaxed);
    }

    /// The newest frame since the last call: an IOSurface-backed BGRA
    /// buffer, for drawing without copies.
    pub fn take_buffer(&self) -> Option<CFRetained<CVPixelBuffer>> {
        self.latest.lock().unwrap_or_else(|e| e.into_inner()).take().map(|b| b.0)
    }

    /// The newest frame since the last call, copied out as RGBA.
    pub fn take_frame(&self) -> Option<Frame> {
        let pb = self.take_buffer()?;
        // SAFETY: the buffer is locked read-only while copied.
        unsafe {
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
            let stride = CVPixelBufferGetBytesPerRow(&pb);
            let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
            let frame = (!base.is_null() && w > 0 && h > 0).then(|| {
                let src = std::slice::from_raw_parts(base, stride * h);
                let mut rgba = Vec::with_capacity(w * h * 4);
                for row in src.chunks(stride).take(h) {
                    for px in row[..w * 4].chunks_exact(4) {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    }
                }
                Frame { width: w as u32, height: h as u32, rgba }
            });
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            frame
        }
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        match &self.capture {
            // SAFETY: stopping a stream we started; errors (already stopped) are fine.
            Capture::Screen(stream) => {
                let _ = wait(|b| unsafe { stream.stopCaptureWithCompletionHandler(Some(b)) });
            }
            // SAFETY: stopping a session we started.
            Capture::Camera(session) => unsafe { session.stopRunning() },
        }
    }
}

fn wait(f: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>)) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let block = block2::RcBlock::new(move |err: *mut NSError| {
        // SAFETY: ScreenCaptureKit passes a valid error or null.
        let r = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.send(r);
    });
    f(&block);
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(None) => Ok(()),
        Ok(Some(e)) => bail!("{e}"),
        Err(_) => bail!("timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size math as `start_camera` had it before `camera_output_size`
    /// (it turned `max_px` 0 into 2x2).
    fn old(dims: (i32, i32), max_px: u32) -> (i64, i64) {
        let scale = (max_px as f64 / dims.0.max(dims.1).max(1) as f64).min(1.0);
        let even = |v: i32| ((v as f64 * scale) as i64 / 2 * 2).max(2);
        (even(dims.0), even(dims.1))
    }

    fn long(s: (i64, i64)) -> i64 {
        s.0.max(s.1)
    }

    #[test]
    fn camera_output_size_native_is_capped_at_1280_not_2x2() {
        let s = camera_output_size((1170, 2532), 0);
        assert!((1278..=1280).contains(&long(s)), "{s:?}");
        assert!(s.0 % 2 == 0 && s.1 % 2 == 0);
        // Aspect ratio within a pixel of 1170:2532.
        assert!((s.1 as f64 * 1170.0 / 2532.0 - s.0 as f64).abs() <= 2.0, "{s:?}");
        let land = camera_output_size((2532, 1170), 0);
        assert_eq!((land.0, land.1), (s.1, s.0));
    }

    #[test]
    fn camera_output_size_honours_max_px() {
        assert!((638..=640).contains(&long(camera_output_size((1170, 2532), 640))));
        assert!((718..=720).contains(&long(camera_output_size((1170, 2532), 720))));
        let land = camera_output_size((2532, 1170), 720);
        assert!((718..=720).contains(&land.0) && land.0 > land.1);
    }

    #[test]
    fn camera_output_size_keeps_webcam_sizes_unchanged() {
        for dims in [(1280, 720), (1920, 1080), (640, 480), (1280, 960), (3840, 2160)] {
            assert_eq!(camera_output_size(dims, 720), old(dims, 720), "{dims:?}");
        }
    }

    #[test]
    fn camera_output_size_makes_odd_sizes_even() {
        for max in [0, 640, 720] {
            let s = camera_output_size((1001, 1999), max);
            assert!(s.0 % 2 == 0 && s.1 % 2 == 0 && s.0 >= 2 && s.1 >= 2, "{max}: {s:?}");
        }
    }

    #[test]
    fn camera_output_size_never_enlarges() {
        assert_eq!(camera_output_size((640, 480), 0), (640, 480));
        assert_eq!(camera_output_size((640, 480), 720), (640, 480));
    }

    #[test]
    fn camera_output_size_survives_zero_dimensions() {
        assert_eq!(camera_output_size((0, 0), 0), (2, 2));
        assert_eq!(camera_output_size((0, 0), 720), (2, 2));
    }
}
