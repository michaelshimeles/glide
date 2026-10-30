//! Captions with Whisper (whisper.cpp, Metal): on-device, no Speech
//! permission needed, word-level timings. Models are downloaded on first
//! use into `~/Library/Application Support/DaisyCap/models`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result, bail};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::project::recording::Word;

/// Whisper model sizes offered for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Model {
    Base,
    Small,
    Medium,
}

impl Model {
    pub const ALL: [Model; 3] = [Model::Base, Model::Small, Model::Medium];

    pub fn name(self) -> &'static str {
        match self {
            Model::Base => "base",
            Model::Small => "small",
            Model::Medium => "medium",
        }
    }

    pub fn parse(s: &str) -> Option<Model> {
        Model::ALL.into_iter().find(|m| m.name() == s)
    }

    /// Approximate download size, MB.
    pub fn size_mb(self) -> u32 {
        match self {
            Model::Base => 148,
            Model::Small => 488,
            Model::Medium => 1533,
        }
    }

    fn file(self) -> String {
        format!("ggml-{}.bin", self.name())
    }

    fn url(self) -> String {
        format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}", self.file())
    }

    pub fn path(self) -> PathBuf {
        models_dir().join(self.file())
    }

    pub fn is_downloaded(self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() > (self.size_mb() as u64) * 900_000)
    }
}

fn models_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/DaisyCap/models")
}

/// The largest model already on disk.
pub fn best_downloaded() -> Option<Model> {
    Model::ALL.into_iter().rev().find(|m| m.is_downloaded())
}

/// Progress shared with the UI, in tenths of a percent (0..=1000), with the
/// current phase.
#[derive(Default)]
pub struct Progress {
    pub permille: AtomicU32,
    /// 0 = downloading, 1 = transcribing.
    pub phase: AtomicU32,
}

/// Download `model` unless it's already there (with /usr/bin/curl, into a
/// partial file that is renamed when complete).
pub fn download(model: Model, progress: &Progress) -> Result<PathBuf> {
    let path = model.path();
    if model.is_downloaded() {
        return Ok(path);
    }
    std::fs::create_dir_all(models_dir())?;
    let part = path.with_extension("bin.part");
    let mut child = std::process::Command::new("/usr/bin/curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--output"])
        .arg(&part)
        .arg(model.url())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("starting the model download")?;
    let expected = model.size_mb() as f64 * 1_000_000.0;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        let got = std::fs::metadata(&part).map_or(0, |m| m.len()) as f64;
        progress.permille.store(((got / expected).min(0.99) * 1000.0) as u32, Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        let _ = std::fs::remove_file(&part);
        bail!("couldn't download the Whisper {} model: {}", model.name(), err.trim());
    }
    std::fs::rename(&part, &path)?;
    if !model.is_downloaded() {
        let _ = std::fs::remove_file(&path);
        bail!("the downloaded Whisper {} model is incomplete", model.name());
    }
    Ok(path)
}

/// Is this whisper token text a bracketed annotation rather than speech
/// (e.g. "[BLANK_AUDIO]", "(music)")?
fn is_annotation(t: &str) -> bool {
    let t = t.trim();
    t.is_empty() || (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')')) || t.starts_with("[_")
}

/// Merge whisper's per-word segments: a segment that doesn't start with a
/// space continues the previous word (e.g. "don" + "'t").
pub fn merge_words(segments: &[(f64, f64, String)]) -> Vec<Word> {
    let mut out: Vec<Word> = vec![];
    for (start, end, text) in segments {
        if is_annotation(text) {
            continue;
        }
        match out.last_mut() {
            Some(w) if !text.starts_with(' ') => {
                w.text.push_str(text.trim());
                w.end = *end;
            }
            _ => out.push(Word { start: *start, end: *end, text: text.trim().to_string() }),
        }
    }
    out.retain(|w| !w.text.is_empty());
    out
}

/// Transcribe 16 kHz mono samples; word times in ms from sample 0.
pub fn transcribe(samples: &[f32], model: Model, progress: &Arc<Progress>) -> Result<Vec<Word>> {
    if samples.iter().all(|s| s.abs() < 1e-4) {
        return Ok(vec![]);
    }
    progress.phase.store(0, Ordering::Relaxed);
    let path = download(model, progress)?;
    progress.phase.store(1, Ordering::Relaxed);
    progress.permille.store(0, Ordering::Relaxed);
    whisper_rs::install_logging_hooks();
    let ctx = WhisperContext::new_with_params(&path, WhisperContextParameters::default())
        .map_err(|e| anyhow::anyhow!("loading the Whisper model: {e}"))?;
    let mut state = ctx.create_state().map_err(|e| anyhow::anyhow!("Whisper: {e}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
    params.set_n_threads(threads);
    params.set_language(Some("auto"));
    params.set_translate(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    // One word per segment with token-level timing.
    params.set_token_timestamps(true);
    params.set_split_on_word(true);
    params.set_max_len(1);
    let p = progress.clone();
    params.set_progress_callback_safe(move |pct: i32| p.permille.store(pct.clamp(0, 100) as u32 * 10, Ordering::Relaxed));
    state.full(params, samples).map_err(|e| anyhow::anyhow!("Whisper transcription failed: {e}"))?;
    let mut segments = vec![];
    for seg in state.as_iter() {
        let text = seg.to_str_lossy().map(|t| t.into_owned()).unwrap_or_default();
        // Timestamps are in centiseconds.
        segments.push((seg.start_timestamp() as f64 * 10.0, seg.end_timestamp() as f64 * 10.0, text));
    }
    progress.permille.store(1000, Ordering::Relaxed);
    Ok(merge_words(&segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_word_pieces_and_drops_annotations() {
        let segs = vec![
            (0.0, 300.0, " Hello".to_string()),
            (300.0, 600.0, " world".to_string()),
            (600.0, 650.0, ",".to_string()),
            (700.0, 900.0, " [BLANK_AUDIO]".to_string()),
            (1000.0, 1200.0, " don".to_string()),
            (1200.0, 1300.0, "'t".to_string()),
        ];
        let w = merge_words(&segs);
        let texts: Vec<_> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Hello", "world,", "don't"]);
        assert_eq!((w[1].start, w[1].end), (300.0, 650.0));
        assert_eq!((w[2].start, w[2].end), (1000.0, 1300.0));
    }

    /// Speech made with macOS `say` comes back as the same words. Skipped
    /// when the base model hasn't been downloaded (`daisycap transcribe`).
    #[test]
    fn transcribes_synthesized_speech() {
        if !Model::Base.is_downloaded() {
            eprintln!("Whisper base model not downloaded; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("daisycap-whisper-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("speech.aiff");
        let ok = std::process::Command::new("say")
            .args(["-o"])
            .arg(&file)
            .arg("Record the screen and zoom into every click.")
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("`say` unavailable; skipping");
            return;
        }
        let stereo = crate::media::read_audio(&file, 0).unwrap();
        let words = transcribe(&crate::transcribe::downsample(&stereo), Model::Base, &Arc::default()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let text = words.iter().map(|w| w.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        for expected in ["record", "screen", "zoom", "click"] {
            assert!(text.contains(expected), "{text}");
        }
        assert!(words.windows(2).all(|w| w[0].start <= w[1].start), "word times go forward");
    }

    #[test]
    fn model_names_round_trip() {
        for m in Model::ALL {
            assert_eq!(Model::parse(m.name()), Some(m));
        }
        assert!(Model::Base.size_mb() < Model::Small.size_mb());
    }
}
