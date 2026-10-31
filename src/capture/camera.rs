//! Webcam capture (AVCaptureSession) and device enumeration.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::*;
use objc2_core_media::CMSampleBuffer;
use objc2_core_video::{CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelBufferPixelFormatTypeKey};
use objc2_foundation::{NSArray, NSError, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol};
use dispatch2::DispatchQueue;

use crate::capture::record::Shared;
use crate::util::*;

pub struct Device {
    pub id: String,
    pub name: String,
}

fn discover(types: &[&AVCaptureDeviceType], media: &AVMediaType) -> Vec<Retained<AVCaptureDevice>> {
    unsafe {
        let arr = NSArray::from_slice(types);
        let s = AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &arr,
            Some(media),
            AVCaptureDevicePosition::Unspecified,
        );
        s.devices().iter().collect()
    }
}

fn cameras_raw() -> Vec<Retained<AVCaptureDevice>> {
    unsafe {
        discover(
            &[
                AVCaptureDeviceTypeBuiltInWideAngleCamera,
                AVCaptureDeviceTypeExternal,
                AVCaptureDeviceTypeContinuityCamera,
            ],
            AVMediaTypeVideo.unwrap(),
        )
    }
}

fn describe(d: &AVCaptureDevice) -> Device {
    unsafe { Device { id: d.uniqueID().to_string(), name: d.localizedName().to_string() } }
}

#[repr(C)]
struct CmioPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreMediaIO", kind = "framework")]
unsafe extern "C" {
    fn CMIOObjectSetPropertyData(
        object: u32,
        address: *const CmioPropertyAddress,
        qualifier_size: u32,
        qualifier: *const std::ffi::c_void,
        data_size: u32,
        data: *const std::ffi::c_void,
    ) -> i32;
}

/// Let USB-connected iPhones and iPads show up as capture devices (their
/// screens become "muxed" AVCaptureDevices a moment later).
pub fn enable_screen_capture_devices() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let address = CmioPropertyAddress {
            selector: u32::from_be_bytes(*b"yes "), // kCMIOHardwarePropertyAllowScreenCaptureDevices
            scope: u32::from_be_bytes(*b"glob"),
            element: 0,
        };
        let allow: u32 = 1;
        // SAFETY: documented CoreMediaIO system-object property, plain data.
        unsafe {
            CMIOObjectSetPropertyData(1, &address, 0, std::ptr::null(), 4, &allow as *const u32 as *const std::ffi::c_void);
        }
        // The system only starts announcing screens after a discovery query
        // has run once; this one's result is dropped.
        let _ = ios_query();
    });
}

/// One muxed-device discovery, without the opt-in (which calls this).
fn ios_query() -> Vec<Retained<AVCaptureDevice>> {
    unsafe { discover(&[AVCaptureDeviceTypeExternal], AVMediaTypeMuxed.unwrap()) }
}

fn ios_raw() -> Vec<Retained<AVCaptureDevice>> {
    enable_screen_capture_devices();
    ios_query()
}

/// How long an iPhone/iPad lookup waits for the device to show up, and the
/// pause between tries. The opt-in is asynchronous: the screen appears a
/// moment after it, not before the first query returns.
pub const IOS_WAIT: Duration = Duration::from_secs(3);
pub const IOS_POLL: Duration = Duration::from_millis(150);

/// Try `probe`, then retry after each `step` pause until it returns a value
/// or the pauses add up past `deadline`. Returns the value, the number of
/// tries and the time spent in pauses. `sleep` does the pausing, so tests
/// need no clock.
pub fn poll_until<T>(
    deadline: Duration,
    step: Duration,
    mut probe: impl FnMut() -> Option<T>,
    mut sleep: impl FnMut(Duration),
) -> (Option<T>, u32, Duration) {
    let (mut tries, mut waited) = (0u32, Duration::ZERO);
    loop {
        tries += 1;
        if let Some(v) = probe() {
            return (Some(v), tries, waited);
        }
        if step.is_zero() || waited + step > deadline {
            return (None, tries, waited);
        }
        sleep(step);
        waited += step;
    }
}

