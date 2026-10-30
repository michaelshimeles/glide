//! Microphone level for the recording-setup meters: a CoreAudio (cpal)
//! input stream on its own thread that only tracks the peak.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// A running level meter; stops when dropped.
pub struct MicMeter {
    level: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
}

impl MicMeter {
    /// Meter the input device named `name` (as AVFoundation names it), or the
    /// default input.
    pub fn start(name: Option<&str>) -> Result<MicMeter> {
        let level = Arc::new(AtomicU32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (lv, st, name) = (level.clone(), stop.clone(), name.map(str::to_owned));
        // The stream stays on the thread that made it.
        std::thread::spawn(move || {
            let stream = match open(name.as_deref(), lv) {
                Ok(s) => s,
                Err(e) => return drop(tx.send(Err(format!("{e:#}")))),
            };
            let _ = tx.send(Ok(()));
            while !st.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            drop(stream);
        });
        rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow!("microphone timed out"))?.map_err(|e| anyhow!(e))?;
        Ok(MicMeter { level, stop })
    }

    /// Peak level (0..1) since the last call.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn open(name: Option<&str>, level: Arc<AtomicU32>) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => host
            .input_devices()?
            .find(|d| d.description().is_ok_and(|desc| desc.name() == n))
            .or_else(|| host.default_input_device()),
        None => host.default_input_device(),
    }
    .context("no microphone")?;
    let supported = device.default_input_config()?;
    let config = supported.config();
    let err = |e| eprintln!("daisycap: mic meter: {e}");
    use cpal::SampleFormat as F;
    let stream = match supported.sample_format() {
        F::F32 => device.build_input_stream(config.clone(), move |d: &[f32], _: &_| peak(&level, d.iter().copied()), err, None)?,
        F::I16 => device.build_input_stream(config.clone(), move |d: &[i16], _: &_| peak(&level, d.iter().map(|&x| x as f32 / 32768.0)), err, None)?,
        F::I32 => device.build_input_stream(config.clone(), move |d: &[i32], _: &_| peak(&level, d.iter().map(|&x| x as f32 / 2_147_483_648.0)), err, None)?,
        other => bail!("unsupported microphone format {other:?}"),
    };
    stream.play()?;
    Ok(stream)
}

fn peak(level: &AtomicU32, samples: impl Iterator<Item = f32>) {
    let p = samples.fold(0f32, |m, x| m.max(x.abs())).min(1.0);
    // Non-negative f32 bits order like the floats.
    level.fetch_max(p.to_bits(), Ordering::Relaxed);
}
