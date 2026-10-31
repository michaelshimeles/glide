//! Offline movie writer for export: VideoToolbox H.264/HEVC through
//! AVAssetWriter, plus an optional AAC stereo track.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_av_foundation::*;
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMAudioFormatDescription, CMTime};
use objc2_core_video::*;
use objc2_foundation::NSURL;

use crate::capture::mixer;
use crate::project::model::Codec;
use crate::util::*;

const TIMESCALE: i32 = 60_000;

/// Presentation time of frame `frame` in [`TIMESCALE`] units. Multiplying before dividing keeps
/// rates that don't divide 60 000 (7 fps, say) from drifting a few ticks short each second.
pub(crate) fn frame_pts(frame: u64, fps: u32) -> i64 {
    (frame as i64 * TIMESCALE as i64) / fps as i64
}

pub struct WriterSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// Average bitrate, used when `quality` is None (fit to a file size).
    pub bitrate: u64,
    /// Constant quality 0..1 (VideoToolbox spends what each frame needs).
    pub quality: Option<f64>,
    pub audio: bool,
}

pub struct MovieWriter {
    writer: Retained<AVAssetWriter>,
    video: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    audio: Option<Retained<AVAssetWriterInput>>,
    audio_fmt: CFRetained<CMAudioFormatDescription>,
    audio_written: usize,
    soundtrack: Option<std::sync::Arc<Vec<[f32; 2]>>>,
    audio_done: bool,
    fps: u32,
    pub frames: u64,
    path: std::path::PathBuf,
}

unsafe impl Send for MovieWriter {}

fn video_settings(s: &WriterSpec, quality: Option<f64>) -> Retained<Dict> {
    unsafe {
        let rate = match quality {
            Some(q) => (AVVideoQualityKey.unwrap(), num_f(q)),
            None => (AVVideoAverageBitRateKey.unwrap(), num_i(s.bitrate as i64)),
        };
        let mut compression = vec![
            rate,
            (AVVideoExpectedSourceFrameRateKey.unwrap(), num_i(s.fps as i64)),
            // A keyframe every 2 s, in frames: some HEVC encoders (Intel
            // Macs) reject the duration key.
            (AVVideoMaxKeyFrameIntervalKey.unwrap(), num_i(s.fps as i64 * 2)),
            (AVVideoAllowFrameReorderingKey.unwrap(), num_b(true)),
        ];
        let codec = match s.codec {
            Codec::H264 => {
                compression
                    .push((AVVideoProfileLevelKey.unwrap(), obj(AVVideoProfileLevelH264HighAutoLevel.unwrap())));
                AVVideoCodecTypeH264.unwrap()
            }
            Codec::Prores => AVVideoCodecTypeAppleProRes4444.unwrap(),
            Codec::Hevc => {
                // kVTProfileLevel_HEVC_Main_AutoLevel: 8-bit 4:2:0, level
                // chosen from the size and frame rate.
                compression.push((AVVideoProfileLevelKey.unwrap(), obj(&*ns("HEVC_Main_AutoLevel"))));
                AVVideoCodecTypeHEVC.unwrap()
            }
        };
        // Frames are sRGB-encoded screen pixels (ScreenCaptureKit captures
        // in sRGB and nothing in between converts): BT.709 primaries and
        // matrix with the sRGB transfer function, so players show exactly
        // the recorded colours. A BT.709 transfer tag would make QuickTime
        // and Safari decode with a different curve and lift the shadows.
        let color = dict(vec![
            (AVVideoColorPrimariesKey.unwrap(), obj(AVVideoColorPrimaries_ITU_R_709_2.unwrap())),
            // AVVideoTransferFunction_IEC_sRGB's value; the symbol itself is
            // macOS 15+ and would stop the binary loading on macOS 14.
            (AVVideoTransferFunctionKey.unwrap(), obj(&*ns("IEC_sRGB"))),
            (AVVideoYCbCrMatrixKey.unwrap(), obj(AVVideoYCbCrMatrix_ITU_R_709_2.unwrap())),
        ]);
        let mut settings = vec![
            (AVVideoCodecKey.unwrap(), obj(codec)),
            (AVVideoWidthKey.unwrap(), num_i(s.width as i64)),
            (AVVideoHeightKey.unwrap(), num_i(s.height as i64)),
            (AVVideoColorPropertiesKey.unwrap(), obj(&*color)),
        ];
        // ProRes is fixed-quality intra-frame: no rate or GOP settings.
        if s.codec != Codec::Prores {
            settings.push((AVVideoCompressionPropertiesKey.unwrap(), obj(&*dict(compression))));
        }
        dict(settings)
    }
}