fn names(list: &[Device]) -> String {
    list.iter().map(|d| format!("{} ({})", d.name, d.id)).collect::<Vec<_>>().join(", ")
}

/// The iOS device `id`, waiting up to `IOS_WAIT` for it to be discovered.
fn wait_ios(id: &str) -> Option<Retained<AVCaptureDevice>> {
    let started = Instant::now();
    let mut last = vec![];
    let (found, tries, _) = poll_until(
        IOS_WAIT,
        IOS_POLL,
        || {
            let all = ios_raw();
            let hit = all.iter().find(|d| unsafe { d.uniqueID().to_string() } == id).cloned();
            if hit.is_none() {
                last = all.iter().map(|d| describe(d)).collect();
            }
            hit
        },
        std::thread::sleep,
    );
    let ms = started.elapsed().as_millis();
    match &found {
        Some(_) => crate::diag::note(&format!("glide: ios discovery: {id} found after {tries} tries ({ms} ms)")),
        None => crate::diag::note(&format!("glide: ios discovery: {id} not found after {tries} tries ({ms} ms); listed: [{}]", names(&last))),
    }
    found
}

/// Connected iPhones/iPads whose screens can be recorded (one query, no waiting).
pub fn ios_devices() -> Vec<Device> {
    ios_raw().iter().map(|d| describe(d)).collect()
}

/// Like `ios_devices`, but while the list is empty keeps asking for up to
/// `deadline`. For the command line; never call it on the UI thread.
pub fn ios_devices_waiting(deadline: Duration) -> Vec<Device> {
    let started = Instant::now();
    let (list, tries, _) = poll_until(deadline, IOS_POLL, || Some(ios_devices()).filter(|l| !l.is_empty()), std::thread::sleep);
    let list = list.unwrap_or_default();
    crate::diag::note(&format!("glide: ios discovery: {} listed after {tries} tries ({} ms): [{}]", list.len(), started.elapsed().as_millis(), names(&list)));
    list
}

static IOS_FRAME_DIMS: Mutex<Option<(String, (u32, u32))>> = Mutex::new(None);

/// Record dimensions from an observed iOS screen preview frame.
pub fn record_ios_frame_dimensions(id: &str, w: u32, h: u32) {
    if w >= 2 && h >= 2 {
        let mut g = IOS_FRAME_DIMS.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some((id.to_string(), (w, h)));
    }
}

/// The latest dimensions seen from an iOS screen preview frame for `id`, if any.
pub fn last_ios_frame_dimensions(id: &str) -> Option<(u32, u32)> {
    let g = IOS_FRAME_DIMS.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_ref() {
        Some((dev_id, dims)) if dev_id == id => Some(*dims),
        _ => None,
    }
}

/// Clear the recorded iOS screen frame dimensions (for tests).
pub fn clear_ios_frame_dimensions() {
    let mut g = IOS_FRAME_DIMS.lock().unwrap_or_else(|e| e.into_inner());
    *g = None;
}

/// Screen dimensions in pixels for a device, checking its active format,
/// then its supported formats, then its input ports.
pub fn device_dimensions(dev: &AVCaptureDevice) -> Option<(u32, u32)> {
    unsafe {
        let desc = dev.activeFormat().formatDescription();
        let dims = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&desc);
        if dims.width > 0 && dims.height > 0 {
            return Some((dims.width as u32, dims.height as u32));
        }

        let mut best: Option<(Retained<AVCaptureDeviceFormat>, (u32, u32))> = None;
        for f in dev.formats().iter() {
            let desc = f.formatDescription();
            let dims = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&desc);
            if dims.width > 0 && dims.height > 0 {
                let (w, h) = (dims.width as u32, dims.height as u32);
                match &best {
                    Some((_, (bw, bh))) if (w as u64 * h as u64) <= (*bw as u64 * *bh as u64) => {}
                    _ => best = Some((f.clone(), (w, h))),
                }
            }
        }
        if let Some((f, (w, h))) = best {
            if dev.lockForConfiguration().is_ok() {
                dev.setActiveFormat(&f);
                dev.unlockForConfiguration();
            }
            return Some((w, h));
        }

        if let Ok(input) = AVCaptureDeviceInput::deviceInputWithDevice_error(dev) {
            for port in input.ports().iter() {
                if let Some(desc) = port.formatDescription() {
                    let dims = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&desc);
                    if dims.width > 0 && dims.height > 0 {
                        return Some((dims.width as u32, dims.height as u32));
                    }
                }
            }
        }

        None
    }
}

