//! Preview audio: the edit's soundtrack, mixed in the background and played
//! through CoreAudio (cpal) in step with the playhead. The master volume is
//! applied live in the output callback; other edits (mutes, clip volumes,
//! music, voice-overs) re-mix in the background and swap in while playing.
//! The playhead is the clock: the audio is re-seeked if it drifts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

use crate::capture::mixer::RATE;
use crate::engine::timemap::TimeMap;
use crate::export::audio::{Stereo, sample, soft_clip};
use crate::project::model::{AudioStyle, Project, Slice, VoiceOver};
use crate::project::recording::Recording;
use crate::util::SendBox;

/// Re-seek when the audio is this far from the playhead.
const MAX_DRIFT_MS: f64 = 60.0;

pub fn write_wav(path: &Path, samples: &[[f32; 2]]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data_len = (samples.len() * 4) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(RATE as u32).to_le_bytes())?;
    f.write_all(&(RATE as u32 * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        for c in s {
            f.write_all(&((c.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
    }
    f.flush()
}

/// What the output has played since the meter was reset.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    /// Output frames rendered while playing.
    pub frames: u64,
    /// RMS and peak of what was sent to the device (0..1).
    pub rms: f32,
    pub peak: f32,
    /// RMS of the soundtrack before the master volume.
    pub source_rms: f32,
}

impl Meter {
    pub fn rms_db(&self) -> f32 {
        20.0 * self.rms.max(1e-9).log10()
    }

    /// Output level relative to the soundtrack (the volume as heard), or
    /// None while the soundtrack is silent.
    pub fn gain_db(&self) -> Option<f32> {
        (self.source_rms > 1e-5).then(|| 20.0 * (self.rms.max(1e-9) / self.source_rms).log10())
    }
}

/// State shared with the output callback.
struct Shared {
    /// The soundtrack before the master volume (None = nothing audible).
    mix: Mutex<Option<Arc<Vec<Stereo>>>>,
    /// Bumped when `mix` changes.
    mix_gen: AtomicU64,
    /// Newest requested mix; older background mixes are dropped.
    requested: AtomicU64,
    /// Playback position in soundtrack frames (f64 bits).
    pos: AtomicU64,
    playing: AtomicBool,
    /// Master volume (f32 bits).
    volume: AtomicU32,
    /// The device went away or changed format: rebuild the stream.
    broken: AtomicBool,
    /// Bumped by every output callback (to notice a stream that stopped).
    heartbeat: AtomicU64,
    meter_reset: AtomicBool,
    meter_frames: AtomicU64,
    /// Sums of squares out and in (f64 bits), and peak (f32 bits).
    meter_energy: AtomicU64,
    meter_source: AtomicU64,
    meter_peak: AtomicU32,
}

/// The parts of a project the soundtrack depends on, besides the master
/// volume (which is applied live).
#[derive(Clone, PartialEq)]
struct MixKey {
    audio: AudioStyle,
    slices: Vec<Slice>,
    voiceovers: Vec<VoiceOver>,
}

impl MixKey {
    fn of(p: &Project) -> MixKey {
        let mut audio = p.style.audio.clone();
        audio.volume = 1.0;
        MixKey { audio, slices: p.timeline.slices.clone(), voiceovers: p.timeline.voiceovers.clone() }
    }
}

struct Output {
    stream: SendBox<cpal::Stream>,
    device: Option<cpal::DeviceId>,
    /// Last heartbeat seen and when it last moved.
    beat: (u64, Instant),
}

pub struct AudioPreview {
    shared: Arc<Shared>,
    output: Option<Output>,
    /// Edits arrive per frame while dragging a slider: mix once they settle
    /// (the Instant is when the request is due).
    request: Option<(Instant, Project, Recording, PathBuf)>,
    mixed: Option<MixKey>,
    /// Mixes running in the background.
    mixing: Arc<AtomicU64>,
    /// Last output error, for the status line.
    pub error: Option<String>,
    /// When the output was last restarted (at most once a second).
    restarted: Option<Instant>,
    /// Held while playing, so App Nap doesn't throttle the picture when
    /// Glide isn't the frontmost app.
    activity: Option<Activity>,
}

type Activity = SendBox<objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>>>;

impl AudioPreview {
    pub fn new() -> Self {
        AudioPreview {
            shared: Arc::new(Shared {
                mix: Mutex::new(None),
                mix_gen: AtomicU64::new(0),
                requested: AtomicU64::new(0),
                pos: AtomicU64::new(0f64.to_bits()),
                playing: AtomicBool::new(false),
                volume: AtomicU32::new(1f32.to_bits()),
                broken: AtomicBool::new(false),
                heartbeat: AtomicU64::new(0),
                meter_reset: AtomicBool::new(false),
                meter_frames: AtomicU64::new(0),
                meter_energy: AtomicU64::new(0f64.to_bits()),
                meter_source: AtomicU64::new(0f64.to_bits()),
                meter_peak: AtomicU32::new(0f32.to_bits()),
            }),
            output: None,
            request: None,
            mixed: None,
            mixing: Arc::new(AtomicU64::new(0)),
            error: None,
            restarted: None,
            activity: None,
        }
    }

    /// The project changed: apply the volume now, and re-mix (debounced;
    /// see `tick`) if anything else that's heard changed.
    pub fn rebuild(&mut self, project: &Project, rec: &Recording, package: &Path) {
        self.set_volume(project.style.audio.volume);
        if self.mixed.as_ref() != Some(&MixKey::of(project)) {
            self.request = Some((Instant::now() + Duration::from_millis(120), project.clone(), rec.clone(), package.to_path_buf()));
        } else {
            // Back to what is already mixed: a queued request is stale.
            self.request = None;
        }
    }

    /// Mix right away (opening a project).
    pub fn rebuild_now(&mut self, project: &Project, rec: &Recording, package: &Path) {
        self.set_volume(project.style.audio.volume);
        self.request = Some((Instant::now(), project.clone(), rec.clone(), package.to_path_buf()));
        self.start_due();
    }

    fn set_volume(&self, v: f64) {
        self.shared.volume.store((v as f32).to_bits(), Ordering::Relaxed);
    }

    fn start_due(&mut self) {
        if !self.request.as_ref().is_some_and(|r| Instant::now() >= r.0) {
            return;
        }
        let (_, p, r, dir) = self.request.take().unwrap();
        self.mixed = Some(MixKey::of(&p));
        let shared = self.shared.clone();
        let gen_ = shared.requested.fetch_add(1, Ordering::SeqCst) + 1;
        let mixing = self.mixing.clone();
        mixing.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let stale = || shared.requested.load(Ordering::SeqCst) != gen_;
            if !stale() {
                let map = TimeMap::new(&p.timeline.slices, r.meta.duration_ms);
                let mix = crate::export::audio::mix_unmastered(&p, &r, Some(&dir), &map);
                let mut slot = shared.mix.lock().unwrap_or_else(|e| e.into_inner());
                if !stale() {
                    *slot = mix.map(Arc::new);
                    shared.mix_gen.fetch_add(1, Ordering::Release);
                }
            }
            mixing.fetch_sub(1, Ordering::SeqCst);
        });
    }

    /// Is a re-mix waiting or running?
    pub fn mixing(&self) -> bool {
        self.request.is_some() || self.mixing.load(Ordering::SeqCst) > 0
    }

    /// Is there anything to hear (once mixed)?
    pub fn has_audio(&self) -> bool {
        self.shared.mix.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    /// Start playing from `at_ms` (playback time). Plays silence until the
    /// mix is ready, then picks up in step.
    /// If the output can't be opened, `sync` keeps retrying while playing.
    pub fn play(&mut self, at_ms: f64) {
        self.seek(at_ms);
        self.shared.playing.store(true, Ordering::Relaxed);
        if let Err(e) = self.ensure_output() {
            self.error = Some(format!("No audio output: {e:#}"));
            eprintln!("glide: audio output: {e:#}");
            self.output = None;
        }
        if self.activity.is_none() {
            use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
            let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(NSActivityOptions::UserInitiated, &NSString::from_str("playing a recording"));
            self.activity = Some(SendBox(token));
        }
        if let Some(o) = &mut self.output {
            o.beat = (u64::MAX, Instant::now());
            if let Err(e) = o.stream.0.play() {
                self.error = Some(format!("Couldn't start audio output: {e}"));
                eprintln!("glide: audio output: {e}");
                self.output = None;
            }
        }
    }

    pub fn pause(&mut self) {
        self.shared.playing.store(false, Ordering::Relaxed);
        if let Some(t) = self.activity.take() {
            // SAFETY: the token came from beginActivity and is ended once.
            unsafe { objc2_foundation::NSProcessInfo::processInfo().endActivity(&t.0) };
        }
        if let Some(o) = &self.output {
            // Stopping the device lets the Mac sleep; restarting is instant.
            let _ = o.stream.0.pause();
        }
    }

    fn seek(&self, at_ms: f64) {
        let frames = (at_ms.max(0.0) / 1000.0 * RATE).max(0.0);
        self.shared.pos.store(frames.to_bits(), Ordering::Relaxed);
    }

    /// Where the audio is, in playback ms.
    pub fn position_ms(&self) -> f64 {
        f64::from_bits(self.shared.pos.load(Ordering::Relaxed)) / RATE * 1000.0
    }

    /// Keep the audio on the playhead; returns the drift before correcting.
    /// Also restarts the output if the device changed or stopped calling
    /// back (another app reconfiguring it, headphones plugged in).
    pub fn sync(&mut self, playhead_ms: f64) -> f64 {
        if !self.is_playing() {
            return 0.0;
        }
        let beat = self.shared.heartbeat.load(Ordering::Relaxed);
        let stalled = self.output.as_mut().is_none_or(|o| {
            if o.beat.0 != beat {
                o.beat = (beat, Instant::now());
            }
            o.beat.1.elapsed() > Duration::from_millis(500)
        });
        let recently = self.restarted.is_some_and(|t| t.elapsed() < Duration::from_secs(1));
        if (stalled || self.shared.broken.load(Ordering::Relaxed)) && !recently {
            eprintln!("glide: restarting audio output");
            self.restarted = Some(Instant::now());
            self.output = None;
            self.play(playhead_ms);
            return 0.0;
        }
        let drift = self.position_ms() - playhead_ms;
        if drift.abs() > MAX_DRIFT_MS {
            self.seek(playhead_ms);
        }
        drift
    }

    pub fn meter(&self) -> Meter {
        let s = &self.shared;
        let frames = s.meter_frames.load(Ordering::Relaxed);
        let rms = |bits: &AtomicU64| if frames > 0 { (f64::from_bits(bits.load(Ordering::Relaxed)) / frames as f64).sqrt() as f32 } else { 0.0 };
        Meter { frames, rms: rms(&s.meter_energy), peak: f32::from_bits(s.meter_peak.load(Ordering::Relaxed)), source_rms: rms(&s.meter_source) }
    }

    pub fn reset_meter(&self) {
        let s = &self.shared;
        s.meter_frames.store(0, Ordering::Relaxed);
        s.meter_energy.store(0f64.to_bits(), Ordering::Relaxed);
        s.meter_source.store(0f64.to_bits(), Ordering::Relaxed);
        s.meter_peak.store(0f32.to_bits(), Ordering::Relaxed);
        s.meter_reset.store(true, Ordering::Release);
    }

    /// Open (or re-open, after a device change) the default output.
    fn ensure_output(&mut self) -> Result<()> {
        let host = cpal::default_host();
        let device = host.default_output_device().context("no output device")?;
        let id = device.id().ok();
        let stale = self.shared.broken.swap(false, Ordering::Relaxed) || self.output.as_ref().is_some_and(|o| o.device != id);
        if stale {
            self.output = None;
        }
        if self.output.is_none() {
            let stream = open_stream(&device, self.shared.clone())?;
            self.output = Some(Output { stream: SendBox(stream), device: id, beat: (u64::MAX, Instant::now()) });
        }
        Ok(())
    }

    /// Start a settled re-mix.
    pub fn tick(&mut self) {
        self.start_due();
    }
}

fn open_stream(device: &cpal::Device, shared: Arc<Shared>) -> Result<cpal::Stream> {
    let supported = device.default_output_config()?;
    let config = supported.config();
    use cpal::SampleFormat as F;
    match supported.sample_format() {
        F::F32 => build::<f32>(device, config, shared),
        F::F64 => build::<f64>(device, config, shared),
        F::I16 => build::<i16>(device, config, shared),
        F::I32 => build::<i32>(device, config, shared),
        F::U16 => build::<u16>(device, config, shared),
        other => bail!("unsupported output format {other:?}"),
    }
}

fn build<T: SizedSample + FromSample<f32>>(device: &cpal::Device, config: cpal::StreamConfig, shared: Arc<Shared>) -> Result<cpal::Stream> {
    let channels = config.channels as usize;
    // Soundtrack frames per device frame.
    let step = RATE / config.sample_rate as f64;
    let mut mix: Option<Arc<Vec<Stereo>>> = None;
    let mut seen = u64::MAX;
    let (mut frames, mut energy, mut source, mut peak) = (0u64, 0f64, 0f64, 0f32);
    let cb = shared.clone();
    let data = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        let s = &cb;
        s.heartbeat.fetch_add(1, Ordering::Relaxed);
        let g = s.mix_gen.load(Ordering::Acquire);
        if g != seen
            && let Ok(m) = s.mix.try_lock()
        {
            mix = m.clone();
            seen = g;
        }
        if s.meter_reset.swap(false, Ordering::Acquire) {
            (frames, energy, source, peak) = (0, 0.0, 0.0, 0.0);
        }
        let playing = s.playing.load(Ordering::Relaxed);
        let volume = f32::from_bits(s.volume.load(Ordering::Relaxed));
        let start = s.pos.load(Ordering::Relaxed);
        let mut pos = f64::from_bits(start);
        for frame in out.chunks_mut(channels.max(1)) {
            let v = match (&mix, playing) {
                (Some(m), true) => sample(m, pos),
                _ => [0.0, 0.0],
            };
            let [l, r] = [soft_clip(v[0] * volume), soft_clip(v[1] * volume)];
            if playing {
                pos += step;
                frames += 1;
                energy += ((l * l + r * r) * 0.5) as f64;
                source += ((v[0] * v[0] + v[1] * v[1]) * 0.5) as f64;
                peak = peak.max(l.abs()).max(r.abs());
            }
            match frame {
                [m] => *m = T::from_sample((l + r) * 0.5),
                [a, b, rest @ ..] => {
                    *a = T::from_sample(l);
                    *b = T::from_sample(r);
                    for x in rest {
                        *x = T::EQUILIBRIUM;
                    }
                }
                [] => {}
            }
        }
        if playing {
            // A seek from the UI since `start` wins over our advance.
            let _ = s.pos.compare_exchange(start, pos.to_bits(), Ordering::Relaxed, Ordering::Relaxed);
            s.meter_frames.store(frames, Ordering::Relaxed);
            s.meter_energy.store(energy.to_bits(), Ordering::Relaxed);
            s.meter_source.store(source.to_bits(), Ordering::Relaxed);
            s.meter_peak.store(peak.to_bits(), Ordering::Relaxed);
        }
    };
    let err = move |e: cpal::Error| {
        eprintln!("glide: audio output: {e}");
        shared.broken.store(true, Ordering::Relaxed);
    };
    Ok(device.build_output_stream::<T, _, _>(config, data, err, None)?)
}

/// Remove preview WAVs left by Glide versions that played audio from files.
pub fn sweep_stale() {
    let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("glide-preview-") else { continue };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<i32>().ok()) else { continue };
        // SAFETY: signal 0 only checks whether the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

impl Drop for AudioPreview {
    fn drop(&mut self) {
        // Background mixes still running are discarded.
        self.shared.requested.fetch_add(1, Ordering::SeqCst);
        self.pause();
    }
}

impl Default for AudioPreview {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn rebuilt(a: &mut AudioPreview, p: &Project) {
        a.rebuild(p, &Recording::default(), Path::new("/nonexistent"));
    }

    #[test]
    fn a_reverted_edit_clears_the_queued_remix() {
        let p0 = Project::default();
        let mut a = AudioPreview::new();
        a.mixed = Some(MixKey::of(&p0));
        let mut p1 = p0.clone();
        p1.style.audio.mute_mic = true;
        rebuilt(&mut a, &p1);
        assert!(a.mixing());
        rebuilt(&mut a, &p0);
        assert!(!a.mixing());
        assert!(a.mixed == Some(MixKey::of(&p0)));
    }

    #[test]
    fn a_changed_key_queues_and_a_newer_key_replaces_it() {
        let p0 = Project::default();
        let mut a = AudioPreview::new();
        a.mixed = Some(MixKey::of(&p0));
        let mut p1 = p0.clone();
        p1.style.audio.mute_mic = true;
        rebuilt(&mut a, &p1);
        assert!(a.request.as_ref().is_some_and(|r| r.1.style.audio.mute_mic));
        let mut p2 = p1.clone();
        p2.style.audio.mute_system = true;
        rebuilt(&mut a, &p2);
        assert!(a.request.as_ref().is_some_and(|r| r.1.style.audio.mute_system));
    }

    #[test]
    fn a_volume_only_change_queues_nothing() {
        let p0 = Project::default();
        let mut a = AudioPreview::new();
        a.mixed = Some(MixKey::of(&p0));
        let mut p1 = p0.clone();
        p1.style.audio.volume = 0.3;
        rebuilt(&mut a, &p1);
        assert!(!a.mixing());
    }
}