impl MovieWriter {
    pub fn create(path: &Path, s: &WriterSpec) -> Result<Self> {
        if s.fps == 0 {
            bail!("frame rate must be at least 1 (got {})", s.fps);
        }
        if s.width == 0 || s.height == 0 {
            bail!("frame size must be at least 1x1 (got {}x{})", s.width, s.height);
        }
        if path.exists() {
            std::fs::remove_file(path).with_context(|| format!("replacing {}", path.display()))?;
        }
        let abs = std::path::absolute(path)?;
        let file_type = match path.extension().and_then(|e| e.to_str()) {
            Some("mov") => unsafe { AVFileTypeQuickTimeMovie }.unwrap(),
            _ => unsafe { AVFileTypeMPEG4 }.unwrap(),
        };
        unsafe {
            let url = NSURL::fileURLWithPath(&ns(abs.to_str().context("non-UTF8 path")?));
            let writer = AVAssetWriter::initWithURL_fileType_error(AVAssetWriter::alloc(), &url, file_type)
                .map_err(|e| anyhow!("AVAssetWriter: {}", e.localizedDescription()))?;
            writer.setShouldOptimizeForNetworkUse(true);
            // Not every encoder takes constant quality (an Intel Mac's H.264
            // doesn't): then encode for the spec's bitrate instead.
            let mut settings = video_settings(s, s.quality);
            if s.quality.is_some() && !writer.canApplyOutputSettings_forMediaType(Some(&settings), AVMediaTypeVideo.unwrap()) {
                settings = video_settings(s, None);
            }
            let video = AVAssetWriterInput::initWithMediaType_outputSettings(
                AVAssetWriterInput::alloc(),
                AVMediaTypeVideo.unwrap(),
                Some(&settings),
            );
            video.setExpectsMediaDataInRealTime(false);
            video.setMediaTimeScale(TIMESCALE);
            let attrs = dict(vec![
                (cf_ns(kCVPixelBufferPixelFormatTypeKey), num_i(PIX_BGRA as i64)),
                (cf_ns(kCVPixelBufferWidthKey), num_i(s.width as i64)),
                (cf_ns(kCVPixelBufferHeightKey), num_i(s.height as i64)),
                (cf_ns(kCVPixelBufferIOSurfacePropertiesKey), obj(&*dict(vec![]))),
                (cf_ns(kCVPixelBufferMetalCompatibilityKey), num_b(true)),
            ]);
            let adaptor = AVAssetWriterInputPixelBufferAdaptor::initWithAssetWriterInput_sourcePixelBufferAttributes(
                AVAssetWriterInputPixelBufferAdaptor::alloc(),
                &video,
                Some(&attrs),
            );
            if !writer.canAddInput(&video) {
                bail!("cannot add video input ({}x{} {:?})", s.width, s.height, s.codec);
            }
            writer.addInput(&video);
            let audio = if s.audio {
                let a = AVAssetWriterInput::initWithMediaType_outputSettings(
                    AVAssetWriterInput::alloc(),
                    AVMediaTypeAudio.unwrap(),
                    Some(&dict(vec![
                        (&*ns("AVFormatIDKey"), num_i(fourcc(b"aac ") as i64)),
                        (&*ns("AVSampleRateKey"), num_f(mixer::RATE)),
                        (&*ns("AVNumberOfChannelsKey"), num_i(2)),
                        (&*ns("AVEncoderBitRateKey"), num_i(192_000)),
                    ])),
                );
                a.setExpectsMediaDataInRealTime(false);
                if !writer.canAddInput(&a) {
                    bail!("cannot add audio input");
                }
                writer.addInput(&a);
                Some(a)
            } else {
                None
            };
            if !writer.startWriting() {
                bail!("start writing: {}", err_text(&writer));
            }
            writer.startSessionAtSourceTime(CMTime::new(0, TIMESCALE));
            Ok(MovieWriter {
                writer,
                video,
                adaptor,
                audio,
                audio_fmt: mixer::stereo_format(),
                audio_written: 0,
                soundtrack: None,
                audio_done: false,
                fps: s.fps,
                frames: 0,
                path: abs,
            })
        }
    }