/// An iOS device's current screen size in pixels.
pub fn ios_dimensions(id: &str) -> Option<(u32, u32)> {
    if let Some((w, h)) = last_ios_frame_dimensions(id) {
        crate::diag::note(&format!("glide: ios dimensions for {id}: {w}x{h} (from live preview frame)"));
        return Some((w, h));
    }

    let dev = wait_ios(id)?;

    if let Some((w, h)) = device_dimensions(&dev) {
        crate::diag::note(&format!("glide: ios dimensions for {id}: {w}x{h} (from device formats/ports)"));
        return Some((w, h));
    }

    let fallback = (1170, 2532);
    crate::diag::note(&format!("glide: ios dimensions for {id}: fallback {fallback:?}"));
    Some(fallback)
}

pub fn cameras() -> Vec<Device> {
    cameras_raw().iter().map(|d| describe(d)).collect()
}

pub fn microphones() -> Vec<Device> {
    unsafe {
        discover(&[AVCaptureDeviceTypeMicrophone], AVMediaTypeAudio.unwrap())
            .iter()
            .map(|d| describe(d))
            .collect()
    }
}

/// Unique ID of the system's default microphone.
pub fn default_microphone() -> Option<String> {
    unsafe { AVCaptureDevice::defaultDeviceWithMediaType(AVMediaTypeAudio.unwrap()).map(|d| d.uniqueID().to_string()) }
}

/// A MacBook's built-in microphone is disconnected in hardware while the lid
/// is closed (e.g. on an external display), so it records only silence.
/// True when `mic_id` (None = default) is that microphone and the lid is shut.
pub fn builtin_mic_off(mic_id: Option<&str>) -> bool {
    let id = mic_id.map(str::to_owned).or_else(default_microphone);
    id.as_deref() == Some("BuiltInMicrophoneDevice") && lid_closed()
}

/// Is the laptop lid closed (IOPMrootDomain's AppleClamshellState)?
/// Cached for two seconds so the UI can ask every frame.
pub fn lid_closed() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, v)) = *c
        && at.elapsed() < Duration::from_secs(2)
    {
        return v;
    }
    let v = read_clamshell_state();
    *c = Some((Instant::now(), v));
    v
}

fn read_clamshell_state() -> bool {
    use std::ffi::{c_char, c_void};
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
        fn IORegistryEntryCreateCFProperty(entry: u32, key: *const c_void, allocator: *const c_void, options: u32) -> *const c_void;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFBooleanGetValue(b: *const c_void) -> bool;
        fn CFGetTypeID(cf: *const c_void) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: standard IOKit registry lookup; the matching dictionary is
    // consumed by IOServiceGetMatchingService and every object is released.
    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(c"IOPMrootDomain".as_ptr()));
        if service == 0 {
            return false;
        }
        let key = objc2_core_foundation::CFString::from_str("AppleClamshellState");
        let v = IORegistryEntryCreateCFProperty(service, objc2_core_foundation::CFRetained::as_ptr(&key).as_ptr() as *const c_void, std::ptr::null(), 0);
        IOObjectRelease(service);
        if v.is_null() {
            return false;
        }
        let closed = CFGetTypeID(v) == CFBooleanGetTypeID() && CFBooleanGetValue(v);
        CFRelease(v);
        closed
    }
}

