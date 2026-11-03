//! Synthetic recordings for tests and demos: a generated app-like screen
//! and scripted mouse/keyboard activity. Lets the whole pipeline run
//! without Screen Recording permission.

use crate::engine::geom::Rect;
use crate::project::recording::{
    CursorImage, KeyEvent, MouseEvent, MouseKind, Recording, RecordingMeta, SourceKind, VideoTrack, Word, mods,
};

/// Draw a mock app UI at time `t_ms` into BGRA bytes (stride = 4·w).
pub fn screen_bgra(w: u32, h: u32, t_ms: f64) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 4) as usize];
    let s = h as f64 / 1080.0;
    let fill = |px: &mut Vec<u8>, r: Rect, c: [u8; 3]| {
        let (x0, y0) = ((r.x * s) as i64, (r.y * s) as i64);
        let (x1, y1) = (((r.x + r.w) * s) as i64, ((r.y + r.h) * s) as i64);
        for y in y0.max(0)..y1.min(h as i64) {
            for x in x0.max(0)..x1.min(w as i64) {
                let i = ((y as u32 * w + x as u32) * 4) as usize;
                px[i] = c[2];
                px[i + 1] = c[1];
                px[i + 2] = c[0];
                px[i + 3] = 255;
            }
        }
    };
    let ws = w as f64 / s;
    // Window chrome, sidebar, toolbar.
    fill(&mut px, Rect::new(0.0, 0.0, ws, 1080.0), [246, 246, 248]);
    fill(&mut px, Rect::new(0.0, 0.0, ws, 52.0), [232, 232, 236]);
    for (i, c) in [[255, 95, 87], [254, 188, 46], [40, 200, 64]].iter().enumerate() {
        fill(&mut px, Rect::new(20.0 + i as f64 * 22.0, 20.0, 12.0, 12.0), *c);
    }
    fill(&mut px, Rect::new(0.0, 52.0, 260.0, 1028.0), [236, 236, 240]);
    for i in 0..12 {
        let sel = (i as f64 - (t_ms / 1500.0).floor()).rem_euclid(12.0) == 0.0;
        let c = if sel { [0, 122, 255] } else { [200, 200, 208] };
        fill(&mut px, Rect::new(20.0, 80.0 + i as f64 * 44.0, 200.0, 22.0), c);
    }
    // Content: cards with "text" lines.
    for row in 0..3 {
        for col in 0..3 {
            let x = 300.0 + col as f64 * 520.0;
            let y = 90.0 + row as f64 * 320.0;
            if x + 480.0 > ws {
                continue;
            }
            fill(&mut px, Rect::new(x, y, 480.0, 290.0), [255, 255, 255]);
            fill(&mut px, Rect::new(x + 24.0, y + 24.0, 260.0, 26.0), [40, 40, 48]);
            for l in 0..5 {
                let len = 300.0 + ((row * 3 + col + l) % 4) as f64 * 30.0;
                fill(&mut px, Rect::new(x + 24.0, y + 76.0 + l as f64 * 32.0, len, 14.0), [170, 170, 180]);
            }
            fill(&mut px, Rect::new(x + 24.0, y + 236.0, 120.0, 32.0), [0, 122, 255]);
        }
    }
    // A progress bar that moves, so frames visibly change.
    let p = (t_ms / 8000.0).fract();
    fill(&mut px, Rect::new(300.0, 1040.0, (ws - 340.0) * p, 10.0), [52, 199, 89]);
    px
}