    /// A pixel buffer from the encoder's pool (IOSurface-backed BGRA).
    pub fn buffer(&self) -> Result<CFRetained<CVPixelBuffer>> {
        unsafe {
            let pool = self.adaptor.pixelBufferPool().ok_or_else(|| anyhow!("no pixel buffer pool: {}", err_text(&self.writer)))?;
            let mut out = std::ptr::null_mut();
            CVPixelBufferPool::create_pixel_buffer(None, &pool, std::ptr::NonNull::from(&mut out));
            Ok(CFRetained::from_raw(std::ptr::NonNull::new(out).ok_or_else(|| anyhow!("pixel buffer alloc"))?))
        }
    }

    /// Interleaved stereo at 48 kHz to write alongside the frames.
    pub fn set_soundtrack(&mut self, mix: std::sync::Arc<Vec<[f32; 2]>>) {
        self.soundtrack = Some(mix);
    }

    fn stalled(&self, input: &AVAssetWriterInput) -> anyhow::Error {
        let which = if std::ptr::eq(input, &*self.video) { "video" } else { "audio" };
        anyhow!(
            "encoder stalled waiting for {which} (video frames {}, audio {:.2} s)",
            self.frames,
            self.audio_written as f64 / mixer::RATE
        )
    }

    /// Append one audio chunk if the audio input will take it.
    fn feed_audio_chunk(&mut self, until: usize) -> Result<bool> {
        let (Some(audio), Some(mix)) = (self.audio.clone(), self.soundtrack.clone()) else { return Ok(false) };
        if self.audio_done {
            return Ok(false);
        }
        if self.audio_written >= mix.len() {
            // Tell the writer the track is complete so video isn't held
            // back waiting to interleave with audio that never comes.
            unsafe { audio.markAsFinished() };
            self.audio_done = true;
            return Ok(false);
        }
        let until = until.min(mix.len());
        if self.audio_written >= until || !unsafe { audio.isReadyForMoreMediaData() } {
            return Ok(false);
        }
        let n = (until - self.audio_written).min(4800);
        let chunk: Vec<f32> = mix[self.audio_written..self.audio_written + n].iter().flat_map(|s| [s[0], s[1]]).collect();
        let pts = unsafe { CMTime::new(self.audio_written as i64, mixer::RATE as i32) };
        let sb = mixer::pcm_buffer(&self.audio_fmt, chunk, pts).ok_or_else(|| anyhow!("audio buffer"))?;
        if !unsafe { audio.appendSampleBuffer(&sb) } {
            bail!("append audio: {}", err_text(&self.writer));
        }
        self.audio_written += n;
        Ok(true)
    }

    /// Append the next frame (frames are evenly spaced at the export fps).
    /// Audio is fed while the video input waits, so neither starves.
    pub fn push_frame(&mut self, pb: &CVPixelBuffer) -> Result<()> {
        let t_s = self.frames as f64 / self.fps as f64;
        let lead = ((t_s + 1.0) * mixer::RATE) as usize;
        while self.feed_audio_chunk(lead)? {}
        let mut spins = 0;
        while !unsafe { self.video.isReadyForMoreMediaData() } {
            if unsafe { self.writer.status() } == AVAssetWriterStatus::Failed {
                bail!("encoder failed: {}", err_text(&self.writer));
            }
            // The writer may want more audio before taking more video.
            let ahead = ((t_s + 3.0) * mixer::RATE) as usize;
            if !self.feed_audio_chunk(ahead)? {
                // 1 ms: far under a frame, without thousands of wakeups a second.
                std::thread::sleep(Duration::from_millis(1));
                spins += 1;
                if spins > 10_000 {
                    return Err(self.stalled(&self.video));
                }
            }
        }
        let t = unsafe { CMTime::new(frame_pts(self.frames, self.fps), TIMESCALE) };
        if !unsafe { self.adaptor.appendPixelBuffer_withPresentationTime(pb, t) } {
            bail!("append frame {}: {}", self.frames, err_text(&self.writer));
        }
        self.frames += 1;
        Ok(())
    }