/// Resolve a user-supplied mic selector (unique ID, or case-insensitive name
/// substring) to a unique ID.
pub fn resolve_mic(sel: &str) -> Result<String> {
    let mics = microphones();
    if let Some(m) = mics.iter().find(|m| m.id == sel) {
        return Ok(m.id.clone());
    }
    let lower = sel.to_lowercase();
    mics.iter()
        .find(|m| m.name.to_lowercase().contains(&lower))
        .map(|m| m.id.clone())
        .ok_or_else(|| anyhow!("no microphone matches {sel:?} (see `glide devices`)"))
}

/// The camera `sel` names: None = the system-preferred camera (what FaceTime
/// would use), else the built-in camera, else anything; Some = unique ID or
/// name substring.
pub fn find_camera(sel: Option<&str>) -> Result<Retained<AVCaptureDevice>> {
    let cams = cameras_raw();
    match sel {
        None => unsafe { AVCaptureDevice::systemPreferredCamera() }
            .or_else(|| {
                cams.iter()
                    .find(|d| unsafe { d.deviceType().to_string() == AVCaptureDeviceTypeBuiltInWideAngleCamera.to_string() })
                    .cloned()
            })
            .or_else(|| cams.into_iter().next()),
        Some(s) => {
            let lower = s.to_lowercase();
            cams.into_iter().find(|d| unsafe {
                d.uniqueID().to_string() == s || d.localizedName().to_string().to_lowercase().contains(&lower)
            })
        }
    }
    .ok_or_else(|| anyhow!("no camera found (see `glide devices`)"))
}

/// A connected iPhone/iPad by unique id.
pub fn find_ios(id: &str) -> Result<Retained<AVCaptureDevice>> {
    wait_ios(id).ok_or_else(|| anyhow!("the iPhone/iPad is no longer connected"))
}

/// Consecutive frames of the wrong size tolerated before a device recording
/// is stopped with an error (half a second at 60 fps).
const SIZE_MISMATCH_LIMIT: u32 = 30;

/// Does a delivered `frame` (width, height) match the `writer` size? The
/// writer's sides are rounded to even, so an odd frame side is one pixel off
/// (up or down) and still fits.
pub fn frame_fits_writer(frame: (usize, usize), writer: (usize, usize)) -> bool {
    frame.0 >= 2 && frame.1 >= 2 && frame.0.abs_diff(writer.0) <= 1 && frame.1.abs_diff(writer.1) <= 1
}

/// The length of the current run of wrong-sized frames after one more frame.
fn mismatch_run(run: u32, fits: bool) -> u32 {
    if fits { 0 } else { run.saturating_add(1) }
}

/// Use the device's own format instead of a preset that could change it. A
/// preset on an iPhone's screen can switch the active format to another size.
pub fn use_native_format(session: &AVCaptureSession) {
    // SAFETY: AVFoundation calls on a session the caller owns.
    unsafe {
        if session.canSetSessionPreset(AVCaptureSessionPresetInputPriority) {
            session.setSessionPreset(AVCaptureSessionPresetInputPriority);
        }
    }
}

/// What to tell the user when a capture session reports a runtime error.
pub fn runtime_error_text(description: Option<&str>) -> String {
    match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("The capture session stopped: {d}"),
        None => "The capture session stopped unexpectedly.".into(),
    }
}

/// What to tell the user when the device itself goes away.
pub const IOS_DISCONNECTED: &str = "The iPhone/iPad was disconnected.";