/// A scripted recording: the cursor tours the UI and clicks a few buttons.
pub fn recording(w: u32, h: u32, duration_ms: f64) -> Recording {
    let s = h as f64 / 1080.0;
    let stops = [
        (0.0, 900.0, 600.0, false),
        (1200.0, 420.0, 340.0, true),
        (2600.0, 460.0, 360.0, true),
        (4500.0, 1450.0, 720.0, true),
        (6200.0, 1500.0, 680.0, false),
        (8000.0, 110.0, 300.0, true),
        (9500.0, 900.0, 900.0, false),
        (11_000.0, 1000.0, 500.0, true),
    ];
    let mut mouse = vec![];
    let mut t = 0.0;
    while t <= duration_ms {
        // Piecewise eased path between stops, with a little hand jitter.
        let i = stops.iter().rposition(|st| st.0 <= t).unwrap_or(0);
        let (a, b) = (stops[i], stops[(i + 1).min(stops.len() - 1)]);
        let span = (b.0 - a.0).max(1.0);
        let u = ((t - a.0) / span).clamp(0.0, 1.0);
        let e = u * u * (3.0 - 2.0 * u);
        let jitter = ((t * 0.05).sin() * 1.5, (t * 0.037).cos() * 1.5);
        let (x, y) = ((a.1 + (b.1 - a.1) * e + jitter.0) * s, (a.2 + (b.2 - a.2) * e + jitter.1) * s);
        mouse.push(MouseEvent { t, x, y, kind: MouseKind::Move, button: 0, cursor: 0 });
        t += 1000.0 / 60.0;
    }
    for st in stops.iter().filter(|st| st.3) {
        let (x, y) = (st.1 * s, st.2 * s);
        mouse.push(MouseEvent { t: st.0 + 80.0, x, y, kind: MouseKind::Down, button: 0, cursor: 0 });
        mouse.push(MouseEvent { t: st.0 + 160.0, x, y, kind: MouseKind::Up, button: 0, cursor: 0 });
    }
    mouse.sort_by(|a, b| a.t.total_cmp(&b.t));
    let keys = vec![
        KeyEvent { t: 3200.0, code: 1, key: "S".into(), mods: mods::CMD, down: true },
        KeyEvent { t: 7000.0, code: 8, key: "K".into(), mods: mods::CMD | mods::SHIFT, down: true },
    ];
    let transcript = ["Here's", "a", "quick", "tour", "of", "the", "new", "dashboard."]
        .iter()
        .enumerate()
        .map(|(i, w)| Word { start: 500.0 + i as f64 * 320.0, end: 760.0 + i as f64 * 320.0, text: (*w).into() })
        .collect();
    Recording {
        meta: RecordingMeta {
            version: 1,
            kind: SourceKind::Window,
            duration_ms,
            screen: VideoTrack { file: "screen.mov".into(), width: w, height: h, fps: 60.0, offset_ms: 0.0 },
            scale: 2.0,
            bounds: Rect::new(0.0, 0.0, w as f64 / 2.0, h as f64 / 2.0),
            ..Default::default()
        },
        mouse,
        keys,
        cursors: vec![CursorImage { id: 0, file: None, hotspot: (0.0, 0.0), size: (17.0, 23.0), system: Some("arrow".into()) }],
        transcript,
        beats: Default::default(),
    }
}

/// Write a synthetic `.glide` package (HEVC screen track plus scripted input).
/// Returns (frames, auto zooms).
pub fn write_package(output: &std::path::Path, duration: f64, width: u32, height: u32) -> anyhow::Result<(u64, usize)> {
    write(output, recording(width, height, duration * 1000.0), duration)
}

/// Like `write_package`, but recorded from an iPhone/iPad: no cursor or
/// keys (the project gets a device frame).
pub fn write_device_package(output: &std::path::Path, duration: f64, width: u32, height: u32) -> anyhow::Result<(u64, usize)> {
    let mut rec = recording(width, height, duration * 1000.0);
    rec.meta.kind = SourceKind::Device;
    rec.mouse.clear();
    rec.keys.clear();
    rec.cursors.clear();
    write(output, rec, duration)
}

