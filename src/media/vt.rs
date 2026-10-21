//! Exact frames by sample through one persistent VideoToolbox session.
//!
//! `FrameSource` builds a new AVAssetReader, and with it a new decoder, on
//! every backward move or long jump. `VtDecoder` keeps one
//! `VTDecompressionSession`, feeds it compressed samples straight from the
//! file and decodes from the target's full-sync sample, so a seek costs only
//! the frames in between and a forward move in the same GOP continues from
//! where the decoder stopped. With frame reordering, pictures decoded before
//! the target but shown after it are kept, so playback decodes each sample
//! once.

use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result, anyhow, bail, ensure};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_av_foundation::{AVAssetTrack, AVMediaTypeVideo, AVSampleBufferGenerator, AVSampleBufferRequest, AVSampleCursor, AVURLAsset};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMFormatDescription, CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVBuffer, CVPixelBuffer, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey,
};

use super::{Frame, ms, open_asset, tracks};
use crate::util::*;

/// Most frames decoded without output between two checks of the caller's
/// `keep_going`: bounds the work a newer request waits for.
const BATCH: usize = 8;

/// Most pictures kept for later targets (see `VtDecoder::ahead`). Common
/// encoders reorder by 2 to 4 frames; a picture past this is decoded without
/// output, and showing it later costs a walk from its full-sync sample.
const AHEAD: usize = 4;

#[repr(C)]
struct OutputCallback {
    callback: extern "C" fn(*mut c_void, *mut c_void, i32, u32, *mut CVBuffer, CMTime, CMTime),
    refcon: *mut c_void,
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTDecompressionSessionCreate(
        allocator: *const c_void,
        format: *const CMFormatDescription,
        decoder_specification: *const c_void,
        destination_attributes: *const c_void,
        callback: *const OutputCallback,
        session: *mut *mut c_void,
    ) -> i32;
    fn VTDecompressionSessionDecodeFrame(
        session: *mut c_void,
        sample: *const CMSampleBuffer,
        flags: u32,
        frame_refcon: *mut c_void,
        info: *mut u32,
    ) -> i32;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> i32;
    fn VTDecompressionSessionCanAcceptFormatDescription(session: *mut c_void, format: *const CMFormatDescription) -> u8;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// kVTDecodeFrame_EnableAsynchronousDecompression
const ASYNC: u32 = 1 << 0;
/// kVTDecodeFrame_DoNotOutputFrame
const NO_OUTPUT: u32 = 1 << 1;

/// What the output callback reports, read after each wait. Boxed by the
/// decoder so the address handed to VideoToolbox never moves.
#[derive(Default)]
struct Outcome {
    /// First nonzero frame status since the last check (0: none).
    status: AtomicI32,
    /// The picture of the frame decoded with output.
    image: Mutex<Option<SendBox<CFRetained<CVPixelBuffer>>>>,
    /// Report the next successful frame as failed.
    #[cfg(test)]
    fail_next: std::sync::atomic::AtomicBool,
}

/// kVTVideoDecoderBadDataErr, for injected failures.
const BAD_DATA: i32 = -12909;

// Runs on VideoToolbox's threads for asynchronous frames: must not panic.
extern "C" fn on_output(refcon: *mut c_void, shown: *mut c_void, status: i32, _: u32, image: *mut CVBuffer, _: CMTime, _: CMTime) {
    // SAFETY: `refcon` is the decoder's boxed `Outcome`, which outlives the
    // session (sessions are drained and invalidated before it is freed).
    let out = unsafe { &*(refcon as *const Outcome) };
    #[cfg(test)]
    let status = if status == 0 && out.fail_next.swap(false, Ordering::SeqCst) { BAD_DATA } else { status };
    if status != 0 {
        let _ = out.status.compare_exchange(0, status, Ordering::SeqCst, Ordering::SeqCst);
        return;
    }
    // Only the target frame carries a non-null `shown` tag.
    if !shown.is_null()
        && let Some(img) = NonNull::new(image)
        && let Ok(mut slot) = out.image.lock()
    {
        *slot = Some(SendBox(unsafe { CFRetained::retain(img) }));
    }
}

/// A decompression session. Dropping it waits for queued frames, then
/// invalidates it.
struct Session(NonNull<c_void>);

impl Session {
    fn create(format: &CMFormatDescription, outcome: &Outcome) -> Result<Session> {
        // BGRA in IOSurfaces Metal can wrap, like `VideoReader`'s output.
        let attrs = dict(vec![
            (cf_ns(unsafe { kCVPixelBufferPixelFormatTypeKey }), num_i(PIX_BGRA as i64)),
            (cf_ns(unsafe { kCVPixelBufferIOSurfacePropertiesKey }), obj(&*dict(vec![]))),
            (cf_ns(unsafe { kCVPixelBufferMetalCompatibilityKey }), num_b(true)),
        ]);
        let callback = OutputCallback { callback: on_output, refcon: outcome as *const Outcome as *mut c_void };
        let mut raw = std::ptr::null_mut();
        // No decoder specification: VideoToolbox picks the hardware decoder
        // when there is one and falls back to software otherwise.
        let status = unsafe {
            VTDecompressionSessionCreate(
                std::ptr::null(),
                format,
                std::ptr::null(),
                Retained::as_ptr(&attrs) as *const c_void,
                &callback,
                &mut raw,
            )
        };
        match NonNull::new(raw) {
            Some(raw) if status == 0 => Ok(Session(raw)),
            _ => bail!("VTDecompressionSessionCreate failed: {status}"),
        }
    }

