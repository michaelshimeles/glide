//! Reading recorded media with AVAssetReader (hardware decode), and exact
//! frames by sample with VideoToolbox (`vt`).

pub mod vt;
pub mod writer;

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use objc2::rc::Retained;
use objc2_av_foundation::*;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange};
use objc2_core_video::*;
use objc2_foundation::{NSString, NSURL};

use crate::capture::mixer;
use crate::render::VideoFrame;
use crate::util::*;

fn open_asset(path: &Path) -> Result<Retained<AVURLAsset>> {
    if !path.exists() {
        bail!("missing media file {}", path.display());
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) })
}

#[allow(deprecated)]
fn tracks(asset: &AVURLAsset, kind: &NSString) -> Vec<Retained<AVAssetTrack>> {
    unsafe { asset.tracksWithMediaType(kind) }.iter().collect()
}

fn ms(t: CMTime) -> f64 {
    secs(t) * 1000.0
}

fn cmtime_ms(v: f64) -> CMTime {
    unsafe { CMTime::new((v * 1000.0).round() as i64, 1_000_000) }
}

/// One decoded video frame.
pub struct Frame {
    pub pts_ms: f64,
    pub pb: SendBox<CFRetained<CVPixelBuffer>>,
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Frame { pts_ms: self.pts_ms, pb: SendBox(self.pb.0.clone()) }
    }
}

impl Frame {
    pub fn size(&self) -> (u32, u32) {
        (CVPixelBufferGetWidth(&self.pb.0) as u32, CVPixelBufferGetHeight(&self.pb.0) as u32)
    }

    /// Lock the pixels and hand them to `f` as BGRA.
    pub fn with_bgra<R>(&self, f: impl FnOnce(&VideoFrame) -> R) -> R {
        let pb = &self.pb.0;
        unsafe {
            CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly);
            let (w, h) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb));
            let stride = CVPixelBufferGetBytesPerRow(pb);
            let base = CVPixelBufferGetBaseAddress(pb) as *const u8;
            let bytes = std::slice::from_raw_parts(base, stride * h);
            let r = f(&VideoFrame { width: w as u32, height: h as u32, stride, bgra: bytes });
            CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly);
            r
        }
    }
}

/// Sequential video reader that can restart at any time.
pub struct VideoReader {
    asset: Retained<AVURLAsset>,
    track: Retained<AVAssetTrack>,
    reader: Option<Retained<AVAssetReader>>,
    output: Option<Retained<AVAssetReaderTrackOutput>>,
    pub size: (u32, u32),
    pub fps: f64,
    pub duration_ms: f64,
    /// Time of the first sample in the file (frames are reported relative to it).
    origin_ms: f64,
}

unsafe impl Send for VideoReader {}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        let asset = open_asset(path)?;
        let track = tracks(&asset, unsafe { AVMediaTypeVideo.unwrap() })
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no video track in {}", path.display()))?;
        let size = unsafe { track.naturalSize() };
        let fps = unsafe { track.nominalFrameRate() } as f64;
        let range = unsafe { track.timeRange() };
        let mut r = VideoReader {
            asset,
            track,
            reader: None,
            output: None,
            size: (size.width.round() as u32, size.height.round() as u32),
            fps: if fps > 0.0 { fps } else { 60.0 },
            duration_ms: ms(range.duration),
            origin_ms: ms(range.start),
        };
        r.seek(0.0)?;
        Ok(r)
    }

    /// Restart decoding so the next frame is the one shown at `t_ms`.
    pub fn seek(&mut self, t_ms: f64) -> Result<()> {
        if let Some(r) = self.reader.take() {
            unsafe { r.cancelReading() };
        }
        unsafe {
            let reader = AVAssetReader::assetReaderWithAsset_error(&self.asset)
                .map_err(|e| anyhow!("asset reader: {}", e.localizedDescription()))?;
            let settings = dict(vec![
                (cf_ns(kCVPixelBufferPixelFormatTypeKey), num_i(PIX_BGRA as i64)),
                (cf_ns(kCVPixelBufferIOSurfacePropertiesKey), obj(&*dict(vec![]))),
                (cf_ns(kCVPixelBufferMetalCompatibilityKey), num_b(true)),
            ]);
            let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(&self.track, Some(&settings));
            output.setAlwaysCopiesSampleData(false);
            let start = (t_ms - 1000.0 / self.fps).max(0.0) + self.origin_ms;
            let range = CMTimeRange { start: cmtime_ms(start), duration: cmtime_ms(self.duration_ms + 10_000.0) };
            reader.setTimeRange(range);
            reader.addOutput(&output);
            if !reader.startReading() {
                let e = reader.error().map(|e| e.localizedDescription().to_string()).unwrap_or_default();
                bail!("start reading: {e}");
            }
            self.reader = Some(reader);
            self.output = Some(output);
        }
        Ok(())
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        let output = self.output.as_ref()?;
        loop {
            let sb: Retained<CMSampleBuffer> = unsafe { output.copyNextSampleBuffer() }?;
            let pts = ms(unsafe { sb.presentation_time_stamp() }) - self.origin_ms;
            if let Some(pb) = unsafe { sb.image_buffer() } {
                return Some(Frame { pts_ms: pts, pb: SendBox(pb) });
            }
        }
    }
}

