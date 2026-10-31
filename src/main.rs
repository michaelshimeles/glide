use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use glide::engine::scene::Scene;
use glide::export::{self, Job, Source};
use glide::project::model::{Codec, ExportFormat, ExportSettings, Quality};
use glide::project::{self, Package};
use glide::render::{Gpu, Renderer, VideoFrame};

#[derive(Parser)]
#[command(name = "glide", version, about = "Fast native screen recorder and editor")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Render one frame of a project to PNG.
    Frame {
        /// A .glide package (omit with --synthetic).
        project: Option<PathBuf>,
        /// Use the built-in synthetic recording.
        #[arg(long)]
        synthetic: bool,
        /// Playback time in seconds.
        #[arg(short, long, default_value_t = 2.0)]
        time: f64,
        #[arg(long, default_value_t = 1080)]
        height: u32,
        /// Apply a style preset JSON (partial project.json) before rendering.
        #[arg(long)]
        style: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Check that every Background setting (and padding, roundness and
    /// shadow) visibly changes a rendered frame of a project.
    CheckBackground {
        /// A .glide package (omit with --synthetic).
        project: Option<PathBuf>,
        #[arg(long)]
        synthetic: bool,
        /// Playback time in seconds.
        #[arg(short, long, default_value_t = 1.0)]
        time: f64,
        #[arg(long, default_value_t = 400)]
        height: u32,
    },
    /// Measure a recording's or a video's audio, and how much of it is a
    /// test tone (system and mic tracks of a .glide package, or every audio
    /// track of a media file).
    AudioCheck {
        path: PathBuf,
        /// The tone to look for, Hz.
        #[arg(long, default_value_t = 880.0)]
        freq: f64,
        /// Fail unless this track (system, mic, or audio0, audio1… for
        /// files) holds the tone for at least --min-seconds.
        #[arg(long)]
        require: Vec<String>,
        #[arg(long, default_value_t = 1.0)]
        min_seconds: f64,
    },
    /// Export a project to MP4 or GIF.
    Export {
        project: Option<PathBuf>,
        #[arg(long)]
        synthetic: bool,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long, value_parser = ["mp4", "gif"])]
        format: Option<String>,
        /// Output height in pixels, 0 to 8640; 0 = native (the recording's own pixels).
        #[arg(long, value_parser = clap::value_parser!(u32).range(0..=ExportSettings::MAX_HEIGHT as i64))]
        height: Option<u32>,
        /// Frames per second, 1 to 120.
        #[arg(long, value_parser = clap::value_parser!(u32).range(ExportSettings::MIN_FPS as i64..=ExportSettings::MAX_FPS as i64))]
        fps: Option<u32>,
        #[arg(long, value_parser = ["studio", "social", "web", "web-low"])]
        quality: Option<String>,
        #[arg(long, value_parser = ["h264", "hevc", "prores"])]
        codec: Option<String>,
        /// MP4: aim for a file of about this many MB (1 or more).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        target_mb: Option<u32>,
    },
    /// Record the screen into a new .glide project.
    Record {
        /// Display id (see `glide devices`); default: main display.
        #[arg(long)]
        display: Option<u32>,
        /// Window id (see `glide devices`).
        #[arg(long)]
        window: Option<u32>,
        /// Area as x,y,w,h in points on the display.
        #[arg(long)]
        area: Option<String>,
        /// Record a USB-connected iPhone/iPad (id from `glide devices`).
        #[arg(long)]
        device: Option<String>,
        /// Record a generated test pattern instead of the screen.
        #[arg(long)]
        pattern: bool,
        #[arg(long)]
        mic: bool,
        #[arg(long)]
        no_system_audio: bool,
        #[arg(long)]
        camera: bool,
        /// Stop after this many seconds (else Ctrl-C).
        #[arg(short = 't', long)]
        duration: Option<f64>,
        #[arg(long, default_value_t = 60)]
        fps: u32,
        /// Folder for the project (default ~/Movies/Glide).
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        /// Leave Finder's desktop icons out of the recording.
        #[arg(long)]
        hide_icons: bool,
        /// Leave the Dock out of the recording.
        #[arg(long)]
        hide_dock: bool,
    },
    /// Open the app (the default with no subcommand).
    Gui {
        /// A .glide project to open.
        project: Option<PathBuf>,
    },
    /// Render the UI headlessly to PNG (layout checks without a display).
    #[command(hide = true)]
    Snapshot {
        project: Option<PathBuf>,
        #[arg(long)]
        tab: Option<String>,
        /// Select the first zoom, mask or slice.
        #[arg(long, value_parser = ["zoom", "mask", "slice"])]
        select: Option<String>,
        #[arg(long)]
        export_sheet: bool,
        /// Show the just-recorded banner.
        #[arg(long)]
        fresh: bool,
        #[arg(long)]
        light: bool,
        #[arg(long, default_value_t = 0.0)]
        at: f64,
        /// Home: show the Window source with the first window chosen.
        #[arg(long)]
        window: bool,
        /// Also draw the recent-recordings popover.
        #[arg(long)]
        recents: bool,
        /// Start view: show a live preview (display or synthetic).
        #[arg(long, value_parser = ["display", "synthetic"])]
        live: Option<String>,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Write the app icon as a macOS .iconset folder (or one PNG with --png).
    #[command(hide = true)]
    Icon {
        output: PathBuf,
        #[arg(long)]
        png: Option<u32>,
    },
    /// Save the current desktop picture as PNG (what "Desktop wallpaper" uses).
    #[command(hide = true)]
    Wallpaper { output: PathBuf },
    /// Run an audio file through voice isolation (writes 48 kHz WAV).
    #[command(hide = true)]
    Denoise { input: PathBuf, output: PathBuf },
    /// Measure editor preview playback speed (frames per second).
    #[command(hide = true)]
    BenchPreview {
        project: PathBuf,
        #[arg(long, default_value_t = 2400)]
        width: u32,
        #[arg(long, default_value_t = 1500)]
        height: u32,
    },
    /// Transcribe a project's microphone track into captions.
    Transcribe {
        project: PathBuf,
        /// apple, whisper-base, whisper-small or whisper-medium.
        #[arg(long, default_value = "whisper-base")]
        engine: String,
    },
    /// Run the live source preview for a while; save the last frame as PNG.
    #[command(hide = true)]
    Preview {
        #[arg(long)]
        window: Option<String>,
        /// x,y,w,h in points on the main display.
        #[arg(long)]
        area: Option<String>,
        /// A camera ("default", or a unique id / name from `glide devices`).
        #[arg(long)]
        camera: Option<String>,
        #[arg(long, default_value_t = 2.0)]
        secs: f64,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Resize a window (id, name, or pid:<process id>; see `glide devices`) to WxH points.
    ResizeWindow { window: String, size: String },
    /// List where content under a blur/pixelate mask moves ("data may be visible").
    CheckMasks { project: PathBuf },
    /// List displays, windows, cameras and microphones.
    Devices,
    /// Show (or request) Screen Recording, Microphone and Camera access.
    Permissions {
        #[arg(long)]
        request: bool,
    },
    /// Write a synthetic recording package (no permissions needed).
    Synth {
        output: PathBuf,
        #[arg(long, default_value_t = 12.0)]
        duration: f64,
        #[arg(long, default_value_t = 2560)]
        width: u32,
        #[arg(long, default_value_t = 1600)]
        height: u32,
        /// Make it an iPhone/iPad recording (portrait sizes get a phone frame).
        #[arg(long)]
        device: bool,
        /// Add a system-audio track: a sine tone at this many Hz.
        #[arg(long)]
        tone: Option<f64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // AppKit must be initialised for cursor and device queries.
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        let _ = objc2_app_kit::NSApplication::sharedApplication(mtm);
    }
    match cli.cmd {
        Some(Cmd::Frame { project, synthetic, time, height, style, output }) => {
            frame(project, synthetic, time, height, style, output)
        }
        Some(Cmd::CheckBackground { project, synthetic, time, height }) => check_background(project, synthetic, time, height),
        Some(Cmd::AudioCheck { path, freq, require, min_seconds }) => audio_check(path, freq, require, min_seconds),
        Some(Cmd::Export { project, synthetic, output, format, height, fps, quality, codec, target_mb }) => {
            export_cmd(project, synthetic, output, format, height, fps, quality, codec, target_mb)
        }
        Some(Cmd::Synth { output, duration, width, height, device, tone }) => synth(output, duration, width, height, device, tone),
        Some(Cmd::Record { display, window, area, device, pattern, mic, no_system_audio, camera, duration, fps, dir, name, hide_icons, hide_dock }) => {
            record(display, window, area, device, pattern, mic, !no_system_audio, camera, duration, fps, dir, name, (hide_icons, hide_dock))
        }
        Some(Cmd::Devices) => devices(),
        Some(Cmd::Permissions { request }) => permissions(request),
        Some(Cmd::Gui { project }) => glide::app::run(project),
        Some(Cmd::BenchPreview { project, width, height }) => {
            let fps = glide::app::snapshot::bench_preview(&project, (width, height), 240)?;
            println!("preview {width}x{height}: {fps:.0} frames/s");
            Ok(())
        }
        Some(Cmd::Denoise { input, output }) => {
            let audio = glide::media::read_audio(&input, 0)?;
            let t = Instant::now();
            let out = glide::export::isolate::isolate_voice(&audio).ok_or_else(|| anyhow::anyhow!("voice isolation unavailable"))?;
            eprintln!("{:.1} s of audio in {:.2} s", audio.len() as f64 / 48_000.0, t.elapsed().as_secs_f64());
            glide::app::player::write_wav(&output, &out)?;
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Wallpaper { output }) => {
            glide::app::macos::save_desktop_wallpaper(&output)?;
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Icon { output, png }) => {
            match png {
                Some(size) => image::save_buffer(&output, &glide::render::icon::render(size), size, size, image::ExtendedColorType::Rgba8)?,
                None => glide::render::icon::write_iconset(&output)?,
            }
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Snapshot { project, tab, select, export_sheet, fresh, light, at, window, recents, live, output }) => {
            use glide::app::editor::Tab;
            let tab = tab.map(|t| {
                Tab::ALL.iter().find(|(_, n)| n.eq_ignore_ascii_case(&t)).map(|(t, _)| *t).expect("unknown tab")
            });
            let shot = glide::app::snapshot::Shot {
                project: project.as_deref(),
                tab,
                select,
                export_sheet,
                fresh,
                dark: !light,
                playhead_ms: at * 1000.0,
                size: (1320.0, 840.0),
                window,
                recents,
                live,
            };
            glide::app::snapshot::render(&shot, &output)?;
            println!("{}", output.display());
            Ok(())
        }
        Some(Cmd::Preview { window, area, camera, secs, output }) => {
            use glide::capture::preview::{LivePreview, Source};
            let source = if let Some(c) = camera {
                Source::Camera { id: (c != "default").then_some(c) }
            } else if let Some(w) = window {
                Source::Window { id: glide::capture::screen::find_window(&w)?.id }
            } else if let Some(a) = area {
                let v: Vec<f64> = a.split(',').map(|x| x.trim().parse().unwrap_or(0.0)).collect();
                anyhow::ensure!(v.len() == 4, "--area expects x,y,w,h");
                Source::Area { display: 0, rect: [v[0], v[1], v[2], v[3]] }
            } else {
                Source::Display { id: 0 }
            };
            let frames = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter = frames.clone();
            let t0 = Instant::now();
            let p = LivePreview::start(&source, 1280, 10, false, move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })?;
            let started = t0.elapsed().as_secs_f64();
            let mut last = None;
            let until = Instant::now() + std::time::Duration::from_secs_f64(secs);
            while Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if let Some(f) = p.take_frame() {
                    last = Some(f);
                }
            }
            drop(p);
            let n = frames.load(std::sync::atomic::Ordering::Relaxed);
            let f = last.ok_or_else(|| anyhow::anyhow!("no preview frames"))?;
            image::save_buffer(&output, &f.rgba, f.width, f.height, image::ExtendedColorType::Rgba8)?;
            println!("{source:?}: started in {started:.2} s, {n} frames in {secs:.1} s, {}x{} → {}", f.width, f.height, output.display());
            Ok(())
        }
        Some(Cmd::ResizeWindow { window, size }) => {
            let (w, h) = size.split_once(['x', '×']).ok_or_else(|| anyhow::anyhow!("size must look like 1920x1080"))?;
            let (w, h): (f64, f64) = (w.trim().parse()?, h.trim().parse()?);
            // A window id needs no ScreenCaptureKit lookup (which can stall).
            let pid = window.strip_prefix("pid:").and_then(|p| p.parse::<i32>().ok());
            let (id, label) = match (pid, window.parse::<u32>()) {
                (Some(pid), _) => {
                    let id = glide::capture::winsize::window_of_pid(pid).ok_or_else(|| anyhow::anyhow!("process {pid} has no window on screen"))?;
                    (id, format!("window {id}"))
                }
                (None, Ok(id)) => (id, format!("window {id}")),
                (None, Err(_)) => {
                    let w = glide::capture::screen::find_window(&window)?;
                    (w.id, format!("{} — {}", w.app, w.title))
                }
            };
            let before = glide::capture::winsize::window_size(id)?;
            let got = glide::capture::winsize::resize(id, w, h)?;
            println!("{label}: {:.0}×{:.0} → {:.0}×{:.0}", before.0, before.1, got.0, got.1);
            Ok(())
        }
        Some(Cmd::CheckMasks { project }) => {
            let pkg = Package::new(&project);
            let (p, rec) = pkg.load()?;
            let t0 = Instant::now();
            let found = glide::exposure::scan(&pkg.rec(&rec.meta.screen.file), &p.timeline.masks, &mut Default::default(), &|| false)?;
            let hiding = p.timeline.masks.iter().filter(|m| glide::exposure::hides(m.kind)).count();
            println!("{hiding} hiding masks checked in {:.2} s", t0.elapsed().as_secs_f64());
            for e in &found {
                println!("mask {}: content moved {:.1}–{:.1} s", e.mask, e.start / 1000.0, e.end / 1000.0);
            }
            if found.is_empty() {
                println!("no content moves under masks");
            }
            Ok(())
        }
        Some(Cmd::Transcribe { project, engine }) => {
            let engine = glide::transcribe::Engine::parse(&engine).ok_or_else(|| anyhow::anyhow!("unknown engine {engine:?}"))?;
            let t0 = Instant::now();
            let n = glide::transcribe::transcribe_package(&Package::new(&project), engine, &Default::default())?;
            eprintln!("{} in {:.1} s", engine.label(), t0.elapsed().as_secs_f64());
            println!("{n} words → {}", glide::transcribe::transcript_path(&Package::new(&project)).display());
            Ok(())
        }
        None => glide::app::run(None),
    }
}