/// Reports a session's runtime errors and its device's disconnection through
/// `report`, until dropped (which removes the observers).
pub struct SessionWatch {
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

// SAFETY: the tokens are only kept to be removed from the notification center,
// which is thread-safe.
unsafe impl Send for SessionWatch {}

impl SessionWatch {
    pub fn new(session: &AVCaptureSession, device: &AVCaptureDevice, disconnected: &'static str, report: Arc<dyn Fn(String) + Send + Sync>) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let on_error = {
            let report = report.clone();
            RcBlock::new(move |n: std::ptr::NonNull<NSNotification>| {
                // SAFETY: a valid notification for the duration of the call.
                let description = unsafe {
                    let n = n.as_ref();
                    n.userInfo()
                        .and_then(|u| u.objectForKey(AVCaptureSessionErrorKey))
                        .and_then(|e| e.downcast::<NSError>().ok())
                        .map(|e| e.localizedDescription().to_string())
                };
                report(runtime_error_text(description.as_deref()));
            })
        };
        let on_gone = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| report(disconnected.to_string()));
        // SAFETY: observers on notifications of objects we hold; the blocks
        // are Send + Sync closures, and `Drop` removes the observers.
        let tokens = unsafe {
            vec![
                center.addObserverForName_object_queue_usingBlock(Some(AVCaptureSessionRuntimeErrorNotification), Some(session), None, &on_error),
                center.addObserverForName_object_queue_usingBlock(Some(AVCaptureDeviceWasDisconnectedNotification), Some(device), None, &on_gone),
            ]
        };
        SessionWatch { tokens }
    }
}

impl Drop for SessionWatch {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for t in self.tokens.drain(..) {
            // SAFETY: the token came from addObserverForName in `new`.
            unsafe { center.removeObserver(AsRef::<AnyObject>::as_ref(&*t)) };
        }
    }
}

pub struct CamIvars {
    shared: Arc<Shared>,
    /// Deliver frames as the sink's primary ("screen") video, for a
    /// camera-only recording.
    as_screen: bool,
    /// The writer's size, for an iPhone/iPad screen. Frames of another size
    /// are dropped, and the recording fails if that goes on.
    writer_size: Option<(usize, usize)>,
    /// Consecutive frames that did not fit `writer_size`.
    mismatched: AtomicU32,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "GlideCameraOutput"]
    #[ivars = CamIvars]
    pub struct CameraOutput;

    unsafe impl NSObjectProtocol for CameraOutput {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn did_output(&self, _o: &AVCaptureOutput, sb: &CMSampleBuffer, _c: &AVCaptureConnection) {
            unsafe {
                if let Some(pb) = sb.image_buffer() {
                    let pts = sb.presentation_time_stamp();
                    let iv = self.ivars();
                    if let Some(writer) = iv.writer_size {
                        let frame = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
                        let run = mismatch_run(iv.mismatched.load(Ordering::Relaxed), frame_fits_writer(frame, writer));
                        iv.mismatched.store(run, Ordering::Relaxed);
                        if run > 0 {
                            if run == SIZE_MISMATCH_LIMIT {
                                let msg = format!(
                                    "The iPhone/iPad screen changed size (rotation?). It sends {}x{}, the recording is {}x{}. Stop and record again.",
                                    frame.0, frame.1, writer.0, writer.1
                                );
                                crate::diag::note(&format!("glide: ios recording: {msg}"));
                                iv.shared.fail(msg);
                            }
                            return;
                        }
                    }
                    if iv.as_screen {
                        iv.shared.with(|s| s.push_screen(pts, pb));
                    } else {
                        iv.shared.with(|s| s.push_cam(pts, pb));
                    }
                }
            }
        }
    }
);

pub struct Camera {
    /// First, so the observers go before the session does.
    _watch: Option<SessionWatch>,
    session: Retained<AVCaptureSession>,
    _out: Retained<CameraOutput>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
    pub name: String,
}

unsafe impl Send for Camera {}

impl Camera {
    /// `sel`: None = first camera, else unique ID or name substring.
    pub fn start(sel: Option<&str>, shared: Arc<Shared>) -> Result<Self> {
        Self::start_with(sel, shared, false)
    }

    /// Record a connected iOS device's screen as the sink's main video. The
    /// writer is `size` pixels; the session keeps the device's own format.
    pub fn start_ios(id: &str, shared: Arc<Shared>, size: (usize, usize)) -> Result<Self> {
        Self::run(find_ios(id)?, shared, true, Some(size))
    }

