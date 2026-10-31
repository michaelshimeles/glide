//! The editor: preview, inspector, timeline and export for one project.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use egui::{Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};

use super::player::AudioPreview;
use super::preview::Preview;
use super::theme::{self, pal};
use super::widgets::{self, fmt_time};
use super::{inspector, timeline};
use crate::export::{self, Job, Source, Stats};
use crate::project::model::{Codec, ExportFormat, ExportSettings, FormatMemory, Project, Quality, Slice, Zoom};
use crate::project::recording::Recording;
use crate::project::Package;
use crate::render::Gpu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    None,
    Slice(u64),
    Zoom(u64),
    Layout(u64),
    Mask(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Background,
    Cursor,
    Camera,
    Captions,
    Audio,
    Shortcuts,
    Animation,
}

impl Tab {
    pub const ALL: [(Tab, &'static str); 7] = [
        (Tab::Background, "Background"),
        (Tab::Cursor, "Cursor"),
        (Tab::Camera, "Camera"),
        (Tab::Captions, "Captions"),
        (Tab::Audio, "Audio"),
        (Tab::Shortcuts, "Keys"),
        (Tab::Animation, "Motion"),
    ];
}

/// Leaving the editor any way (back to the library, opening another project,
/// quitting) cancels a running export, finishes a voice-over in progress and saves.
impl Drop for Editor {
    fn drop(&mut self) {
        self.cancel_export();
        if self.voice.is_some() {
            self.stop_voiceover();
        }
        if self.dirty_at.is_some() {
            self.save();
        }
        self.mask_watch.cancel();
    }
}

/// Inspector actions that must run after its draft is committed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Deferred {
    StartVoice,
    StopVoice,
}

pub struct ExportRun {
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<Stats>>>,
    pub output: PathBuf,
    started: Instant,
}

impl ExportRun {
    pub fn progress(&self) -> f64 {
        f64::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
pub struct ExportState {
    pub open: bool,
    pub running: Option<ExportRun>,
    pub result: Option<Result<(PathBuf, Stats), String>>,
}

/// Peak level of each recorded track (None = not recorded), measured in
/// the background when the project opens.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrackPeaks {
    pub system: Option<f32>,
    pub mic: Option<f32>,
}

impl TrackPeaks {
    pub fn silent(level: Option<f32>) -> bool {
        level.is_none_or(|p| p < crate::export::audio::SILENT_PEAK)
    }
}

pub struct Editor {
    pub package: Package,
    pub project: Project,
    pub rec: Recording,
    pub preview: Preview,
    pub audio: AudioPreview,
    /// Playback time in ms.
    pub playhead: f64,
    playing: Option<(Instant, f64)>,
    pub selection: Selection,
    pub tab: Tab,
    undo: Vec<Project>,
    redo: Vec<Project>,
    last_commit: Option<Instant>,
    dirty_at: Option<Instant>,
    /// Export settings per format; they are not part of the undo history.
    export_memory: FormatMemory,
    pub export: ExportState,
    /// Timeline pixels per second.
    pub px_per_sec: f32,
    pub status: Option<String>,
    pub thumbs: Option<Vec<egui::TextureHandle>>,
    pub close_requested: bool,
    /// Background transcription (word count or error).
    pub transcribing: Option<std::thread::JoinHandle<Result<usize, String>>>,
    /// Engine for "Transcribe microphone" and its download/progress state.
    pub caption_engine: crate::transcribe::Engine,
    pub transcribe_progress: std::sync::Arc<crate::whisper::Progress>,
    preset_name: String,
    pub looping: bool,
    /// Voice-over being recorded.
    pub voice: Option<super::voiceover::VoiceRecorder>,
    pub deferred: Option<Deferred>,
    /// Opened straight after recording: offer a one-click export.
    pub fresh: bool,
    copy_when_exported: bool,
    /// Where content under blur/pixelate masks moves ("data may be visible").
    pub mask_watch: super::maskwatch::MaskWatch,
    pub peaks: Arc<Mutex<Option<TrackPeaks>>>,
    /// The "no sound" note has been shown.
    silence_noted: bool,
}

/// Why a recording has no sound.
pub fn silence_note(rec: &Recording, p: TrackPeaks) -> &'static str {
    match (rec.meta.mic.is_some(), rec.meta.system_audio.is_some()) {
        (false, false) => "This recording has no audio: system audio and the microphone were off.",
        (false, true) => "This recording is silent: the microphone was off and the Mac played nothing ScreenCaptureKit could capture.",
        (true, _) if p.mic.is_some() => "This recording is silent: the microphone only picked up silence (lid closed or muted input?).",
        _ => "This recording's audio couldn't be read.",
    }
}