type Loaded = (glide::project::model::Project, glide::project::recording::Recording, Option<PathBuf>);

/// A project to render frames of: a package, or the synthetic recording.
fn load_for_frames(pkg: Option<PathBuf>, synthetic: bool) -> Result<Loaded> {
    Ok(if synthetic {
        let rec = glide::synth::recording(1920, 1080, 12_000.0);
        (project::new_project("synthetic", &rec), rec, None)
    } else {
        let pkg = Package::new(pkg.context("a project path or --synthetic")?);
        let (p, r) = pkg.load()?;
        (p, r, Some(pkg.root.clone()))
    })
}

/// Render the frame at `time` seconds, `height` pixels tall.
fn render_frame(r: &mut Renderer, (proj, rec, dir): &Loaded, time: f64, height: u32) -> Result<(glide::render::Image, f64)> {
    let mut scene = Scene::new(proj.clone(), rec, 60.0);
    let f = scene.frame(time * 1000.0, height as f64);
    r.prepare(&proj.style, rec, dir.as_deref());
    match dir {
        None => {
            let (w, h) = (rec.meta.screen.width, rec.meta.screen.height);
            let px = glide::synth::screen_bgra(w, h, f.source_ms);
            r.set_screen(&VideoFrame { width: w, height: h, stride: (w * 4) as usize, bgra: &px });
        }
        Some(dir) => {
            let pkg = Package::new(dir);
            let mut src = glide::media::FrameSource::open(&pkg.rec(&rec.meta.screen.file))?;
            if let Some(vf) = src.frame_at(f.source_ms) {
                vf.with_bgra(|v| r.set_screen(v));
            }
            if let Some(cam) = &rec.meta.camera
                && let Ok(mut c) = glide::media::FrameSource::open(&pkg.rec(&cam.file))
                && let Some(vf) = c.frame_at(f.source_ms - cam.offset_ms)
            {
                vf.with_bgra(|v| r.set_camera(v));
            }
        }
    }
    Ok((r.render_to_image(&f, &proj.style), f.source_ms))
}