    /// Write any remaining audio.
    fn drain_audio(&mut self) -> Result<()> {
        let Some(audio) = self.audio.clone() else { return Ok(()) };
        let Some(len) = self.soundtrack.as_ref().map(|m| m.len()) else { return Ok(()) };
        let mut spins = 0;
        while !self.audio_done {
            if !self.feed_audio_chunk(len)? {
                if unsafe { self.writer.status() } == AVAssetWriterStatus::Failed {
                    bail!("encoder failed: {}", err_text(&self.writer));
                }
                // 1 ms: far under a frame, without thousands of wakeups a second.
                std::thread::sleep(Duration::from_millis(1));
                spins += 1;
                if spins > 10_000 {
                    return Err(self.stalled(&audio));
                }
            }
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.drain_audio()?;
        unsafe {
            let end = CMTime::new(frame_pts(self.frames, self.fps), TIMESCALE);
            self.writer.endSessionAtSourceTime(end);
            self.video.markAsFinished();
            if let Some(a) = &self.audio
                && !self.audio_done
            {
                a.markAsFinished();
            }
            let (tx, rx) = mpsc::channel();
            let block = RcBlock::new(move || {
                let _ = tx.send(());
            });
            self.writer.finishWritingWithCompletionHandler(&block);
            rx.recv_timeout(Duration::from_secs(120)).context("timed out finalizing movie")?;
            if self.writer.status() != AVAssetWriterStatus::Completed {
                bail!("writing failed: {}", err_text(&self.writer));
            }
        }
        remove_fast_start_leftovers(&self.path);
        Ok(())
    }

    pub fn cancel(self) {
        unsafe { self.writer.cancelWriting() };
    }
}

/// Optimising for network use (index first) rewrites the movie through a
/// `<name>.sb-…` temporary that AVFoundation can leave next to the finished
/// file; remove it.
pub(crate) fn remove_fast_start_leftovers(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let prefix = format!("{name}.sb-");
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn err_text(w: &AVAssetWriter) -> String {
    unsafe { w.error() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "unknown error".into())
}

/// Copy tightly packed BGRA rows into a pixel buffer.
pub fn fill_bgra(pb: &CVPixelBuffer, bgra: &[u8], width: u32, height: u32, src_stride: usize) {
    unsafe {
        CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags(0));
        let dst = CVPixelBufferGetBaseAddress(pb) as *mut u8;
        let stride = CVPixelBufferGetBytesPerRow(pb);
        let row = (width * 4) as usize;
        for y in 0..height as usize {
            std::ptr::copy_nonoverlapping(bgra.as_ptr().add(y * src_stride), dst.add(y * stride), row);
        }
        CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pts_matches_the_old_formula_for_the_offered_rates() {
        for fps in [10u32, 15, 20, 24, 25, 30, 50, 60] {
            for frame in 0..20_000u64 {
                assert_eq!(frame_pts(frame, fps), frame as i64 * (TIMESCALE as i64 / fps as i64), "frame {frame} at {fps} fps");
            }
        }
    }

    #[test]
    fn frame_pts_lands_on_the_second_for_rates_that_do_not_divide_the_timescale() {
        // 60_000 / 7 truncates to 8571, so the old formula gave 59_997.
        assert_eq!(frame_pts(7, 7), 60_000);
        assert_eq!(frame_pts(70, 7), 600_000);
    }

    #[test]
    fn a_frame_rate_of_zero_is_refused_and_an_existing_file_is_kept() {
        let dir = std::env::temp_dir().join(format!("glide-writer-fps0-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keep.mp4");
        std::fs::write(&path, b"existing").unwrap();
        let spec = WriterSpec { width: 64, height: 64, fps: 0, codec: Codec::H264, bitrate: 1_000_000, quality: None, audio: false };
        let err = MovieWriter::create(&path, &spec).err().expect("fps 0 is refused");
        assert!(err.to_string().contains("frame rate"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), b"existing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