    fn decode(&self, sample: &CMSampleBuffer, flags: u32, shown: bool) -> i32 {
        let tag = if shown { NonNull::<c_void>::dangling().as_ptr() } else { std::ptr::null_mut() };
        let mut info = 0;
        unsafe { VTDecompressionSessionDecodeFrame(self.0.as_ptr(), sample, flags, tag, &mut info) }
    }

    fn wait(&self) -> i32 {
        unsafe { VTDecompressionSessionWaitForAsynchronousFrames(self.0.as_ptr()) }
    }

    fn accepts(&self, format: &CMFormatDescription) -> bool {
        unsafe { VTDecompressionSessionCanAcceptFormatDescription(self.0.as_ptr(), format) != 0 }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.wait();
        unsafe {
            VTDecompressionSessionInvalidate(self.0.as_ptr());
            CFRelease(self.0.as_ptr());
        }
    }
}

/// The track's sample table, read from the movie header.
struct Index {
    /// Presentation time in ms from the track start, by decode position.
    pts: Vec<f64>,
    /// Decode position of the latest full-sync sample at or before each
    /// sample (its GOP).
    key: Vec<usize>,
    /// Decode positions in presentation order.
    by_pts: Vec<usize>,
    /// Half a frame in ms (the target rule's tolerance).
    half: f64,
}

impl Index {
    fn build(track: &AVAssetTrack) -> Result<Index> {
        if !unsafe { track.canProvideSampleCursors() } {
            bail!("the video track can't provide sample cursors");
        }
        let origin = ms(unsafe { track.timeRange() }.start);
        let fps = unsafe { track.nominalFrameRate() } as f64;
        let fps = if fps > 0.0 { fps } else { 60.0 };
        let cursor = unsafe { track.makeSampleCursorAtFirstSampleInDecodeOrder() }.context("empty video track")?;
        let (mut pts, mut key) = (vec![], vec![]);
        loop {
            let full = unsafe { cursor.currentSampleSyncInfo() }.sampleIsFullSync.as_bool();
            // Decoding starts at the first sample, whatever its flags say.
            key.push(if full || key.is_empty() { key.len() } else { key[key.len() - 1] });
            pts.push(ms(unsafe { cursor.presentationTimeStamp() }) - origin);
            if unsafe { cursor.stepInDecodeOrderByCount(1) } != 1 {
                break;
            }
        }
        let mut by_pts: Vec<usize> = (0..pts.len()).collect();
        by_pts.sort_by(|&a, &b| pts[a].total_cmp(&pts[b]));
        Ok(Index { pts, key, by_pts, half: 0.5 * 1000.0 / fps })
    }

    fn target(&self, t_ms: f64) -> usize {
        let n = self.by_pts.partition_point(|&i| self.pts[i] <= t_ms + self.half);
        self.by_pts[n.saturating_sub(1)]
    }

    /// Whether sample `j` is a leading picture: after its full-sync sample
    /// in decode order but before it on screen. With open GOPs (HEVC with
    /// frame reordering) these may reference the GOP before, so they can't
    /// be decoded from their own full-sync sample. No picture after the
    /// full-sync sample on screen references them.
    fn leading(&self, j: usize) -> bool {
        let k = self.key[j];
        k > 0 && self.pts[j] < self.pts[k]
    }