/// Random-access frame lookup on top of a sequential reader: cheap when
/// times move forward, re-seeks on jumps.
pub struct FrameSource {
    reader: VideoReader,
    current: Option<Frame>,
    upcoming: Option<Frame>,
}

impl FrameSource {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(FrameSource { reader: VideoReader::open(path)?, current: None, upcoming: None })
    }

    pub fn size(&self) -> (u32, u32) {
        self.reader.size
    }

    pub fn duration_ms(&self) -> f64 {
        self.reader.duration_ms
    }

    /// The frame on screen at `t_ms` (the latest frame with pts <= t).
    pub fn frame_at(&mut self, t_ms: f64) -> Option<&Frame> {
        let half = 0.5 * 1000.0 / self.reader.fps;
        let jump = match &self.current {
            Some(c) => t_ms + half < c.pts_ms || t_ms > c.pts_ms + 1500.0,
            None => self.upcoming.is_none(),
        };
        if jump {
            self.reader.seek(t_ms).ok()?;
            self.current = None;
            self.upcoming = self.reader.next_frame();
        }
        while self.upcoming.as_ref().is_some_and(|u| u.pts_ms <= t_ms + half) {
            self.current = self.upcoming.take();
            self.upcoming = self.reader.next_frame();
        }
        if self.current.is_none() {
            // Before the first frame: show the first one.
            self.current = self.upcoming.take();
            self.upcoming = self.reader.next_frame();
        }
        self.current.as_ref()
    }
}

/// Decodes the frames for a known list of times on a background thread,
/// so hardware decode overlaps rendering and encoding.
pub struct Prefetch {
    rx: std::sync::mpsc::Receiver<(usize, Option<Frame>)>,
    _thread: std::thread::JoinHandle<()>,
}

impl Prefetch {
    pub fn spawn(path: std::path::PathBuf, times: Vec<f64>) -> Result<Self> {
        // Open eagerly so errors surface here.
        let mut src = FrameSource::open(&path)?;
        let (tx, rx) = std::sync::mpsc::sync_channel(6);
        let thread = std::thread::spawn(move || {
            for (i, t) in times.into_iter().enumerate() {
                let f = objc2::rc::autoreleasepool(|_| src.frame_at(t).cloned());
                if tx.send((i, f)).is_err() {
                    break;
                }
            }
        });
        Ok(Prefetch { rx, _thread: thread })
    }

    /// The frame for output index `i` (indices must be requested in order).
    pub fn get(&self, i: usize) -> Option<Frame> {
        loop {
            let (j, f) = self.rx.recv().ok()?;
            if j == i {
                return f;
            }
            if j > i {
                return None;
            }
        }
    }
}

/// Decode an audio track to 48 kHz interleaved stereo.
pub fn read_audio(path: &Path, track_index: usize) -> Result<Vec<[f32; 2]>> {
    let asset = open_asset(path)?;
    let all = tracks(&asset, unsafe { AVMediaTypeAudio.unwrap() });
    let track = all.get(track_index).ok_or_else(|| anyhow!("no audio track {track_index} in {}", path.display()))?;
    let origin = ms(unsafe { track.timeRange() }.start);
    unsafe {
        let reader = AVAssetReader::assetReaderWithAsset_error(&asset).map_err(|e| anyhow!("asset reader: {}", e.localizedDescription()))?;
        let settings = dict(vec![
            (&*ns("AVFormatIDKey"), num_i(fourcc(b"lpcm") as i64)),
            (&*ns("AVSampleRateKey"), num_f(mixer::RATE)),
            (&*ns("AVNumberOfChannelsKey"), num_i(2)),
            (&*ns("AVLinearPCMBitDepthKey"), num_i(32)),
            (&*ns("AVLinearPCMIsFloatKey"), num_b(true)),
            (&*ns("AVLinearPCMIsNonInterleaved"), num_b(false)),
            (&*ns("AVLinearPCMIsBigEndianKey"), num_b(false)),
        ]);
        let output = AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(track, Some(&settings));
        reader.addOutput(&output);
        if !reader.startReading() {
            bail!("start reading audio");
        }
        let mut out: Vec<[f32; 2]> = vec![];
        while let Some(sb) = output.copyNextSampleBuffer() {
            if let Some((pts, _rate, frames)) = mixer::decode(&sb) {
                // Place by timestamp so gaps (pauses, drops) stay in sync.
                let at = (((pts * 1000.0 - origin) / 1000.0) * mixer::RATE).round().max(0.0) as usize;
                if at > out.len() && at - out.len() > 48 {
                    out.resize(at, [0.0, 0.0]);
                }
                out.extend_from_slice(&frames);
            }
        }
        Ok(out)
    }
}

/// Number of audio tracks in a file.
pub fn audio_track_count(path: &Path) -> Result<usize> {
    let asset = open_asset(path).context("opening media")?;
    Ok(tracks(&asset, unsafe { AVMediaTypeAudio.unwrap() }).len())
}