    /// Like `start`; `as_screen` routes frames to the sink's main video.
    pub fn start_with(sel: Option<&str>, shared: Arc<Shared>, as_screen: bool) -> Result<Self> {
        Self::run(find_camera(sel)?, shared, as_screen, None)
    }

    /// `ios`: the writer size when `dev` is an iPhone/iPad screen. Webcams
    /// get a 720p preset.
    fn run(dev: Retained<AVCaptureDevice>, shared: Arc<Shared>, as_screen: bool, ios: Option<(usize, usize)>) -> Result<Self> {
        unsafe {
            let name = dev.localizedName().to_string();
            let session = AVCaptureSession::new();
            session.beginConfiguration();
            if let Some(target_size) = ios {
                use_native_format(&session);
                for f in dev.formats().iter() {
                    let desc = f.formatDescription();
                    let dims = objc2_core_media::CMVideoFormatDescriptionGetDimensions(&desc);
                    if dims.width == target_size.0 as i32 && dims.height == target_size.1 as i32 {
                        if dev.lockForConfiguration().is_ok() {
                            dev.setActiveFormat(&f);
                            dev.unlockForConfiguration();
                        }
                        break;
                    }
                }
            } else if session.canSetSessionPreset(AVCaptureSessionPreset1280x720) {
                session.setSessionPreset(AVCaptureSessionPreset1280x720);
            } else if session.canSetSessionPreset(AVCaptureSessionPresetHigh) {
                session.setSessionPreset(AVCaptureSessionPresetHigh);
            }
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&dev)
                .map_err(|e| anyhow!("camera {name}: {}", e.localizedDescription()))?;
            if !session.canAddInput(&input) {
                bail!("cannot use camera {name}");
            }
            session.addInput(&input);
            let output = AVCaptureVideoDataOutput::new();
            output.setVideoSettings(Some(&dict(vec![(
                cf_ns(kCVPixelBufferPixelFormatTypeKey),
                num_i(PIX_BGRA as i64),
            )])));
            output.setAlwaysDiscardsLateVideoFrames(true);
            // Only an iPhone/iPad ends the take: it is the recording's one video.
            let watch = ios.is_some().then(|| {
                let shared = shared.clone();
                SessionWatch::new(&session, &dev, IOS_DISCONNECTED, Arc::new(move |m| {
                    crate::diag::note(&format!("glide: ios recording: {m}"));
                    shared.fail(m);
                }))
            });
            let delegate = Self::make_delegate(shared, as_screen, ios);
            let queue = DispatchQueue::new("rec.camera", None);
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(&queue));
            if !session.canAddOutput(&output) {
                bail!("cannot add camera output");
            }
            session.addOutput(&output);
            session.commitConfiguration();
            session.startRunning();
            if !session.isRunning() {
                bail!("camera {name} failed to start");
            }
            Ok(Camera { _watch: watch, session, _out: delegate, _queue: queue, name })
        }
    }

    fn make_delegate(shared: Arc<Shared>, as_screen: bool, writer_size: Option<(usize, usize)>) -> Retained<CameraOutput> {
        let this = CameraOutput::alloc().set_ivars(CamIvars { shared, as_screen, writer_size, mismatched: AtomicU32::new(0) });
        unsafe { msg_send![super(this), init] }
    }

    pub fn stop(&self) {
        unsafe { self.session.stopRunning() };
    }
}

pub struct MicIvars {
    shared: Arc<Shared>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "GlideMicOutput"]
    #[ivars = MicIvars]
    pub struct MicOutput;

    unsafe impl NSObjectProtocol for MicOutput {}

    unsafe impl AVCaptureAudioDataOutputSampleBufferDelegate for MicOutput {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn did_output(&self, _o: &AVCaptureOutput, sb: &CMSampleBuffer, _c: &AVCaptureConnection) {
            self.ivars().shared.with(|s| s.push_audio(crate::capture::writer::AudioKind::Mic, sb));
        }
    }
);