    /// The full-sync sample decoding sample `i` starts at: its own GOP's,
    /// or the GOP before's for a leading picture.
    fn start(&self, i: usize) -> usize {
        if self.leading(i) { self.key[self.key[i] - 1] } else { self.key[i] }
    }
}

/// The session's reference state: a walk from full-sync sample `start` has
/// decoded every sample it needs up to `last`.
#[derive(Clone, Copy, Debug)]
struct Decoded {
    start: usize,
    last: usize,
}

/// Test-only failures, each injected once at the next opportunity.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fault {
    /// `VTDecompressionSessionDecodeFrame` fails for a hidden frame.
    Decode,
    /// The callback reports a hidden frame as failed.
    Callback,
    /// `VTDecompressionSessionWaitForAsynchronousFrames` fails.
    Wait,
    /// The session rejects the next sample's format.
    Reject,
}

/// Decodes a video file's frames by sample with one long-lived
/// VideoToolbox session. Use it from one thread at a time.
pub struct VtDecoder {
    session: Option<Session>,
    /// Callback state; outlives every session (see `Drop`).
    outcome: Box<Outcome>,
    index: Index,
    generator: Retained<AVSampleBufferGenerator>,
    cursor: Retained<AVSampleCursor>,
    /// Decode position of `cursor`.
    cursor_at: usize,
    /// Format of the last sample the session took.
    format: Option<CFRetained<CMFormatDescription>>,
    /// What the session holds, while its reference state is intact: a later
    /// sample in the same GOP continues from here.
    decoded: Option<Decoded>,
    /// Pictures of samples this walk decoded before its target that are
    /// shown after it (frame reordering), by decode position: the next
    /// targets in playback. At most `AHEAD`.
    ahead: Vec<(usize, Frame)>,
    _asset: Retained<AVURLAsset>,
    #[cfg(test)]
    fault: Option<Fault>,
    /// Samples handed to the session.
    #[cfg(test)]
    submitted: usize,
    /// Sessions created.
    #[cfg(test)]
    sessions: usize,
}

// SAFETY: the decoder is used by one thread at a time; VideoToolbox only
// touches `outcome`, which is thread-safe.
unsafe impl Send for VtDecoder {}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        // Drain and invalidate before `outcome` is freed.
        self.session = None;
    }
}

impl VtDecoder {
    /// Read the first video track's sample table. Fails if the track can't
    /// provide sample cursors.
    pub fn open(path: &Path) -> Result<Self> {
        let asset = open_asset(path)?;
        let track = tracks(&asset, unsafe { AVMediaTypeVideo.unwrap() })
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no video track in {}", path.display()))?;
        let index = Index::build(&track)?;
        // No timebase: sample data loads synchronously.
        let generator = unsafe { AVSampleBufferGenerator::initWithAsset_timebase(AVSampleBufferGenerator::alloc(), &asset, None) };
        let cursor = unsafe { track.makeSampleCursorAtFirstSampleInDecodeOrder() }.context("empty video track")?;
        Ok(VtDecoder {
            session: None,
            outcome: Box::default(),
            index,
            generator,
            cursor,
            cursor_at: 0,
            format: None,
            decoded: None,
            ahead: vec![],
            _asset: asset,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            submitted: 0,
            #[cfg(test)]
            sessions: 0,
        })
    }

    /// The sample on screen at `source_ms`: the latest with pts <= t + half
    /// a frame, as `FrameSource::frame_at` picks it.
    pub fn target(&self, source_ms: f64) -> usize {
        self.index.target(source_ms)
    }

    /// Decode sample `i` (an id from `target`). Frames before it are decoded
    /// without output in batches, and `keep_going` is asked after each one:
    /// false stops with `Ok(None)`, keeping the work done for a later sample
    /// in the same GOP. Frames before it that are shown after it keep their
    /// pictures, and a later call for one of them returns it without decoding.
    /// Any failure forgets the decoder's reference state, so the next call
    /// starts again at the full-sync sample.
    pub fn decode(&mut self, i: usize, keep_going: impl Fn() -> bool) -> Result<Option<Frame>> {
        ensure!(i < self.index.pts.len(), "sample {i} out of range");
        let r = objc2::rc::autoreleasepool(|_| self.walk(i, &keep_going));
        if r.is_err() {
            self.fail();
        }
        r
    }