/// Give a synthetic package a system-audio track: a sine at `hz`, 0.1
/// amplitude (-20 dBFS peak), for the whole recording.
pub fn add_tone(output: &std::path::Path, hz: f64) -> anyhow::Result<()> {
    use crate::project::recording::AudioTrack;
    use crate::project::{self, Package};
    let pkg = Package::new(output);
    let (_, mut rec) = pkg.load()?;
    let n = (rec.meta.duration_ms / 1000.0 * crate::capture::mixer::RATE) as usize;
    let tone: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let v = 0.1 * (std::f64::consts::TAU * hz * i as f64 / crate::capture::mixer::RATE).sin() as f32;
            [v, v]
        })
        .collect();
    crate::app::player::write_wav(&pkg.rec("system.wav"), &tone)?;
    rec.meta.system_audio = Some(AudioTrack { file: "system.wav".into(), track: 0 });
    project::write_json(&pkg.rec("meta.json"), &rec.meta)?;
    Ok(())
}

/// Give a synthetic package a system-audio track of kick drums, `bpm` beats a
/// minute with the first a quarter of a second in, and a shake effect over
/// the whole recording to follow them.
pub fn add_beats(output: &std::path::Path, bpm: f64) -> anyhow::Result<()> {
    use crate::capture::mixer::RATE;
    use crate::project::model::Effect;
    use crate::project::recording::AudioTrack;
    use crate::project::{self, Package};
    let pkg = Package::new(output);
    let (mut proj, mut rec) = pkg.load()?;
    let n = (rec.meta.duration_ms / 1000.0 * RATE) as usize;
    let mut audio = vec![[0.0f32; 2]; n];
    let every = 60_000.0 / bpm.max(1.0);
    let mut at = 250.0;
    while at < rec.meta.duration_ms {
        let start = (at / 1000.0 * RATE) as usize;
        // A 60 Hz kick that dies away over about 0.2 s.
        for k in 0..(0.2 * RATE) as usize {
            let t = k as f64 / RATE;
            if let Some(o) = audio.get_mut(start + k) {
                let v = (0.8 * (-t / 0.05).exp() * (std::f64::consts::TAU * 60.0 * t).sin()) as f32;
                *o = [v, v];
            }
        }
        at += every;
    }
    crate::app::player::write_wav(&pkg.rec("system.wav"), &audio)?;
    rec.meta.system_audio = Some(AudioTrack { file: "system.wav".into(), track: 0 });
    project::write_json(&pkg.rec("meta.json"), &rec.meta)?;
    let id = proj.timeline.new_id();
    proj.timeline.effects.push(Effect { id, start: 0.0, end: rec.meta.duration_ms, strength: 1.0, ..Default::default() });
    pkg.save_project(&proj)?;
    Ok(())
}

fn write(output: &std::path::Path, rec: Recording, duration: f64) -> anyhow::Result<(u64, usize)> {
    use crate::media::writer::{MovieWriter, WriterSpec, fill_bgra};
    use crate::project::{self, Package, model::Codec};
    let (width, height) = (rec.meta.screen.width, rec.meta.screen.height);
    let pkg = Package::new(output);
    std::fs::create_dir_all(pkg.recording_dir())?;
    let fps = 60u32;
    let mut w = MovieWriter::create(
        &pkg.rec("screen.mov"),
        &WriterSpec { width, height, fps, codec: Codec::Hevc, bitrate: (width * height) as u64 * 8, quality: None, audio_tracks: 0 },
    )?;
    let frames = (duration * fps as f64) as u64;
    for i in 0..frames {
        let px = screen_bgra(width, height, i as f64 * 1000.0 / fps as f64);
        let pb = w.buffer()?;
        fill_bgra(&pb, &px, width, height, (width * 4) as usize);
        w.push_frame(&pb)?;
    }
    w.finish()?;
    project::write_json(&pkg.rec("meta.json"), &rec.meta)?;
    project::write_json(&pkg.rec("mouse.json"), &rec.mouse)?;
    project::write_json(&pkg.rec("keys.json"), &rec.keys)?;
    project::write_json(&pkg.rec("cursors.json"), &rec.cursors)?;
    project::write_json(&pkg.rec("transcript.json"), &rec.transcript)?;
    let proj = project::new_project(&pkg.name(), &rec);
    pkg.save_project(&proj)?;
    Ok((frames, proj.timeline.zooms.len()))
}