impl Editor {
    pub fn open(path: &Path, gpu: Gpu) -> Result<Self> {
        let package = Package::new(path);
        let (project, rec) = package.load()?;
        if !package.project_file().exists() {
            package.save_project(&project)?;
        }
        let preview = Preview::new(gpu, path.to_path_buf(), &project, &rec);
        let mask_watch = super::maskwatch::MaskWatch::new(package.rec(&rec.meta.screen.file));
        let mut audio = AudioPreview::new();
        audio.rebuild_now(&project, &rec, path);
        let peaks = Arc::new(Mutex::new(None));
        {
            let (peaks, meta, rd) = (peaks.clone(), rec.meta.clone(), package.recording_dir());
            std::thread::spawn(move || {
                let p = TrackPeaks { system: crate::export::audio::track_peak(&rd, &meta.system_audio), mic: crate::export::audio::track_peak(&rd, &meta.mic) };
                *peaks.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
            });
        }
        let export_memory = FormatMemory::new(project.export);
        Ok(Editor {
            package,
            project,
            rec,
            preview,
            audio,
            playhead: 0.0,
            playing: None,
            selection: Selection::None,
            tab: Tab::Background,
            undo: vec![],
            redo: vec![],
            last_commit: None,
            dirty_at: None,
            export_memory,
            export: ExportState::default(),
            px_per_sec: 80.0,
            status: None,
            thumbs: None,
            close_requested: false,
            transcribing: None,
            caption_engine: crate::transcribe::Engine::Whisper(crate::whisper::best_downloaded().unwrap_or(crate::whisper::Model::Base)),
            transcribe_progress: Default::default(),
            preset_name: String::new(),
            looping: false,
            voice: None,
            deferred: None,
            fresh: false,
            copy_when_exported: false,
            mask_watch,
            peaks,
            silence_noted: false,
        })
    }

    pub fn duration(&self) -> f64 {
        self.preview.scene.duration()
    }