    /// Drop the session and the reference state; the next decode creates a
    /// new session and starts at the full-sync sample.
    pub fn reset(&mut self) {
        self.session = None;
        self.format = None;
        self.decoded = None;
        self.ahead.clear();
    }

    fn walk(&mut self, i: usize, keep_going: &impl Fn() -> bool) -> Result<Option<Frame>> {
        let pts = self.index.pts[i];
        // Kept pictures stay until the targets pass them.
        self.ahead.retain(|(_, f)| f.pts_ms >= pts);
        if let Some((_, f)) = self.ahead.iter().find(|(j, _)| *j == i) {
            return Ok(Some(f.clone()));
        }
        let start = self.index.start(i);
        // Continue when the session already holds everything from `i`'s
        // full-sync sample up to a sample before `i`.
        let (from, walk) = match self.decoded {
            Some(d) if d.start <= start && start <= d.last && d.last < i => (d.last + 1, d.start),
            _ => {
                self.decoded = None;
                self.ahead.clear();
                (start, start)
            }
        };
        // The leading pictures of the walk's own full-sync sample need the
        // GOP before it, which this walk doesn't have: skip them.
        let needed = |j: usize| !(self.index.leading(j) && self.index.key[j] == walk);
        let needed: Vec<usize> = (from..i).filter(|&j| needed(j)).collect();
        for batch in needed.chunks(BATCH) {
            for &j in batch {
                if self.index.pts[j] > pts && self.ahead.len() < AHEAD {
                    let f = self.output(j, j == start)?;
                    self.ahead.push((j, f));
                } else {
                    self.submit(j, false, j == start)?;
                }
            }
            self.settle()?;
            self.decoded = Some(Decoded { start: walk, last: batch[batch.len() - 1] });
            if !keep_going() {
                return Ok(None);
            }
        }
        let f = self.output(i, i == start)?;
        self.decoded = Some(Decoded { start: walk, last: i });
        Ok(Some(f))
    }

    /// Decode sample `j` with output, after the frames queued before it.
    fn output(&mut self, j: usize, first: bool) -> Result<Frame> {
        self.submit(j, true, first)?;
        self.settle()?;
        let image = self.outcome.image.lock().ok().and_then(|mut s| s.take());
        let pb = image.ok_or_else(|| anyhow!("VideoToolbox returned no picture for sample {j}"))?;
        Ok(Frame { pts_ms: self.index.pts[j], pb })
    }

    /// Hand sample `j` to the session: asynchronously without output for a
    /// frame before the target, synchronously with output for the target.
    /// `first` marks the first sample of a walk from its full-sync sample.
    fn submit(&mut self, j: usize, shown: bool, first: bool) -> Result<()> {
        let sample = self.load(j)?;
        let format = unsafe { sample.format_description() }.context("sample without a format description")?;
        self.check_format(j, format, first)?;
        #[cfg(test)]
        let injected = {
            self.submitted += 1;
            if !shown && self.take_fault(Fault::Callback) {
                self.outcome.fail_next.store(true, Ordering::SeqCst);
            }
            !shown && self.take_fault(Fault::Decode)
        };
        #[cfg(not(test))]
        let injected = false;
        let session = self.session.as_ref().context("no decompression session")?;
        let flags = if shown { 0 } else { ASYNC | NO_OUTPUT };
        let status = if injected { BAD_DATA } else { session.decode(&sample, flags, shown) };
        // A failed call queues nothing, so no callback reports it.
        ensure!(status == 0, "VTDecompressionSessionDecodeFrame failed for sample {j}: {status}");
        Ok(())
    }

    /// Make sure the session takes `format`. A session that doesn't is
    /// replaced at the first sample of a walk; later in a walk, with
    /// references queued in the old session, that's a decode failure.
    fn check_format(&mut self, j: usize, format: CFRetained<CMFormatDescription>, first: bool) -> Result<()> {
        #[cfg(test)]
        let reject = self.take_fault(Fault::Reject);
        #[cfg(not(test))]
        let reject = false;
        let same = self.format.as_ref().is_some_and(|f| CFRetained::as_ptr(f) == CFRetained::as_ptr(&format));
        if same && !reject && self.session.is_some() {
            return Ok(());
        }
        if !reject && self.session.as_ref().is_some_and(|s| s.accepts(&format)) {
            self.format = Some(format);
            return Ok(());
        }
        ensure!(first, "the video format changed at sample {j}, inside a walk");
        // Nothing of this walk is queued yet: drop the old session (it drains).
        self.session = None;
        self.session = Some(Session::create(&format, &self.outcome)?);
        self.format = Some(format);
        #[cfg(test)]
        {
            self.sessions += 1;
        }
        Ok(())
    }