fn frame(pkg: Option<PathBuf>, synthetic: bool, time: f64, height: u32, style: Option<String>, output: PathBuf) -> Result<()> {
    let mut loaded = load_for_frames(pkg, synthetic)?;
    if let Some(s) = style {
        let mut v = serde_json::to_value(&loaded.0)?;
        merge(&mut v, serde_json::from_str(&s)?);
        loaded.0 = serde_json::from_value(v)?;
    }
    let mut r = Renderer::new(Gpu::headless()?);
    let (img, source_ms) = render_frame(&mut r, &loaded, time, height)?;
    image::save_buffer(&output, &img.rgba, img.width, img.height, image::ExtendedColorType::Rgba8)?;
    println!("{} ({}x{}, source {:.0} ms)", output.display(), img.width, img.height, source_ms);
    Ok(())
}

fn audio_check(path: PathBuf, freq: f64, require: Vec<String>, min_seconds: f64) -> Result<()> {
    let mut tracks: Vec<(String, PathBuf, usize)> = vec![];
    if path.join("recording/meta.json").exists() {
        let pkg = Package::new(&path);
        let (_, rec) = pkg.load()?;
        for (label, t) in [("system", &rec.meta.system_audio), ("mic", &rec.meta.mic)] {
            match t {
                Some(t) => tracks.push((label.into(), pkg.rec(&t.file), t.track)),
                None => println!("{label}: not recorded"),
            }
        }
    } else {
        for i in 0..8 {
            if glide::media::read_audio(&path, i).is_err() {
                if i == 0 {
                    println!("audio0: no audio track");
                }
                break;
            }
            tracks.push((format!("audio{i}"), path.clone(), i));
        }
    }
    let mut found = std::collections::HashMap::new();
    for (label, file, track) in &tracks {
        let samples = glide::media::read_audio(file, *track)?;
        let l = glide::audiocheck::analyze(&samples, freq);
        println!(
            "{label}: {:.1} s, {:.1} dBFS RMS; {freq:.0} Hz tone for {:.2} s at {:.1} dBFS",
            l.seconds, l.rms_db, l.tone_seconds, l.tone_db
        );
        found.insert(label.clone(), l.tone_seconds);
    }
    let missing: Vec<_> = require.iter().filter(|r| found.get(*r).is_none_or(|s| *s < min_seconds)).collect();
    if !missing.is_empty() {
        bail!("no {freq:.0} Hz tone (at least {min_seconds} s) in: {}", missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

fn check_background(pkg: Option<PathBuf>, synthetic: bool, time: f64, height: u32) -> Result<()> {
    use glide::render::bgcheck;
    let mut loaded = load_for_frames(pkg, synthetic)?;
    // Zoomed-in frames push the background out of view.
    loaded.0.timeline.zooms.clear();
    let style = loaded.0.style.clone();
    let tmp = std::env::temp_dir().join(format!("glide-check-background-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let image = tmp.join("checker.png");
    bgcheck::checkerboard(&image)?;
    let mut r = Renderer::new(Gpu::headless()?);
    let outcomes = bgcheck::run(&style, &image.to_string_lossy(), |s| {
        loaded.0.style = s.clone();
        Ok(render_frame(&mut r, &loaded, time, height)?.0)
    });
    let _ = std::fs::remove_dir_all(&tmp);
    let outcomes = outcomes?;
    let mut bad = 0;
    for o in &outcomes {
        let mark = if o.ok() { "ok  " } else { "FAIL" };
        bad += usize::from(!o.ok());
        println!("{mark} {:<22} {:>6.2}% changed (needs {:.2}%)", o.name, o.changed * 100.0, o.min * 100.0);
    }
    if bad > 0 {
        bail!("{bad} of {} background settings made no visible change", outcomes.len());
    }
    println!("all {} background settings change the frame", outcomes.len());
    Ok(())
}

/// Deep-merge JSON objects (b into a).
fn merge(a: &mut serde_json::Value, b: serde_json::Value) {
    match (a, b) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (a, b) => *a = b,
    }
}

#[allow(clippy::too_many_arguments)]
fn export_cmd(
    pkg: Option<PathBuf>,
    synthetic: bool,
    output: Option<PathBuf>,
    format: Option<String>,
    height: Option<u32>,
    fps: Option<u32>,
    quality: Option<String>,
    codec: Option<String>,
    target_mb: Option<u32>,
) -> Result<()> {
    let (proj, rec, source, base) = if synthetic {
        let rec = glide::synth::recording(1920, 1080, 12_000.0);
        (project::new_project("synthetic", &rec), rec, Source::Synthetic, PathBuf::from("synthetic.glide"))
    } else {
        let path = pkg.expect("project path or --synthetic");
        let pkg = Package::new(&path);
        let (p, r) = pkg.load()?;
        (p, r, Source::Package(path.clone()), path)
    };
    let mut settings = proj.export;
    if target_mb.is_some() {
        settings.target_mb = target_mb;
    }
    if let Some(f) = format {
        settings = if f == "gif" { ExportSettings::gif_default() } else { ExportSettings { format: ExportFormat::Mp4, ..settings } };
    }
    if let Some(h) = height {
        settings.height = h;
    }
    if let Some(f) = fps {
        settings.fps = f;
    }
    if let Some(q) = quality {
        settings.quality = match q.as_str() {
            "studio" => Quality::Studio,
            "web" => Quality::Web,
            "web-low" => Quality::WebLow,
            _ => Quality::Social,
        };
    }
    if let Some(c) = codec {
        settings.codec = match c.as_str() {
            "hevc" => Codec::Hevc,
            "prores" => Codec::Prores,
            _ => Codec::H264,
        };
    }
    let output = output.unwrap_or_else(|| export::default_output(&base, &settings));
    let job = Job { project: proj, recording: rec, source, settings, output: output.clone() };
    let cancel = AtomicBool::new(false);
    let mut last = Instant::now();
    let stats = export::run(&job, &mut |p| {
        if last.elapsed().as_millis() > 500 {
            eprint!("\r{:5.1}%", p * 100.0);
            last = Instant::now();
        }
    }, &cancel)?;
    eprintln!("\r100.0%");
    println!(
        "{}: {}x{} {} frames, {:.1} s video in {:.2} s ({:.1}x real time), {:.1} MB",
        output.display(),
        stats.width,
        stats.height,
        stats.frames,
        stats.video_seconds,
        stats.seconds,
        stats.speed(),
        stats.bytes as f64 / 1e6
    );
    Ok(())
}

fn synth(output: PathBuf, duration: f64, width: u32, height: u32, device: bool, tone: Option<f64>) -> Result<()> {
    let write = if device { glide::synth::write_device_package } else { glide::synth::write_package };
    let (frames, zooms) = write(&output, duration, width, height)?;
    if let Some(hz) = tone {
        glide::synth::add_tone(&output, hz)?;
    }
    println!("{} ({frames} frames, {zooms} zooms)", output.display());
    Ok(())
}

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
fn record(
    display: Option<u32>,
    window: Option<u32>,
    area: Option<String>,
    device: Option<String>,
    pattern: bool,
    mic: bool,
    system_audio: bool,
    camera: bool,
    duration: Option<f64>,
    fps: u32,
    dir: Option<PathBuf>,
    name: Option<String>,
    (hide_desktop_icons, hide_dock): (bool, bool),
) -> Result<()> {
    use glide::recorder::{Options, Recorder, Target};
    let target = if pattern {
        Target::Pattern
    } else if let Some(id) = device {
        glide::capture::camera::enable_screen_capture_devices();
        let wanted = id.to_lowercase();
        let (found, ..) = glide::capture::camera::poll_until(
            glide::capture::camera::IOS_WAIT,
            glide::capture::camera::IOS_POLL,
            || glide::capture::camera::ios_devices().into_iter().find(|d| d.id == id || d.name.to_lowercase().contains(&wanted)),
            std::thread::sleep,
        );
        let d = found
            .ok_or_else(|| anyhow::anyhow!("no iPhone/iPad matches {id:?} (see `glide devices`)"))?;
        Target::Device { id: d.id.clone() }
    } else if let Some(id) = window {
        Target::Window { id }
    } else if let Some(a) = area {
        let v: Vec<f64> = a.split(',').map(|x| x.trim().parse().unwrap_or(0.0)).collect();
        anyhow::ensure!(v.len() == 4, "--area expects x,y,w,h");
        Target::Area { display: display.unwrap_or(0), rect: glide::engine::geom::Rect::new(v[0], v[1], v[2], v[3]) }
    } else {
        Target::Display { id: display.unwrap_or(0) }
    };
    let opts = Options {
        target,
        system_audio: system_audio && !pattern,
        mic: mic.then_some(None),
        camera: camera.then_some(None),
        fps,
        dir: dir.unwrap_or_else(project::projects_dir),
        name,
        hide_desktop_icons,
        hide_dock,
        // The command line makes no sound of its own; excluding "this app"
        // would also drop a running Glide window's audio.
        exclude_own_audio: false,
        ..Default::default()
    };
    unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as usize) };
    let rec = Recorder::start(opts)?;
    eprintln!("glide: recording to {} (Ctrl-C to stop)", rec.package.root.display());
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if STOP.load(std::sync::atomic::Ordering::SeqCst) || rec.error().is_some() {
            break;
        }
        if duration.is_some_and(|d| rec.has_frames() && rec.elapsed() >= d) {
            break;
        }
        if duration.is_some_and(|d| !rec.has_frames() && rec.started.elapsed().as_secs_f64() > d + 5.0) {
            break;
        }
    }
    if let Some(e) = rec.error() {
        eprintln!("glide: {e}");
    }
    let path = rec.stop()?;
    let pkg = Package::new(&path);
    let r = pkg.load_recording()?;
    let p = pkg.load_project()?;
    println!(
        "{} ({:.1} s, {}x{}, {} mouse events, {} keys, {} cursors, {} zooms)",
        path.display(),
        r.meta.duration_ms / 1000.0,
        r.meta.screen.width,
        r.meta.screen.height,
        r.mouse.len(),
        r.keys.len(),
        r.cursors.len(),
        p.timeline.zooms.len()
    );
    Ok(())
}

fn devices() -> Result<()> {
    use glide::capture::{camera, screen};
    println!("Displays:");
    for d in screen::displays()? {
        println!("  {:>10}  {}x{} pt  {}x{} px", d.id, d.points.0, d.points.1, d.pixels.0, d.pixels.1);
    }
    println!("Windows:");
    for w in screen::windows()? {
        println!("  {:>10}  {} — {} ({:.0}x{:.0})", w.id, w.app, w.title, w.size.0, w.size.1);
    }
    println!("Cameras:");
    for c in camera::cameras() {
        println!("  {}  ({})", c.name, c.id);
    }
    println!("Microphones:");
    for m in camera::microphones() {
        println!("  {}  ({})", m.name, m.id);
    }
    // iOS devices appear shortly after screen-capture devices are enabled.
    camera::enable_screen_capture_devices();
    println!("iPhone / iPad:");
    for d in camera::ios_devices_waiting(camera::IOS_WAIT) {
        println!("  {}  ({})", d.name, d.id);
    }
    Ok(())
}

fn permissions(request: bool) -> Result<()> {
    use glide::capture::tcc;
    if request {
        tcc::request_all(true, true, true, std::time::Duration::from_secs(30));
    }
    println!("Screen Recording: {:?}", tcc::screen_status());
    println!("Microphone:       {:?}", tcc::mic_status());
    println!("Camera:           {:?}", tcc::camera_status());
    println!("Accessibility:    {} (resizing windows)", if glide::capture::winsize::trusted(false) { "Authorized" } else { "NotDetermined/Denied" });
    if glide::capture::camera::builtin_mic_off(None) {
        println!("Note: the lid is closed, so the built-in microphone records silence.");
    }
    Ok(())
}