/// Microphone via AVCaptureSession (fallback / test path; the default mic path
/// is ScreenCaptureKit's built-in microphone capture).
pub struct Mic {
    session: Retained<AVCaptureSession>,
    _out: Retained<MicOutput>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
}

unsafe impl Send for Mic {}

impl Mic {
    pub fn start(id: Option<&str>, shared: Arc<Shared>) -> Result<Self> {
        unsafe {
            let media = AVMediaTypeAudio.unwrap();
            let dev = match id {
                Some(id) => AVCaptureDevice::deviceWithUniqueID(&objc2_foundation::NSString::from_str(id)),
                None => AVCaptureDevice::defaultDeviceWithMediaType(media),
            }
            .ok_or_else(|| anyhow!("microphone not found"))?;
            let session = AVCaptureSession::new();
            let input = AVCaptureDeviceInput::deviceInputWithDevice_error(&dev)
                .map_err(|e| anyhow!("microphone: {}", e.localizedDescription()))?;
            if !session.canAddInput(&input) {
                bail!("cannot use microphone");
            }
            session.addInput(&input);
            let output = AVCaptureAudioDataOutput::new();
            // Mono 48 kHz float, the mic track's own layout. A mic's native
            // format (an iMac's: stereo 44.1 kHz 24-bit) made the writer
            // fail on the first buffer (kCMSampleBufferError_ArrayTooSmall).
            // Key constants live in AVFAudio; their values equal their names.
            output.setAudioSettings(Some(&dict(vec![
                (&*ns("AVFormatIDKey"), num_i(fourcc(b"lpcm") as i64)),
                (&*ns("AVSampleRateKey"), num_f(48000.0)),
                (&*ns("AVNumberOfChannelsKey"), num_i(1)),
                (&*ns("AVLinearPCMBitDepthKey"), num_i(32)),
                (&*ns("AVLinearPCMIsFloatKey"), num_b(true)),
                (&*ns("AVLinearPCMIsNonInterleaved"), num_b(false)),
                (&*ns("AVLinearPCMIsBigEndianKey"), num_b(false)),
            ])));
            let this = MicOutput::alloc().set_ivars(MicIvars { shared });
            let delegate: Retained<MicOutput> = msg_send![super(this), init];
            let queue = DispatchQueue::new("rec.mic", None);
            output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(&queue));
            if !session.canAddOutput(&output) {
                bail!("cannot add microphone output");
            }
            session.addOutput(&output);
            session.startRunning();
            Ok(Mic { session, _out: delegate, _queue: queue })
        }
    }

    pub fn stop(&self) {
        unsafe { self.session.stopRunning() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that succeeds on its `n`th call, recording the pauses asked for.
    fn run(n: u32, deadline: Duration) -> (Option<u32>, u32, Duration, Vec<Duration>) {
        let (mut calls, mut slept) = (0, vec![]);
        let (v, tries, waited) = poll_until(
            deadline,
            Duration::from_millis(150),
            || {
                calls += 1;
                (calls == n).then_some(calls)
            },
            |d| slept.push(d),
        );
        (v, tries, waited, slept)
    }

    #[test]
    fn poll_until_finds_the_device_on_the_fourth_try() {
        let (v, tries, waited, slept) = run(4, IOS_WAIT);
        assert_eq!(v, Some(4));
        assert_eq!(tries, 4);
        assert_eq!(waited, Duration::from_millis(450));
        assert_eq!(slept, vec![Duration::from_millis(150); 3]);
    }

    #[test]
    fn poll_until_gives_up_after_the_deadline() {
        let (v, tries, waited, slept) = run(u32::MAX, IOS_WAIT);
        assert_eq!(v, None);
        assert_eq!(tries, 21);
        assert_eq!(waited, IOS_WAIT);
        assert_eq!(slept.len(), 20);
    }

    #[test]
    fn poll_until_does_not_sleep_when_the_first_try_succeeds() {
        let (v, tries, waited, slept) = run(1, IOS_WAIT);
        assert_eq!((v, tries, waited), (Some(1), 1, Duration::ZERO));
        assert!(slept.is_empty());
    }

    #[test]
    fn poll_until_tries_once_when_the_deadline_is_shorter_than_a_step() {
        let (v, tries, _, slept) = run(u32::MAX, Duration::from_millis(100));
        assert_eq!((v, tries), (None, 1));
        assert!(slept.is_empty());
    }

    #[test]
    fn frame_fits_writer_accepts_the_same_size_and_one_extra_odd_pixel() {
        assert!(frame_fits_writer((1170, 2532), (1170, 2532)));
        assert!(frame_fits_writer((1171, 2532), (1170, 2532)));
        assert!(frame_fits_writer((1170, 2533), (1170, 2532)));
        // The writer rounds an odd side up.
        assert!(frame_fits_writer((1171, 2533), (1172, 2534)));
    }

    #[test]
    fn frame_fits_writer_rejects_a_rotated_or_other_size() {
        assert!(!frame_fits_writer((2532, 1170), (1170, 2532)));
        assert!(!frame_fits_writer((1280, 720), (1170, 2532)));
        assert!(!frame_fits_writer((1172, 2532), (1170, 2532)));
    }

    #[test]
    fn frame_fits_writer_rejects_empty_frames() {
        assert!(!frame_fits_writer((0, 0), (0, 0)));
        assert!(!frame_fits_writer((1, 1), (0, 0)));
        assert!(!frame_fits_writer((0, 2532), (1170, 2532)));
    }

    #[test]
    fn mismatch_run_counts_consecutive_bad_frames_and_resets_on_a_good_one() {
        let run = (0..5).fold(0, |r, _| mismatch_run(r, false));
        assert_eq!(run, 5);
        assert_eq!(mismatch_run(run, true), 0);
        assert_eq!(mismatch_run(u32::MAX, false), u32::MAX);
    }

    #[test]
    fn runtime_error_text_uses_the_description_or_a_fallback() {
        assert_eq!(runtime_error_text(Some("Cannot Record")), "The capture session stopped: Cannot Record");
        assert_eq!(runtime_error_text(Some("  ")), "The capture session stopped unexpectedly.");
        assert_eq!(runtime_error_text(None), "The capture session stopped unexpectedly.");
    }

    #[test]
    fn poll_until_with_a_zero_step_does_not_loop() {
        let (v, tries, _) = poll_until(IOS_WAIT, Duration::ZERO, || None::<u8>, |_| {});
        assert_eq!((v, tries), (None, 1));
    }

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn record_and_retrieve_ios_frame_dimensions() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        clear_ios_frame_dimensions();
        assert_eq!(last_ios_frame_dimensions("mock-id"), None);
        record_ios_frame_dimensions("mock-id", 1170, 2532);
        assert_eq!(last_ios_frame_dimensions("mock-id"), Some((1170, 2532)));
        // Degenerate dimensions are ignored
        record_ios_frame_dimensions("mock-id", 0, 0);
        record_ios_frame_dimensions("mock-id", 1, 100);
        assert_eq!(last_ios_frame_dimensions("mock-id"), Some((1170, 2532)));
        // Dimensions for another device ID are not returned
        assert_eq!(last_ios_frame_dimensions("other-id"), None);
        clear_ios_frame_dimensions();
        assert_eq!(last_ios_frame_dimensions("mock-id"), None);
    }

    #[test]
    fn ios_dimensions_uses_recorded_frame_dimensions() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        clear_ios_frame_dimensions();
        record_ios_frame_dimensions("mock-id", 1179, 2556);
        assert_eq!(ios_dimensions("mock-id"), Some((1179, 2556)));
        clear_ios_frame_dimensions();
    }

    #[test]
    fn ios_dimensions_ignores_dimensions_from_another_device() {
        let _guard = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        clear_ios_frame_dimensions();
        record_ios_frame_dimensions("phone-1", 1170, 2532);
        assert_eq!(last_ios_frame_dimensions("phone-2"), None);
        clear_ios_frame_dimensions();
    }
}
