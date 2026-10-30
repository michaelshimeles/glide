//! Captions: transcribe the microphone track with Apple's Speech framework
//! (on-device when the recogniser supports it) or Whisper (see `whisper`),
//! producing word timings in source milliseconds. Results go to
//! `recording/transcript.json`.
//!
//! The audio is downmixed to 16 kHz mono WAV and split into chunks of at most
//! ~55 s at the quietest point near each boundary, because URL recognition
//! requests are limited in length.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::msg_send;
use objc2_foundation::{NSArray, NSBundle, NSError, NSLocale, NSOperationQueue, NSString, NSURL};

use crate::capture::mixer;
use crate::project::recording::Word;
use crate::project::{Package, write_json};
use crate::util::ns;

#[link(name = "Speech", kind = "framework")]
unsafe extern "C" {}

const RATE: f64 = 16_000.0;
const MAX_CHUNK_S: f64 = 55.0;
/// How far back from the hard limit we look for a quiet split point.
const SPLIT_WINDOW_S: f64 = 8.0;

/// Speech-to-text engine for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Apple's Speech framework (needs Speech Recognition permission).
    Apple,
    Whisper(crate::whisper::Model),
}

impl Engine {
    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "apple" => Some(Engine::Apple),
            _ => crate::whisper::Model::parse(s.strip_prefix("whisper-").unwrap_or(s)).map(Engine::Whisper),
        }
    }

    pub fn label(self) -> String {
        match self {
            Engine::Apple => "Apple Speech".into(),
            Engine::Whisper(m) => format!("Whisper {}", m.name()),
        }
    }
}

/// Transcribe a package's microphone track. Returns the number of words.
pub fn transcribe_package(pkg: &Package, engine: Engine, progress: &std::sync::Arc<crate::whisper::Progress>) -> Result<usize> {
    let rec = pkg.load_recording()?;
    let mic = rec.meta.mic.as_ref().ok_or_else(|| anyhow!("this recording has no microphone track"))?;
    let stereo = crate::media::read_audio(&pkg.rec(&mic.file), mic.track)?;
    let mono = downsample(&stereo);
    let words = match engine {
        Engine::Apple => transcribe_samples(&mono)?,
        Engine::Whisper(model) => crate::whisper::transcribe(&mono, model, progress)?,
    };
    write_json(&pkg.rec("transcript.json"), &words)?;
    Ok(words.len())
}

/// 48 kHz stereo → 16 kHz mono (box filter over each group of three).
pub fn downsample(stereo: &[[f32; 2]]) -> Vec<f32> {
    let step = (mixer::RATE / RATE).round() as usize;
    stereo
        .chunks(step)
        .map(|c| c.iter().map(|s| (s[0] + s[1]) * 0.5).sum::<f32>() / c.len() as f32)
        .collect()
}

/// Split points (sample indices) so no chunk exceeds `MAX_CHUNK_S`, cutting at
/// the quietest 100 ms window near each limit.
pub fn split_points(samples: &[f32]) -> Vec<usize> {
    let max = (MAX_CHUNK_S * RATE) as usize;
    let win = (0.1 * RATE) as usize;
    let mut cuts = vec![0];
    let mut start = 0;
    while samples.len() - start > max {
        let hi = start + max;
        let lo = hi - (SPLIT_WINDOW_S * RATE) as usize;
        let mut best = (f32::MAX, hi);
        let mut i = lo;
        while i + win <= hi {
            let e: f32 = samples[i..i + win].iter().map(|s| s * s).sum();
            if e < best.0 {
                best = (e, i + win / 2);
            }
            i += win / 2;
        }
        start = best.1;
        cuts.push(start);
    }
    cuts.push(samples.len());
    cuts
}