    /// The playhead right now (it moves between frames while playing).
    pub fn playhead_now(&self) -> f64 {
        self.playing.map_or(self.playhead, |(t0, from)| (from + t0.elapsed().as_secs_f64() * 1000.0).min(self.duration()))
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Apply an edit made on a draft copy of the project.
    pub fn commit(&mut self, draft: Project) {
        if draft == self.project {
            return;
        }
        // Coalesce rapid edits (slider drags) into one undo step.
        let coalesce = self.last_commit.is_some_and(|t| t.elapsed() < Duration::from_millis(600));
        if !coalesce {
            self.undo.push(self.project.clone());
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_commit = Some(Instant::now());
        self.apply(draft);
    }

    fn apply(&mut self, mut p: Project) {
        let audio_changed = p.timeline.slices != self.project.timeline.slices
            || p.timeline.voiceovers != self.project.timeline.voiceovers
            || p.style.audio != self.project.style.audio;
        p.updated_ms = crate::recorder::now_ms();
        self.project = p;
        self.preview.set_project(&self.project, &self.rec);
        if audio_changed {
            self.audio.rebuild(&self.project, &self.rec, &self.package.root);
        }
        self.playhead = self.playhead.min(self.duration());
        self.dirty_at = Some(Instant::now());
        // Undo/redo can remove the selected item; don't leave the inspector blank.
        let tl = &self.project.timeline;
        let exists = match self.selection {
            Selection::None => true,
            Selection::Slice(id) => tl.slices.iter().any(|s| s.id == id),
            Selection::Zoom(id) => tl.zooms.iter().any(|z| z.id == id),
            Selection::Layout(id) => tl.layouts.iter().any(|l| l.id == id),
            Selection::Mask(id) => tl.masks.iter().any(|m| m.id == id),
        };
        if !exists {
            self.selection = Selection::None;
        }
    }

    pub fn undo(&mut self) {
        if let Some(mut p) = self.undo.pop() {
            self.redo.push(self.project.clone());
            self.last_commit = None;
            p.export = self.project.export;
            self.apply(p);
        }
    }

    pub fn redo(&mut self) {
        if let Some(mut p) = self.redo.pop() {
            self.undo.push(self.project.clone());
            self.last_commit = None;
            p.export = self.project.export;
            self.apply(p);
        }
    }

    /// Change the export settings. They are saved with the project but are
    /// not an edit: no undo step, and undo and redo leave them as they are.
    pub fn set_export(&mut self, s: ExportSettings) {
        if s != self.project.export {
            self.project.export = s;
            self.dirty_at = Some(Instant::now());
        }
    }
    pub fn edit(&mut self, f: impl FnOnce(&mut Project)) {
        let mut d = self.project.clone();
        f(&mut d);
        self.last_commit = None;
        self.commit(d);
    }

    pub fn save(&mut self) {
        if let Err(e) = self.package.save_project(&self.project) {
            self.status = Some(format!("Couldn't save: {e}"));
        }
        self.dirty_at = None;
    }

    /// Write edits that autosave has not reached yet.
    pub fn flush(&mut self) {
        if self.dirty_at.is_some() {
            self.save();
        }
    }

    pub fn toggle_play(&mut self) {
        // Pausing while narrating ends the voice-over instead of letting the
        // narration drift from the picture.
        if self.voice.is_some() {
            self.stop_voiceover();
            return;
        }
        if self.playing.is_some() {
            self.playing = None;
            self.audio.pause();
        } else {
            if self.playhead >= self.duration() - 1.0 {
                self.playhead = 0.0;
            }
            self.playing = Some((Instant::now(), self.playhead));
            self.audio.play(self.playhead);
        }
    }

    pub fn seek(&mut self, ms: f64) {
        if self.voice.is_some() {
            self.stop_voiceover();
        }
        self.playhead = ms.clamp(0.0, self.duration());
        if self.playing.is_some() {
            self.playing = Some((Instant::now(), self.playhead));
            self.audio.play(self.playhead);
        }
    }

    /// Source time under the playhead.
    pub fn source_at_playhead(&self) -> f64 {
        self.preview.scene.map.to_source(self.playhead)
    }

    /// Split the slice under the playhead.
    pub fn split(&mut self) {
        let src = self.source_at_playhead();
        let map = self.preview.scene.map.clone();
        self.edit(|p| {
            if p.timeline.slices.is_empty() {
                let id = p.timeline.new_id();
                p.timeline.slices.push(Slice { id, start: 0.0, end: map.source_end(), ..Default::default() });
            }
            crate::engine::typing::split_at(&mut p.timeline, src);
        });
    }

    /// Split zoom `id` of `p` at the playhead. Says why when it can't
    /// (the playhead has to be at least the minimum zoom length inside it).
    pub fn split_zoom_in(&mut self, p: &mut Project, id: u64) -> bool {
        let src = self.source_at_playhead();
        let split = p.timeline.split_zoom(id, src).is_some();
        if !split {
            self.status = Some(format!("Put the playhead at least {:.1} s inside the zoom to split it", Zoom::MIN_LEN_MS / 1000.0));
        }
        split
    }

    /// Split the selected zoom at the playhead (⇧S).
    pub fn split_zoom(&mut self) {
        if let Selection::Zoom(id) = self.selection {
            let mut draft = self.project.clone();
            if self.split_zoom_in(&mut draft, id) {
                self.last_commit = None;
                self.commit(draft);
            }
        }
    }

    /// Cut a range where content under a mask moved out of the edit.
    pub fn cut_exposure(&mut self, e: crate::exposure::Exposure) {
        let duration = self.preview.scene.map.source_end();
        let pad = crate::exposure::CUT_PAD_MS;
        let mut cut = false;
        self.edit(|p| cut = crate::engine::typing::cut(&mut p.timeline, duration, e.start - pad, e.end + pad));
        self.status = Some(if cut {
            format!("Cut {:.1} s where masked content was visible", (e.end - e.start + 2.0 * pad) / 1000.0)
        } else {
            "Couldn't cut that part (it's the whole clip)".into()
        });
        if cut {
            self.selection = Selection::Mask(e.mask);
        }
    }

    /// Transcribe the microphone track in the background with the chosen engine.
    pub fn start_transcription(&mut self) {
        if self.transcribing.is_some() {
            return;
        }
        let (pkg, engine) = (self.package.clone(), self.caption_engine);
        let progress = std::sync::Arc::new(crate::whisper::Progress::default());
        self.transcribe_progress = progress.clone();
        self.transcribing = Some(std::thread::spawn(move || {
            objc2::rc::autoreleasepool(|_| crate::transcribe::transcribe_package(&pkg, engine, &progress)).map_err(|e| format!("{e:#}"))
        }));
    }

    pub fn delete_selection(&mut self) {
        let sel = self.selection;
        self.edit(|p| match sel {
            Selection::Slice(id) => {
                if p.timeline.slices.len() > 1 {
                    p.timeline.slices.retain(|s| s.id != id);
                }
            }
            Selection::Zoom(id) => p.timeline.zooms.retain(|z| z.id != id),
            Selection::Layout(id) => p.timeline.layouts.retain(|l| l.id != id),
            Selection::Mask(id) => p.timeline.masks.retain(|m| m.id != id),
            Selection::None => {}
        });
        self.selection = Selection::None;
    }

    /// Add a zoom at the playhead.
    pub fn add_zoom(&mut self) {
        let src = self.source_at_playhead();
        let duration = self.rec.meta.duration_ms;
        let mut new_id = 0;
        self.edit(|p| new_id = p.timeline.add_zoom(src, p.style.default_zoom, duration));
        self.selection = Selection::Zoom(new_id);
    }

    /// Record narration from the playhead while the preview plays silently.
    pub fn start_voiceover(&mut self) {
        if self.voice.is_some() {
            return;
        }
        if self.playing.is_some() {
            self.toggle_play();
        }
        if self.playhead >= self.duration() - 100.0 {
            self.playhead = 0.0;
        }
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let file = format!("voiceover-{ms}.m4a");
        match super::voiceover::VoiceRecorder::start(&self.package.root, &file, self.source_at_playhead()) {
            Ok(v) => {
                self.voice = Some(v);
                self.looping = false;
                self.playing = Some((Instant::now(), self.playhead));
                self.status = Some("Recording voice-over… press Stop when you're done".into());
            }
            Err(e) => self.status = Some(format!("{e:#}")),
        }
    }

    pub fn stop_voiceover(&mut self) {
        let Some(v) = self.voice.take() else { return };
        let (file, start) = (v.file.clone(), v.start_source);
        let ms = v.stop();
        self.playing = None;
        self.audio.pause();
        if ms < 300.0 {
            let _ = std::fs::remove_file(self.package.rec(&file));
            self.status = Some("Voice-over too short; discarded".into());
            return;
        }
        self.edit(|p| {
            let id = p.timeline.new_id();
            p.timeline.voiceovers.push(crate::project::model::VoiceOver { id, file, start, duration: ms, volume: 1.0 });
        });
        self.status = Some(format!("Added a {:.1} s voice-over", ms / 1000.0));
    }

    /// Add a blur mask at the playhead.
    pub fn add_mask(&mut self) {
        let src = self.source_at_playhead();
        let duration = self.rec.meta.duration_ms;
        let mut new_id = 0;
        self.edit(|p| new_id = p.timeline.add_mask(src, duration));
        self.selection = Selection::Mask(new_id);
    }

    /// Preview point (egui) → normalised recording coordinates, using the
    /// last rendered frame's window placement.
    fn preview_to_recording(&self, preview: egui::Rect, pos: egui::Pos2) -> Option<crate::engine::geom::Vec2> {
        let f = self.preview.last_frame.as_ref()?;
        let out = crate::engine::geom::v2(
            ((pos.x - preview.min.x) / preview.width()) as f64 * f.size.x,
            ((pos.y - preview.min.y) / preview.height()) as f64 * f.size.y,
        );
        let p = self.preview.scene.output_to_screen(out, f.window.rect);
        let rec = self.preview.scene.sizes.recording;
        Some(crate::engine::geom::v2((p.x / rec.x).clamp(0.0, 1.0), (p.y / rec.y).clamp(0.0, 1.0)))
    }

    /// Normalised recording rect → preview rect (egui points).
    fn recording_to_preview(&self, preview: egui::Rect, r: crate::engine::geom::Rect) -> Option<egui::Rect> {
        let f = self.preview.last_frame.as_ref()?;
        let rec = self.preview.scene.sizes.recording;
        let map = |x: f64, y: f64| {
            let o = self.preview.scene.screen_to_output(crate::engine::geom::v2(x * rec.x, y * rec.y), f.window.rect);
            egui::pos2(preview.min.x + (o.x / f.size.x) as f32 * preview.width(), preview.min.y + (o.y / f.size.y) as f32 * preview.height())
        };
        Some(egui::Rect::from_two_pos(map(r.x, r.y), map(r.x + r.w, r.y + r.h)))
    }

    /// Draw or move the selected mask by dragging on the preview.
    fn mask_on_preview(&mut self, ui: &mut Ui, preview: egui::Rect, resp: &egui::Response, id: u64) {
        use crate::engine::geom::{Rect as R, v2};
        let Some(mask) = self.project.timeline.masks.iter().find(|m| m.id == id).copied() else { return };
        if let Some(r) = self.recording_to_preview(preview, mask.rect) {
            let r = r.intersect(preview);
            ui.painter().rect_stroke(r, CornerRadius::same(3), Stroke::new(1.5, theme::accent()), egui::StrokeKind::Outside);
            for c in [r.left_top(), r.right_top(), r.left_bottom(), r.right_bottom()] {
                ui.painter().circle(c, 3.5, Color32::WHITE, Stroke::new(1.0, theme::accent()));
            }
            if resp.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.contains(p)) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
            } else if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
        let drag_id = ui.id().with("mask-drag");
        #[derive(Clone, Copy)]
        enum MaskDrag {
            Draw(crate::engine::geom::Vec2),
            Move(crate::engine::geom::Vec2, R),
        }
        let Some(pos) = resp.interact_pointer_pos() else { return };
        let Some(at) = self.preview_to_recording(preview, pos) else { return };
        if resp.drag_started() {
            let inside = mask.rect.contains(at);
            let d = if inside { MaskDrag::Move(at, mask.rect) } else { MaskDrag::Draw(at) };
            ui.data_mut(|m| m.insert_temp(drag_id, d));
        }
        if resp.dragged()
            && let Some(d) = ui.data(|m| m.get_temp::<MaskDrag>(drag_id))
        {
            let rect = match d {
                MaskDrag::Draw(a) => R::from_points(a, at),
                MaskDrag::Move(grab, orig) => {
                    let x = (orig.x + at.x - grab.x).clamp(0.0, 1.0 - orig.w);
                    let y = (orig.y + at.y - grab.y).clamp(0.0, 1.0 - orig.h);
                    R::new(x, y, orig.w, orig.h)
                }
            };
            if rect.w > 0.005 && rect.h > 0.005 {
                let mut draft = self.project.clone();
                if let Some(m) = draft.timeline.masks.iter_mut().find(|m| m.id == id) {
                    m.rect = rect;
                }
                self.commit(draft);
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|m| m.remove::<MaskDrag>(drag_id));
        }
        let _ = v2;
    }

    /// Whether an export started from this editor is still running.
    pub fn export_running(&self) -> bool {
        self.export.running.is_some()
    }
    /// Stop a running export and wait for its thread, which removes its temp
    /// file. A cancelled run leaves nothing at the output path and shows no
    /// result. A run that had already finished keeps its file and its result.
    pub fn cancel_export(&mut self) {
        if let Some(mut run) = self.export.running.take() {
            run.cancel.store(true, Ordering::SeqCst);
            self.export.result = run.handle.take().and_then(|handle| kept_after_cancel(handle.join(), &run.output));
            self.copy_when_exported = false;
        }
    }
    pub fn start_export(&mut self) {
        if self.export.running.is_some() {
            return;
        }
        let settings = self.project.export;
        let output = export::default_output(&self.package.root, &settings);
        let job = Job {
            project: self.project.clone(),
            recording: self.rec.clone(),
            source: Source::Package(self.package.root.clone()),
            settings,
            output: output.clone(),
        };
        if crate::export::audio::music_missing(Some(&self.package.root), &self.project.style.audio) {
            self.status = Some("Music file missing: exporting without background music".into());
        }
        let progress = Arc::new(AtomicU64::new(0f64.to_bits()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p2, c2) = (progress.clone(), cancel.clone());
        let handle = std::thread::spawn(move || {
            objc2::rc::autoreleasepool(|_| export::run(&job, &mut |f| p2.store(f.to_bits(), Ordering::Relaxed), &c2))
        });
        self.export.result = None;
        self.export.running = Some(ExportRun { progress, cancel, handle: Some(handle), output, started: Instant::now() });
    }

    /// Per-frame housekeeping. Returns true while something animates.
    pub fn tick(&mut self) -> bool {
        let busy_before = self.background_busy();
        if let Some((t0, from)) = self.playing {
            self.playhead = from + t0.elapsed().as_secs_f64() * 1000.0;
            if self.playhead >= self.duration() && self.voice.is_some() {
                self.playhead = self.duration();
                self.stop_voiceover();
            } else if self.playhead >= self.duration() {
                if self.looping {
                    self.playhead = 0.0;
                    self.playing = Some((Instant::now(), 0.0));
                    self.audio.play(0.0);
                } else {
                    self.playhead = self.duration();
                    self.playing = None;
                    self.audio.pause();
                }
            }
        }
        if self.playing.is_some() {
            self.audio.sync(self.playhead);
        }
        self.audio.tick();
        if let Some(e) = self.audio.error.take() {
            self.status = Some(e);
        }
        if !self.silence_noted
            && let Some(p) = *self.peaks.lock().unwrap_or_else(|e| e.into_inner())
        {
            self.silence_noted = true;
            let added = !self.project.timeline.voiceovers.is_empty() || self.project.style.audio.music.is_some();
            if TrackPeaks::silent(p.system) && TrackPeaks::silent(p.mic) && !added {
                self.status = Some(silence_note(&self.rec, p).into());
            }
        }
        let exposures_changed = self.mask_watch.update(&self.project.timeline.masks);
        if self.dirty_at.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) {
            self.save();
        }
        if let Some(run) = &mut self.export.running
            && run.handle.as_ref().is_some_and(|h| h.is_finished())
        {
            let r = run.handle.take().unwrap().join();
            self.export.result = Some(run_result(r, &run.output));
            self.export.running = None;
            if self.copy_when_exported {
                self.copy_when_exported = false;
                if let Some(Ok((path, _))) = &self.export.result {
                    super::macos::copy_file(path);
                    self.status = Some(format!("Exported and copied {} — paste it anywhere", path.file_name().unwrap_or_default().to_string_lossy()));
                }
            }
        }
        if self.transcribing.as_ref().is_some_and(|h| h.is_finished()) {
            match self.transcribing.take().unwrap().join() {
                Ok(Ok(n)) => {
                    if let Ok(r) = self.package.load_recording() {
                        self.rec = r;
                        self.preview.set_project(&self.project, &self.rec);
                    }
                    self.status = Some(if n == 0 { "No speech found in the microphone track".into() } else { format!("Transcribed {n} words") });
                }
                Ok(Err(e)) => self.status = Some(format!("Transcription failed: {e}")),
                Err(_) => self.status = Some("Transcription crashed".into()),
            }
        }
        // Export and transcription only move a progress bar: the app
        // redraws for them at a slow rate (`background_busy`), not per frame,
        // and once more when one finishes (to show its result).
        let finished = busy_before && !self.background_busy();
        self.playing.is_some() || self.voice.is_some() || exposures_changed || finished
    }

    /// Whether an export or transcription is running (progress to show).
    pub fn background_busy(&self) -> bool {
        self.export.running.is_some() || self.transcribing.is_some()
    }

    /// Stop playback, ending a voice-over (no-op when paused).
    pub fn pause(&mut self) {
        if self.voice.is_some() {
            self.stop_voiceover();
        }
        if self.playing.is_some() {
            self.toggle_play();
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k| ctx.input(|i| i.key_pressed(k));
        if pressed(egui::Key::Space) {
            self.toggle_play();
        }
        if cmd && pressed(egui::Key::Z) {
            if shift { self.redo() } else { self.undo() }
        } else if pressed(egui::Key::S) && !cmd {
            if shift { self.split_zoom() } else { self.split() }
        } else if pressed(egui::Key::Z) && !cmd {
            self.add_zoom();
        }
        if !cmd && pressed(egui::Key::M) {
            self.add_mask();
        }
        if !cmd && pressed(egui::Key::L) {
            self.looping = !self.looping;
        }
        if cmd && pressed(egui::Key::E) {
            self.export.open = true;
        }
        if cmd && pressed(egui::Key::S) {
            self.save();
        }
        if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            self.delete_selection();
        }
        let step = 1000.0 / 60.0;
        if pressed(egui::Key::ArrowLeft) {
            self.seek(self.playhead - if shift { 1000.0 } else { step });
        }
        if pressed(egui::Key::ArrowRight) {
            self.seek(self.playhead + if shift { 1000.0 } else { step });
        }
        if pressed(egui::Key::Escape) {
            if self.export.open && self.export.running.is_none() {
                self.export.open = false;
            } else {
                self.selection = Selection::None;
            }
        }
    }

    fn presets_menu(&mut self, ui: &mut Ui) {
        use crate::project::presets;
        ui.menu_button("Presets", |ui| {
            ui.set_min_width(220.0);
            let list = presets::list();
            let default = presets::default_name();
            if list.is_empty() {
                ui.label(RichText::new("No presets yet").color(pal(ui).secondary));
            }
            for p in &list {
                let label = if default.as_deref() == Some(p.name.as_str()) { format!("{}  (default)", p.name) } else { p.name.clone() };
                if ui.button(label).on_hover_text("Apply this style").clicked() {
                    match presets::load(p) {
                        Ok(style) => {
                            self.edit(|d| presets::apply(&mut d.style, &style));
                            self.status = Some(format!("Applied preset “{}”", p.name));
                        }
                        Err(e) => self.status = Some(format!("Couldn't load preset: {e:#}")),
                    }
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("Save current style as").color(pal(ui).secondary));
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut self.preset_name).hint_text("Preset name").desired_width(140.0));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(!self.preset_name.trim().is_empty(), egui::Button::new("Save")).clicked() || enter)
                    && !self.preset_name.trim().is_empty()
                {
                    match presets::save(&self.preset_name, &self.project.style, Some(&self.package.root)) {
                        Ok(p) => self.status = Some(format!("Saved preset “{}”", p.name)),
                        Err(e) => self.status = Some(format!("Couldn't save preset: {e:#}")),
                    }
                    self.preset_name.clear();
                }
            });
            if !list.is_empty() {
                ui.separator();
                ui.menu_button("Default for new recordings", |ui| {
                    if ui.selectable_label(default.is_none(), "None").clicked() {
                        let _ = presets::set_default(None);
                        ui.close();
                    }
                    for p in &list {
                        if ui.selectable_label(default.as_deref() == Some(p.name.as_str()), &p.name).clicked() {
                            let _ = presets::set_default(Some(&p.name));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Delete preset", |ui| {
                    for p in &list {
                        if ui.button(&p.name).clicked() {
                            if let Err(e) = presets::delete(p) {
                                self.status = Some(format!("Couldn't delete preset: {e:#}"));
                            }
                            ui.close();
                        }
                    }
                });
            }
        });
    }

    /// Draw the editor. `back` is set when the user leaves for the library.
    /// The editor window. `chrome` draws the app-level title-bar controls
    /// (Back, the title with recent recordings, Record) and collects their
    /// action.
    pub fn ui(&mut self, ui: &mut Ui, egui_renderer: &mut egui_wgpu::Renderer, chrome: &mut super::home::Chrome) {
        let ctx = ui.ctx().clone();
        self.handle_keys(&ctx);
        let p = pal(ui);

        // Toolbar.
        egui::Panel::top("editor-toolbar")
            .exact_size(theme::TOOLBAR_H)
            .frame(egui::Frame::new().fill(p.window).inner_margin(egui::Margin { left: theme::toolbar_left(), right: 12, top: 0, bottom: 0 }))
            .show(ui, |ui| {
                super::toolbar_drag(ui);
                ui.horizontal_centered(|ui| {
                    chrome.title_left(ui, &self.project.name, Some(&self.package.root));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary(ui, "Export").clicked() {
                            self.export.open = true;
                        }
                        ui.add_space(6.0);
                        self.presets_menu(ui);
                        ui.add_space(4.0);
                        if widgets::icon_button(ui, widgets::Icon::Redo, !self.redo.is_empty()).on_hover_text("Redo ⇧⌘Z").clicked() {
                            self.redo();
                        }
                        if widgets::icon_button(ui, widgets::Icon::Undo, !self.undo.is_empty()).on_hover_text("Undo ⌘Z").clicked() {
                            self.undo();
                        }
                        ui.add_space(10.0);
                        chrome.record_controls(ui);
                    });
                });
            });

        // Inspector.
        egui::Panel::right("inspector")
            .exact_size(320.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.inspector).inner_margin(egui::Margin::symmetric(14, 10)))
            .show(ui, |ui| {
                inspector::ui(ui, self);
            });

        // Timeline.
        egui::Panel::bottom("timeline")
            .exact_size(if self.rec.meta.camera.is_some() { 284.0 } else { 236.0 } + timeline::extra_height(self))
            .frame(egui::Frame::new().fill(p.sidebar).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                self.transport(ui);
                ui.add_space(4.0);
                timeline::ui(ui, self);
            });

        // Preview.
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(if ui.visuals().dark_mode { Color32::from_rgb(0x1c, 0x1c, 0x1e) } else { Color32::from_rgb(0xe4, 0xe4, 0xe7) }))
            .show(ui, |ui| {
                let avail = ui.available_rect_before_wrap().shrink(24.0);
                let aspect = self.preview.output_aspect();
                let mut size = vec2(avail.width(), avail.width() / aspect);
                if size.y > avail.height() {
                    size = vec2(avail.height() * aspect, avail.height());
                }
                let rect = egui::Rect::from_center_size(avail.center(), size);
                let ppp = ctx.pixels_per_point();
                let mut px = ((size.x * ppp) as u32, (size.y * ppp) as u32);
                // Light mode: at most 1920 px wide, drawn scaled up (a full
                // Retina-size preview is several times the GPU work).
                if crate::hw::light() && px.0 > 1920 {
                    px = (1920, (px.1 as f32 * 1920.0 / px.0 as f32) as u32);
                }
                if let Some(id) = self.preview.render(egui_renderer, self.playhead, px) {
                    let shadow = egui::epaint::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(60) };
                    ui.painter().add(shadow.as_shape(rect, CornerRadius::same(4)));
                    ui.painter().image(id, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
                // Click on the preview while a manual zoom is selected picks its
                // target; with a mask selected, dragging draws or moves it.
                let resp = ui.interact(rect, ui.id().with("preview"), Sense::click_and_drag());
                if let Selection::Mask(id) = self.selection {
                    self.mask_on_preview(ui, rect, &resp, id);
                } else if resp.clicked() {
                    if let (Selection::Zoom(id), Some(pos)) = (self.selection, resp.interact_pointer_pos()) {
                        self.pick_zoom_target(id, (pos - rect.min) / rect.size());
                    } else {
                        self.toggle_play();
                    }
                }
                if let Some(s) = &self.status {
                    ui.painter().text(rect.left_bottom() + vec2(0.0, 14.0), egui::Align2::LEFT_TOP, s, egui::FontId::proportional(11.0), p.secondary);
                }
                // Just recorded: a quick way out without touching the editor.
                if self.fresh && self.export.running.is_none() {
                    let bar = egui::Rect::from_center_size(egui::pos2(rect.center().x, rect.top() - 4.0), vec2(420.0, 40.0));
                    let bar = bar.translate(vec2(0.0, -bar.height() / 2.0 + 18.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                        egui::Frame::new()
                            .fill(p.popover)
                            .stroke(Stroke::new(0.5, p.control_border))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(format!("Recorded {}", fmt_time(self.duration())));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if widgets::icon_button(ui, widgets::Icon::Close, true).on_hover_text("Dismiss").clicked() {
                                            self.fresh = false;
                                        }
                                        if widgets::primary(ui, "Export & copy").on_hover_text("Export with the current settings and put the file on the clipboard").clicked() {
                                            self.fresh = false;
                                            self.copy_when_exported = true;
                                            self.export.open = true;
                                            self.start_export();
                                        }
                                    });
                                });
                            });
                    });
                }
            });

        if self.export.open {
            self.export_sheet(&ctx);
        }
    }

    /// The zoom target (normalised in the cropped screen) under a point of
    /// the preview, given as a fraction of its width and height. It goes back
    /// through the window as drawn right now, zoom and inset included, so it
    /// names the pixel the user sees under the pointer.
    pub fn zoom_target_at(&self, at: egui::Vec2) -> Option<crate::engine::geom::Vec2> {
        let f = self.preview.last_frame.as_ref()?;
        let scene = &self.preview.scene;
        let p = scene.output_to_screen(crate::engine::geom::v2(at.x as f64 * f.size.x, at.y as f64 * f.size.y), f.window.rect);
        let t = scene.sizes.crop.scalar_of(p);
        Some(crate::engine::geom::v2(t.x.clamp(0.0, 1.0), t.y.clamp(0.0, 1.0)))
    }

    fn pick_zoom_target(&mut self, id: u64, at: egui::Vec2) {
        let Some(target) = self.zoom_target_at(at) else { return };
        self.edit(|p| {
            if let Some(z) = p.timeline.zooms.iter_mut().find(|z| z.id == id) {
                z.mode = crate::project::model::ZoomMode::Manual { target };
            }
        });
    }

    fn transport(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let label = if self.is_playing() { "⏸" } else { "▶" };
            if ui.add(egui::Button::new(RichText::new(label).size(14.0)).min_size(vec2(32.0, 24.0))).on_hover_text("Play/Pause (Space)").clicked() {
                self.toggle_play();
            }
            ui.label(RichText::new(format!("{} / {}", fmt_time(self.playhead), fmt_time(self.duration()))).monospace());
            if ui.selectable_label(self.looping, "Loop").on_hover_text("Loop playback (L)").clicked() {
                self.looping = !self.looping;
            }
            ui.separator();
            if ui.button("✂ Split").on_hover_text("Split at playhead (S)").clicked() {
                self.split();
            }
            if ui.button("+ Zoom").on_hover_text("Add zoom at playhead (Z)").clicked() {
                self.add_zoom();
            }
            if ui.button("+ Mask").on_hover_text("Blur, pixelate or highlight a region (M)").clicked() {
                self.add_mask();
            }
            if self.rec.meta.camera.is_some() && ui.button("+ Layout").clicked() {
                let src = self.source_at_playhead();
                let duration = self.rec.meta.duration_ms;
                self.edit(|p| {
                    p.timeline.add_layout(src, duration);
                });
            }
            if ui.add_enabled(self.selection != Selection::None, egui::Button::new("Delete")).clicked() {
                self.delete_selection();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.px_per_sec, 20.0..=400.0).logarithmic(true).show_value(false));
                widgets::caption(ui, "Zoom");
            });
        });
    }

    fn export_sheet(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Export")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(380.0, 0.0))
            .open(&mut open)
            .show(ctx, |ui| {
                let mut s = self.project.export;
                let running = self.export.running.is_some();
                ui.add_enabled_ui(!running, |ui| {
                    widgets::row(ui, "Format", |ui| {
                        let before = s.format;
                        widgets::segmented(ui, &mut s.format, &[(ExportFormat::Gif, "GIF"), (ExportFormat::Mp4, "MP4")]);
                        if s.format != before {
                            s = self.export_memory.switch(self.project.export, s.format);
                        }
                    });
                    let (heights, fpss): (&[u32], &[u32]) = match s.format {
                        ExportFormat::Mp4 => (&ExportSettings::MP4_HEIGHTS, &ExportSettings::MP4_FPS),
                        ExportFormat::Gif => (&ExportSettings::GIF_HEIGHTS, &ExportSettings::GIF_FPS),
                    };
                    widgets::row(ui, "Resolution", |ui| {
                        egui::ComboBox::from_id_salt("res").selected_text(res_label(s.height)).show_ui(ui, |ui| {
                            for h in heights {
                                ui.selectable_value(&mut s.height, *h, res_label(*h));
                            }
                        });
                    });
                    widgets::row(ui, "Frame rate", |ui| {
                        egui::ComboBox::from_id_salt("fps").selected_text(format!("{} fps", s.fps)).show_ui(ui, |ui| {
                            for f in fpss {
                                ui.selectable_value(&mut s.fps, *f, format!("{f} fps"));
                            }
                        });
                    });
                    widgets::row(ui, "Quality", |ui| {
                        egui::ComboBox::from_id_salt("q").selected_text(s.quality.label()).show_ui(ui, |ui| {
                            let qs: &[Quality] = if s.format == ExportFormat::Gif { &[Quality::Studio, Quality::Social] } else { &Quality::ALL };
                            for q in qs {
                                ui.selectable_value(&mut s.quality, *q, q.label());
                            }
                        });
                    });
                    if s.format == ExportFormat::Mp4 {
                        widgets::row(ui, "Codec", |ui| {
                            widgets::segmented(ui, &mut s.codec, &[(Codec::H264, "H.264"), (Codec::Hevc, "HEVC"), (Codec::Prores, "ProRes")]);
                        });
                        let fits = s.fits_size();
                        let mut fit = s.target_mb.is_some();
                        if fits && widgets::toggle_row(ui, "Fit to file size", &mut fit) {
                            s.target_mb = fit.then_some(10);
                        }
                        if let Some(mb) = s.target_mb.as_mut().filter(|_| fits) {
                            widgets::row(ui, "Target size", |ui| {
                                ui.add(egui::DragValue::new(mb).range(1..=4000).suffix(" MB"));
                            });
                        }
                    } else {
                        widgets::toggle_row(ui, "Loop", &mut s.gif_loop);
                    }
                    let (rw, rh) = self.preview.scene.sizes.render_size(s.height);
                    let (w, h) = crate::export::output_size(&self.preview.scene.sizes, &s);
                    let secs = self.duration() / 1000.0;
                    let mb = s.estimate_mb(w, h, secs);
                    widgets::caption(ui, &format!("{w}×{h} • {:.1} s • about {:.0} MB", secs, mb.max(0.1)));
                    if s.format == ExportFormat::Mp4 && s.codec_for(rw, rh) != s.codec {
                        widgets::caption(ui, "HEVC at this size (H.264 stops at 4096×2304)");
                    }
                    if (w, h) != (rw, rh) {
                        widgets::caption(ui, &format!("This Mac's video encoder can't take {rw}×{rh}, so it exports at {w}×{h}"));
                    }
                });
                self.set_export(s);
                ui.add_space(8.0);
                if let Some(run) = &self.export.running {
                    let f = run.progress();
                    ui.add(egui::ProgressBar::new(f as f32).show_percentage());
                    let el = run.started.elapsed().as_secs_f64();
                    if f > 0.02 {
                        widgets::caption(ui, &format!("{:.0} s left", el / f - el));
                    }
                    if ui.button("Cancel").clicked() {
                        run.cancel.store(true, Ordering::SeqCst);
                    }
                } else {
                    match &self.export.result {
                        Some(Ok((path, stats))) => {
                            ui.label(RichText::new("✓ Exported").color(Color32::from_rgb(52, 199, 89)));
                            widgets::caption(ui, &format!(
                                "{} • {}×{} • {:.1} MB • {:.1}× real time",
                                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                stats.width,
                                stats.height,
                                stats.bytes as f64 / 1e6,
                                stats.speed()
                            ));
                            let path = path.clone();
                            ui.horizontal(|ui| {
                                if ui.button("Show in Finder").clicked() {
                                    super::macos::reveal(&path);
                                }
                                if ui.button("Copy").on_hover_text("Copy the file to the clipboard").clicked() {
                                    super::macos::copy_file(&path);
                                }
                                if ui.button("Open").clicked() {
                                    super::macos::open(&path);
                                }
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(Color32::from_rgb(255, 69, 58), e);
                        }
                        None => {}
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary(ui, "Export").clicked() {
                            self.start_export();
                        }
                    });
                }
                let _ = Stroke::NONE;
            });
        if !open && self.export.running.is_none() {
            self.export.open = false;
        }
    }
}