    /// Wait for queued frames and fail if any of them, or the wait, did.
    fn settle(&mut self) -> Result<()> {
        #[cfg(test)]
        let injected = self.take_fault(Fault::Wait);
        #[cfg(not(test))]
        let injected = false;
        let session = self.session.as_ref().context("no decompression session")?;
        let waited = session.wait();
        let waited = if injected { BAD_DATA } else { waited };
        let status = self.outcome.status.swap(0, Ordering::SeqCst);
        ensure!(waited == 0, "VTDecompressionSessionWaitForAsynchronousFrames failed: {waited}");
        ensure!(status == 0, "decoding a frame failed: {status}");
        Ok(())
    }

    /// After a failure: let queued frames finish, then forget their results
    /// and the reference state.
    fn fail(&mut self) {
        if let Some(s) = &self.session {
            s.wait();
        }
        self.outcome.status.store(0, Ordering::SeqCst);
        if let Ok(mut slot) = self.outcome.image.lock() {
            *slot = None;
        }
        self.decoded = None;
        self.ahead.clear();
    }

    /// The compressed sample at decode position `j`.
    fn load(&mut self, j: usize) -> Result<Retained<CMSampleBuffer>> {
        let step = j as i64 - self.cursor_at as i64;
        if step != 0 {
            let moved = unsafe { self.cursor.stepInDecodeOrderByCount(step) };
            self.cursor_at = (self.cursor_at as i64 + moved) as usize;
            ensure!(moved == step, "sample cursor stopped at {} on its way to {j}", self.cursor_at);
        }
        unsafe {
            let request = AVSampleBufferRequest::initWithStartCursor(AVSampleBufferRequest::alloc(), &self.cursor);
            request.setMaxSampleCount(1);
            request.setPreferredMinSampleCount(1);
            self.generator
                .createSampleBufferForRequest_error(&request)
                .map_err(|e| anyhow!("reading sample {j}: {}", e.localizedDescription()))
        }
    }