/// Recognise 16 kHz mono samples; word times are in ms from sample 0.
pub fn transcribe_samples(samples: &[f32]) -> Result<Vec<Word>> {
    if samples.iter().all(|s| s.abs() < 1e-4) {
        return Ok(vec![]);
    }
    authorize()?;
    let dir = std::env::temp_dir().join(format!("daisycap-transcribe-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let cuts = split_points(samples);
    let mut words = vec![];
    let result = (|| {
        for (i, pair) in cuts.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            let chunk = &samples[a..b];
            if chunk.iter().all(|s| s.abs() < 1e-3) {
                continue;
            }
            let path = dir.join(format!("chunk{i}.wav"));
            write_wav(&path, chunk)?;
            let offset = a as f64 / RATE * 1000.0;
            for mut w in recognize_file(&path)? {
                w.start += offset;
                w.end += offset;
                words.push(w);
            }
        }
        Ok::<_, anyhow::Error>(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result?;
    Ok(words)
}

fn write_wav(path: &Path, samples: &[f32]) -> Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&(RATE as u32).to_le_bytes());
    b.extend_from_slice(&(RATE as u32 * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).with_context(|| format!("writing {}", path.display()))
}

fn class(name: &str) -> Result<&'static AnyClass> {
    AnyClass::get(&std::ffi::CString::new(name).unwrap()).ok_or_else(|| anyhow!("{name} is unavailable"))
}

/// Speech recognition permission. Requesting it from a process without an
/// `NSSpeechRecognitionUsageDescription` would abort, so only the bundled app asks.
fn authorize() -> Result<()> {
    let cls = class("SFSpeechRecognizer")?;
    let status: isize = unsafe { msg_send![cls, authorizationStatus] };
    match status {
        3 => return Ok(()),
        1 => bail!("Speech Recognition access was denied — enable DaisyCap in System Settings › Privacy & Security › Speech Recognition"),
        2 => bail!("Speech Recognition is restricted on this Mac"),
        _ => {}
    }
    let has_usage = NSBundle::mainBundle()
        .infoDictionary()
        .and_then(|d| d.objectForKey(&ns("NSSpeechRecognitionUsageDescription")))
        .is_some();
    if !has_usage {
        bail!("Speech Recognition permission can only be requested from DaisyCap.app");
    }
    let (tx, rx) = mpsc::channel::<isize>();
    let block = RcBlock::new(move |s: isize| {
        let _ = tx.send(s);
    });
    unsafe {
        let _: () = msg_send![cls, requestAuthorization: &*block];
    }
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(3) => Ok(()),
        Ok(_) => bail!("Speech Recognition access was not granted"),
        Err(_) => bail!("timed out waiting for Speech Recognition permission"),
    }
}

fn recognize_file(path: &Path) -> Result<Vec<Word>> {
    let rec_cls = class("SFSpeechRecognizer")?;
    let req_cls = class("SFSpeechURLRecognitionRequest")?;
    unsafe {
        let locale = NSLocale::currentLocale();
        let alloc: Allocated<AnyObject> = msg_send![rec_cls, alloc];
        let recognizer: Option<Retained<AnyObject>> = msg_send![alloc, initWithLocale: &*locale];
        let recognizer = match recognizer {
            Some(r) => r,
            None => {
                let en = NSLocale::localeWithLocaleIdentifier(&ns("en-US"));
                let alloc: Allocated<AnyObject> = msg_send![rec_cls, alloc];
                let r: Option<Retained<AnyObject>> = msg_send![alloc, initWithLocale: &*en];
                r.ok_or_else(|| anyhow!("no speech recogniser for this language"))?
            }
        };
        let available: Bool = msg_send![&*recognizer, isAvailable];
        if !available.as_bool() {
            bail!("speech recognition is not available right now");
        }
        // Results arrive on this queue; the default (main) queue would deadlock
        // a caller blocking on the main thread.
        let queue = NSOperationQueue::new();
        let _: () = msg_send![&*recognizer, setQueue: &*queue];

        let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
        let alloc: Allocated<AnyObject> = msg_send![req_cls, alloc];
        let request: Retained<AnyObject> = msg_send![alloc, initWithURL: &*url];
        let _: () = msg_send![&*request, setShouldReportPartialResults: false];
        let on_device: Bool = msg_send![&*recognizer, supportsOnDeviceRecognition];
        if on_device.as_bool() {
            let _: () = msg_send![&*request, setRequiresOnDeviceRecognition: true];
        }
        let _: () = msg_send![&*request, setAddsPunctuation: true];

        let (tx, rx) = mpsc::channel::<Result<Vec<Word>, String>>();
        let block = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            if let Some(err) = error.as_ref() {
                // 1110 = "No speech detected": an empty chunk, not a failure.
                let code = err.code();
                let _ = tx.send(if code == 1110 || code == 203 { Ok(vec![]) } else { Err(err.localizedDescription().to_string()) });
                return;
            }
            let Some(result) = result.as_ref() else { return };
            let is_final: Bool = msg_send![result, isFinal];
            if !is_final.as_bool() {
                return;
            }
            let best: Retained<AnyObject> = msg_send![result, bestTranscription];
            let segments: Retained<NSArray<AnyObject>> = msg_send![&*best, segments];
            let words = segments
                .iter()
                .map(|seg| {
                    let text: Retained<NSString> = msg_send![&*seg, substring];
                    let t: f64 = msg_send![&*seg, timestamp];
                    let d: f64 = msg_send![&*seg, duration];
                    Word { start: t * 1000.0, end: (t + d) * 1000.0, text: text.to_string() }
                })
                .filter(|w| !w.text.trim().is_empty())
                .collect();
            let _ = tx.send(Ok(words));
        });
        let task: Retained<AnyObject> = msg_send![&*recognizer, recognitionTaskWithRequest: &*request, resultHandler: &*block];
        let out = rx.recv_timeout(Duration::from_secs(300));
        let _: () = msg_send![&*task, cancel];
        match out {
            Ok(Ok(w)) => Ok(w),
            Ok(Err(e)) => bail!("{e}"),
            Err(_) => bail!("speech recognition timed out"),
        }
    }
}

/// Where a package's transcript lives.
pub fn transcript_path(pkg: &Package) -> PathBuf {
    pkg.rec("transcript.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_averages_and_downmixes() {
        let s = vec![[1.0, 0.0]; 6];
        let m = downsample(&s);
        assert_eq!(m.len(), 2);
        assert!((m[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn splits_long_audio_at_quiet_points() {
        let n = (RATE * 130.0) as usize;
        let quiet_at = (RATE * 50.0) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| if (quiet_at..quiet_at + 3200).contains(&i) { 0.0 } else { ((i as f32) * 0.05).sin() * 0.5 })
            .collect();
        let cuts = split_points(&samples);
        assert_eq!(cuts.first(), Some(&0));
        assert_eq!(cuts.last(), Some(&n));
        for w in cuts.windows(2) {
            assert!(w[1] - w[0] <= (MAX_CHUNK_S * RATE) as usize);
        }
        // The first cut lands inside the silent gap.
        assert!((quiet_at..quiet_at + 3200).contains(&cuts[1]), "cut at {}", cuts[1]);
    }

    #[test]
    fn short_audio_is_one_chunk() {
        let samples = vec![0.1f32; (RATE * 10.0) as usize];
        assert_eq!(split_points(&samples), vec![0, samples.len()]);
    }
}