fn res_label(h: u32) -> String {
    match h {
        ExportSettings::NATIVE => "Native".into(),
        2160 => "4K".into(),
        1440 => "1440p (2K)".into(),
        _ => format!("{h}p"),
    }
}

/// What a joined export thread produced, as the export sheet shows it.
fn run_result(r: std::thread::Result<Result<Stats>>, output: &Path) -> Result<(PathBuf, Stats), String> {
    match r {
        Ok(Ok(stats)) => Ok((output.to_path_buf(), stats)),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("export crashed".into()),
    }
}
/// The result to keep when an export is cancelled: only a run that had already finished.
fn kept_after_cancel(r: std::thread::Result<Result<Stats>>, output: &Path) -> Option<Result<(PathBuf, Stats), String>> {
    run_result(r, output).ok().map(Ok)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn stats() -> Stats {
        Stats { frames: 1, width: 2, height: 2, seconds: 0.0, video_seconds: 1.0, bytes: 1 }
    }
    #[test]
    fn a_run_that_finished_before_the_cancel_keeps_its_result() {
        let kept = kept_after_cancel(Ok(Ok(stats())), Path::new("/m/Demo.mp4"));
        assert!(matches!(kept, Some(Ok((p, _))) if p == Path::new("/m/Demo.mp4")));
    }
    #[test]
    fn a_cancelled_or_crashed_run_keeps_nothing() {
        let out = Path::new("/m/Demo.mp4");
        assert!(kept_after_cancel(Ok(Err(anyhow::anyhow!("cancelled"))), out).is_none());
        assert!(kept_after_cancel(Err(Box::new("boom")), out).is_none());
    }
}