    #[cfg(test)]
    fn take_fault(&mut self, f: Fault) -> bool {
        let hit = self.fault == Some(f);
        if hit {
            self.fault = None;
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use objc2_core_video::CVPixelBufferPool;

    use super::*;
    use crate::capture::compose::Scene;
    use crate::capture::writer::{AudioLayout, Codec, Sink, Stats, TIMESCALE, VideoSpec};
    use crate::media::FrameSource;
    use crate::media::writer::{MovieWriter, WriterSpec, fill_bgra};
    use crate::project::model;

    const W: usize = 256;
    const H: usize = 160;
    /// Bits of its index painted into each frame.
    const BITS: usize = 10;

    /// A movie in its own temporary directory, removed on drop.
    struct Fixture {
        dir: PathBuf,
        path: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str) -> Fixture {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("glide-vt-{}-{n}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Fixture { path: dir.join("screen.mov"), dir }
    }

    /// Frame `n`: its index as `BITS` columns, white for a set bit.
    fn pixels(n: usize) -> Vec<u8> {
        let mut px = vec![0u8; W * H * 4];
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = if n >> ((i % W) * BITS / W) & 1 == 1 { 255 } else { 0 };
            *p = [v, v, v, 255];
        }
        px
    }

    /// The index painted into a decoded frame.
    fn painted(f: &Frame) -> usize {
        f.with_bgra(|v| {
            let y = v.height as usize / 2;
            (0..BITS)
                .filter(|b| v.bgra[y * v.stride + (2 * b + 1) * v.width as usize / (2 * BITS) * 4 + 1] > 128)
                .map(|b| 1 << b)
                .sum()
        })
    }

    /// Mean absolute difference of two frames' pixels (0..255).
    fn diff(a: &Frame, b: &Frame) -> f64 {
        a.with_bgra(|va| {
            b.with_bgra(|vb| {
                assert_eq!((va.width, va.height), (vb.width, vb.height));
                let (mut sum, mut n) = (0.0, 0.0);
                for y in 0..va.height as usize {
                    for x in 0..va.width as usize * 4 {
                        sum += (va.bgra[y * va.stride + x] as f64 - vb.bgra[y * vb.stride + x] as f64).abs();
                        n += 1.0;
                    }
                }
                sum / n
            })
        })
    }

    /// Write `frames` frames through the capture writer, the way a
    /// recording does (no reordering, a keyframe every 2 s). The writer
    /// drops a frame when its encoder falls behind, as on a loaded machine,
    /// so such a capture is written again, its frames further apart.
    fn capture(path: &Path, codec: Codec, fps: u32, frames: usize) {
        const ATTEMPTS: u32 = 3;
        for attempt in 1..=ATTEMPTS {
            let stats = capture_once(path, codec, fps, frames, attempt);
            if stats.video_drops == 0 {
                assert_eq!(stats.video_frames_out, frames as u64);
                return;
            }
            eprintln!("capture {attempt} of {ATTEMPTS}: the writer dropped a frame");
            std::fs::remove_file(path).unwrap();
        }
        panic!("the writer dropped frames in all {ATTEMPTS} captures of {frames} frames");
    }

    /// One capture with `pace` frame intervals between frames, stopped at
    /// the first dropped frame.
    fn capture_once(path: &Path, codec: Codec, fps: u32, frames: usize, pace: u32) -> Stats {
        objc2::rc::autoreleasepool(|_| {
            // Bitrate floor and quality as the recorder sets them.
            let quality = (codec == Codec::Hevc).then_some(0.95);
            let spec = VideoSpec { width: W, height: H, fps, codec, bitrate: 4_000_000, quality, pixel_format: PIX_BGRA };
            let mut sink = Sink::new(Some(path), &spec, false, false, AudioLayout::Stems, [0.0; 2], None, Scene::Screen, None).unwrap();
            let pool = make_pool(W, H, PIX_BGRA);
            let tps = TIMESCALE as i64 / fps as i64;
            // Capture timestamps are on the host clock, far from zero.
            let t0 = 1000 * TIMESCALE as i64;
            let mut end = 0.0;
            for n in 0..frames {
                let mut raw = std::ptr::null_mut();
                unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)) };
                let pb = unsafe { CFRetained::from_raw(NonNull::new(raw).unwrap()) };
                fill_bgra(&pb, &pixels(n), W as u32, H as u32, W * 4);
                let t = t0 + n as i64 * tps;
                sink.push_screen(unsafe { CMTime::new(t, TIMESCALE) }, pb);
                end = t as f64 / TIMESCALE as f64;
                sink.emit_until(end);
                if sink.stats.video_drops > 0 {
                    break;
                }
                // The writer expects real-time input: frames that come faster
                // than the encoder takes them are dropped.
                std::thread::sleep(Duration::from_secs_f64(pace as f64 / fps as f64));
            }
            sink.finish(end).unwrap()
        })
    }

    /// Write `frames` frames the way `glide synth` does (HEVC, 60 fps,
    /// frame reordering on).
    fn synth(path: &Path, frames: usize) {
        objc2::rc::autoreleasepool(|_| {
            let spec = WriterSpec {
                width: W as u32,
                height: H as u32,
                fps: 60,
                codec: model::Codec::Hevc,
                bitrate: (W * H) as u64 * 8,
                quality: None,
                audio: false,
            };
            let mut w = MovieWriter::create(path, &spec).unwrap();
            for n in 0..frames {
                let pb = w.buffer().unwrap();
                fill_bgra(&pb, &pixels(n), W as u32, H as u32, W * 4);
                w.push_frame(&pb).unwrap();
            }
            w.finish().unwrap();
        })
    }

    /// Full-sync samples, in decode order.
    fn keys(dec: &VtDecoder) -> Vec<usize> {
        (0..dec.index.key.len()).filter(|&i| dec.index.key[i] == i).collect()
    }

    /// Presentation position of sample `i` (the index painted into it).
    fn position(dec: &VtDecoder, i: usize) -> usize {
        dec.index.by_pts.iter().position(|&s| s == i).unwrap()
    }

    /// Source times of a deterministic walk: the first and last frame, the
    /// frames around the second full-sync sample, then short steps forward
    /// and back and long jumps, each a little off the frame's own time.
    fn walk(dec: &VtDecoder, steps: usize) -> Vec<f64> {
        let idx = &dec.index;
        let n = idx.pts.len();
        let frame = 2.0 * idx.half;
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut rand = move |m: usize| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x % m as u64) as usize
        };
        let mut at = vec![0, n - 1];
        if let Some(&k) = keys(dec).get(1) {
            let p = position(dec, k);
            at.extend([p - 1, p, p - 1]);
        }
        let mut p = 0;
        for s in 0..steps {
            p = match s % 3 {
                0 => (p + 1 + rand(6)).min(n - 1),
                1 => p.saturating_sub(1 + rand(6)),
                _ => rand(n),
            };
            at.push(p);
        }
        at.into_iter().map(|p| idx.pts[idx.by_pts[p]] + (rand(1000) as f64 / 1000.0 - 0.5) * 0.9 * frame).collect()
    }

    /// How a walk moved between consecutive targets.
    #[derive(Default, Debug)]
    struct Moves {
        forward_in_gop: usize,
        backward_in_gop: usize,
        across_gops: usize,
    }

    /// Decode `t` and check it against `FrameSource`: the same pts and the
    /// same picture, which carries the frame's own index.
    fn exact(dec: &mut VtDecoder, src: &mut FrameSource, t: f64) -> usize {
        let i = dec.target(t);
        let f = dec.decode(i, || true).unwrap().expect("not cancelled");
        let g = src.frame_at(t).cloned().expect("FrameSource frame");
        assert!((f.pts_ms - g.pts_ms).abs() < 1e-6, "pts at {t}: {} vs {}", f.pts_ms, g.pts_ms);
        assert_eq!(painted(&f), position(dec, i), "picture of sample {i}");
        assert_eq!(painted(&g), position(dec, i), "FrameSource picture at {t}");
        assert!(diff(&f, &g) < 1.0, "pixels of sample {i} differ from FrameSource");
        i
    }

    /// Walk the movie with both decoders: each target must match. For a
    /// movie without reordering, `count` also checks the work: a target
    /// after the last one in the same GOP costs only the samples between,
    /// any other target the samples from its full-sync sample.
    fn check_walk(path: &Path, count: bool) -> Moves {
        let mut dec = VtDecoder::open(path).unwrap();
        let mut src = FrameSource::open(path).unwrap();
        if count {
            assert_eq!(dec.index.by_pts, (0..dec.index.pts.len()).collect::<Vec<_>>(), "no reordering");
        }
        let mut moves = Moves::default();
        let mut prev: Option<usize> = None;
        for t in walk(&dec, 60) {
            objc2::rc::autoreleasepool(|_| {
                let before = dec.submitted;
                let i = exact(&mut dec, &mut src, t);
                let key = dec.index.key[i];
                let work = match prev {
                    Some(d) if dec.index.key[d] == key && d < i => {
                        moves.forward_in_gop += 1;
                        i - d
                    }
                    Some(d) if dec.index.key[d] == key && d > i => {
                        moves.backward_in_gop += 1;
                        i - key + 1
                    }
                    Some(d) if dec.index.key[d] != key => {
                        moves.across_gops += 1;
                        i - key + 1
                    }
                    _ => i - key + 1,
                };
                if count {
                    assert_eq!(dec.submitted - before, work, "samples decoded for {i} after {prev:?}");
                }
                prev = Some(i);
            });
        }
        moves
    }

    #[test]
    fn h264_capture_matches_frame_source() {
        let f = fixture("h264");
        capture(&f.path, Codec::H264, 30, 150);
        let dec = VtDecoder::open(&f.path).unwrap();
        assert_eq!(keys(&dec), [0, 60, 120], "a full-sync sample every 2 s");
        let moves = check_walk(&f.path, true);
        assert!(moves.forward_in_gop > 0 && moves.backward_in_gop > 0 && moves.across_gops > 0, "{moves:?}");
    }

    #[test]
    fn hevc_capture_matches_frame_source() {
        let f = fixture("hevc");
        capture(&f.path, Codec::Hevc, 60, 180);
        let keys = keys(&VtDecoder::open(&f.path).unwrap());
        eprintln!("HEVC capture: full-sync samples {keys:?}");
        let moves = check_walk(&f.path, true);
        assert!(moves.forward_in_gop > 0 && moves.backward_in_gop > 0, "{moves:?}");
        if keys.len() > 1 {
            assert!(moves.across_gops > 0, "{moves:?}");
        }
    }

    #[test]
    fn synth_reordered_matches_frame_source() {
        let f = fixture("synth");
        synth(&f.path, 300);
        let mut dec = VtDecoder::open(&f.path).unwrap();
        let n = dec.index.pts.len();
        assert_ne!(dec.index.by_pts, (0..n).collect::<Vec<_>>(), "frames are reordered");
        let leading = (0..n).filter(|&j| dec.index.leading(j)).count();
        eprintln!("synth: full-sync samples {:?}, {leading} leading pictures", keys(&dec));
        check_walk(&f.path, false);
        // Every frame from a fresh start, leading pictures included.
        for p in 0..n {
            objc2::rc::autoreleasepool(|_| {
                dec.decoded = None;
                dec.ahead.clear();
                let i = dec.index.by_pts[p];
                let f = dec.decode(i, || true).unwrap().unwrap();
                assert_eq!(painted(&f), p, "sample {i} from its full-sync sample");
            });
        }
    }

    #[test]
    fn reordered_playback_decodes_each_sample_once() {
        let f = fixture("synth-play");
        synth(&f.path, 300);
        let mut dec = VtDecoder::open(&f.path).unwrap();
        let mut src = FrameSource::open(&f.path).unwrap();
        let n = dec.index.pts.len();
        for p in 0..n {
            let t = dec.index.pts[dec.index.by_pts[p]];
            objc2::rc::autoreleasepool(|_| exact(&mut dec, &mut src, t));
        }
        assert_eq!(dec.submitted, n, "samples decoded for {n} frames in display order");
    }

    #[test]
    fn failures_recover_on_the_next_request() {
        let f = fixture("faults");
        capture(&f.path, Codec::H264, 30, 90);
        let mut dec = VtDecoder::open(&f.path).unwrap();
        let mut src = FrameSource::open(&f.path).unwrap();
        let t = |i: usize| i as f64 * 1000.0 / 30.0;
        for fault in [Fault::Decode, Fault::Callback, Fault::Wait] {
            objc2::rc::autoreleasepool(|_| {
                assert_eq!(exact(&mut dec, &mut src, t(5)), 5);
                dec.fault = Some(fault);
                assert!(dec.decode(20, || true).is_err(), "{fault:?} fails the request");
                assert_eq!(dec.fault, None, "{fault:?} was injected");
                // The next request starts again at the full-sync sample.
                let before = dec.submitted;
                assert_eq!(exact(&mut dec, &mut src, t(22)), 22);
                assert_eq!(dec.submitted - before, 23, "after {fault:?}");
            });
        }
    }

    #[test]
    fn a_new_session_keeps_frames_exact() {
        let f = fixture("sessions");
        capture(&f.path, Codec::H264, 30, 90);
        let mut dec = VtDecoder::open(&f.path).unwrap();
        let mut src = FrameSource::open(&f.path).unwrap();
        let t = |i: usize| i as f64 * 1000.0 / 30.0;
        objc2::rc::autoreleasepool(|_| {
            exact(&mut dec, &mut src, t(10));
            assert_eq!(dec.sessions, 1);
            // A format the session rejects at a full-sync sample: replaced.
            dec.fault = Some(Fault::Reject);
            exact(&mut dec, &mut src, t(70));
            assert_eq!(dec.sessions, 2);
            let before = dec.submitted;
            exact(&mut dec, &mut src, t(75));
            assert_eq!(dec.submitted - before, 5, "resumes in the new session");
            // Rejected inside a GOP: the request fails, the next one restarts.
            dec.fault = Some(Fault::Reject);
            assert!(dec.decode(80, || true).is_err());
            let before = dec.submitted;
            exact(&mut dec, &mut src, t(80));
            assert_eq!(dec.submitted - before, 21);
            // After a reset, the next request opens a session again.
            dec.reset();
            let before = dec.submitted;
            exact(&mut dec, &mut src, t(85));
            assert_eq!((dec.sessions, dec.submitted - before), (3, 26));
        });
    }

    #[test]
    fn a_stopped_walk_keeps_its_progress() {
        let f = fixture("stop");
        capture(&f.path, Codec::H264, 30, 60);
        let mut dec = VtDecoder::open(&f.path).unwrap();
        let mut src = FrameSource::open(&f.path).unwrap();
        objc2::rc::autoreleasepool(|_| {
            let asked = std::cell::Cell::new(0);
            let stop = || {
                asked.set(asked.get() + 1);
                false
            };
            assert!(dec.decode(30, stop).unwrap().is_none());
            assert_eq!((asked.get(), dec.submitted), (1, BATCH), "stops after one batch");
            exact(&mut dec, &mut src, 30.0 * 1000.0 / 30.0);
            assert_eq!(dec.submitted, 31, "continues where it stopped");
        });
    }
}
