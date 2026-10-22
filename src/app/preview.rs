//! Editor preview: renders the current frame on the UI's GPU device and
//! shows it as an egui texture. Video frames are used in place (zero copy).
//! They're decoded on a worker thread per track (`scrub`); until a frame
//! arrives the preview shows the last one it has. The camera and the scene
//! (cursor, zoom, captions, masks) follow the screen frame on display, so
//! everything shows the same moment.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::scrub::{Signal, Worker};
use crate::engine::scene::{Frame, Scene};
use crate::media;
use crate::project::model::Project;
use crate::project::recording::Recording;
use crate::render::interop::TextureCache;
use crate::render::{Gpu, OUTPUT_FORMAT, Renderer};

pub struct Preview {
    pub scene: Scene,
    renderer: Renderer,
    cache: Option<TextureCache>,
    screen: Worker<media::Frame>,
    camera: Option<Worker<media::Frame>>,
    /// Raised by both workers when they publish a frame.
    signal: Arc<Signal>,
    camera_offset: f64,
    target: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    pub texture_id: Option<egui::TextureId>,
    /// (playback frame, height, project revision, video revision) last
    /// rendered.
    last: Option<(i64, u32, u64, u64)>,
    revision: u64,
    pub last_frame: Option<Frame>,
    package: PathBuf,
}

impl Preview {
    pub fn new(gpu: Gpu, package: PathBuf, project: &Project, rec: &Recording) -> Self {
        let rd = package.join("recording");
        let signal = Signal::new(super::wake);
        let screen = Worker::open(rd.join(&rec.meta.screen.file), "screen", signal.clone());
        let camera = rec.meta.camera.as_ref().map(|c| Worker::open(rd.join(&c.file), "camera", signal.clone()));
        let cache = TextureCache::new(&gpu.device);
        let mut renderer = Renderer::new(gpu);
        // Older Macs' GPUs can't afford the export-quality downscale live.
        renderer.fast_sampling = crate::hw::light();
        renderer.prepare(&project.style, rec, Some(&package));
        Preview {
            // Light mode previews at 30 fps (exports keep their own rate).
            scene: Scene::new(project.clone(), rec, if crate::hw::light() { 30.0 } else { 60.0 }),
            renderer,
            cache,
            screen,
            camera,
            signal,
            camera_offset: rec.meta.camera.as_ref().map_or(0.0, |c| c.offset_ms),
            target: None,
            texture_id: None,
            last: None,
            revision: 0,
            last_frame: None,
            package,
        }
    }

    /// The project changed: rebuild the scene.
    pub fn set_project(&mut self, project: &Project, rec: &Recording) {
        self.scene.set_project(project.clone(), rec);
        self.renderer.prepare(&project.style, rec, Some(&self.package));
        self.revision += 1;
    }

    pub fn output_aspect(&self) -> f32 {
        let o = self.scene.sizes.output;
        (o.x / o.y) as f32
    }

    /// Render the frame at `playback_ms` at `size` pixels (cheap when nothing
    /// changed). While playing, `next_ms` is the playhead at the next redraw,
    /// so its video frames decode ahead.
    pub fn render(&mut self, egui: &mut egui_wgpu::Renderer, playback_ms: f64, next_ms: Option<f64>, size: (u32, u32)) -> Option<egui::TextureId> {
        if self.draw(playback_ms, next_ms, size) {
            let device = &self.renderer.gpu().device;
            let view = &self.target.as_ref().unwrap().1;
            match self.texture_id {
                Some(id) => egui.update_egui_texture_from_wgpu_texture(device, view, wgpu::FilterMode::Linear, id),
                None => self.texture_id = Some(egui.register_native_texture(device, view, wgpu::FilterMode::Linear)),
            }
        }
        self.texture_id
    }

    /// Wait up to `timeout` for the video frames the last draw asked for;
    /// returns whether they arrived (or their track has none). For callers
    /// without an event loop for the workers to wake (tests, benchmarks).
    pub fn settle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.screen.wait(deadline) & self.camera.as_ref().is_none_or(|c| c.wait(deadline))
    }

    /// Render into the preview texture unless that frame is already there.
    /// Returns whether the texture was (re)created.
    pub fn draw(&mut self, playback_ms: f64, next_ms: Option<f64>, size: (u32, u32)) -> bool {
        let (w, h) = (size.0.max(16), size.1.max(16));
        let f = self.scene.frame_index(playback_ms);
        let src = self.scene.source_ms(f);
        // Through the time map, so cuts and slice speed land on the right
        // source frame.
        let ahead = next_ms.map(|ms| self.scene.source_ms(self.scene.frame_index(ms)));
        let newest = self.screen.request(src, ahead);
        let (screen, camera) = self.signal.read(|| {
            let screen = self.screen.latest();
            let camera = self.camera.as_ref().and_then(|c| {
                let at = camera_ms(screen.as_ref().map(|p| p.frame.pts_ms), src, self.camera_offset);
                c.request(at, ahead.map(|a| a - self.camera_offset));
                c.latest()
            });
            (screen, camera)
        });
        let resized = self.target.as_ref().is_none_or(|t| t.2 != (w, h));
        // The scene at the moment of the screen frame on display.
        let at = match &screen {
            Some(p) if p.generation != newest => scene_ms(&self.scene, p.source_ms),
            _ => Some(playback_ms),
        };
        let Some(at) = at.or_else(|| self.last.is_none().then_some(playback_ms)) else {
            // An edit cut that moment or moved it between preview frames:
            // keep the last picture, at its old size if the size changed,
            // until the new frame arrives.
            return false;
        };
        let shown = self.scene.frame_index(at);
        let video = screen.as_ref().map_or(0, |p| p.revision) + camera.as_ref().map_or(0, |p| p.revision);
        let key = (shown, h, self.revision, video);
        let gpu = self.renderer.gpu().clone();
        if !resized && self.last == Some(key) {
            return false;
        }
        if resized {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.target = Some((texture, view, (w, h)));
        }
        let frame = self.scene.frame(at, h as f64);
        if let Some(vf) = screen.map(|p| p.frame) {
            match self.cache.as_ref().and_then(|c| c.wrap(&vf.pb.0, wgpu::TextureUsages::TEXTURE_BINDING)) {
                Some(wr) => self.renderer.set_screen_wrapped(wr),
                None => vf.with_bgra(|v| self.renderer.set_screen(v)),
            }
        }
        if let Some(vf) = camera.map(|p| p.frame) {
            match self.cache.as_ref().and_then(|tc| tc.wrap(&vf.pb.0, wgpu::TextureUsages::TEXTURE_BINDING)) {
                Some(wr) => self.renderer.set_camera_wrapped(wr),
                None => vf.with_bgra(|v| self.renderer.set_camera(v)),
            }
        }
        let style = self.scene.project.style.clone();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let view = &self.target.as_ref().unwrap().1;
        self.renderer.render(&frame, &style, view, &mut enc);
        gpu.queue.submit([enc.finish()]);
        self.last = Some(key);
        self.last_frame = Some(frame);
        resized
    }

    /// The last rendered preview as RGBA (width, height, pixels).
    pub fn read_pixels(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (texture, _, (w, h)) = self.target.as_ref()?;
        let gpu = self.renderer.gpu();
        let enc = gpu.device.create_command_encoder(&Default::default());
        let mut px = crate::render::gpu::read_texture(gpu, texture, *w, *h, enc);
        // The target is BGRA.
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        Some((*w, *h, px))
    }
}

/// The source time to ask the camera for: the time of the screen frame on
/// display (its pts) shifted by the camera's offset, or the playhead's
/// source time `source_ms` while the screen has no frame (still decoding its
/// first, or its track failed to open).
fn camera_ms(screen_pts: Option<f64>, source_ms: f64, offset: f64) -> f64 {
    screen_pts.unwrap_or(source_ms) - offset
}

/// Where `scene` plays `source_ms`, the time an older screen frame on
/// display answers: a playhead whose preview frame shows exactly that
/// moment, so cursor, zoom, captions and masks match the picture under them.
/// None when an edit cut that moment or moved it between preview frames.
fn scene_ms(scene: &Scene, source_ms: f64) -> Option<f64> {
    let pb = scene.map.to_playback(source_ms)?;
    ((scene.source_ms(scene.frame_index(pb)) - source_ms).abs() < 1e-6).then_some(pb)
}

#[cfg(test)]
mod tests {
    use super::{camera_ms, scene_ms};
    use crate::engine::scene::Scene;
    use crate::project::model::{Project, Slice};

    #[test]
    fn the_camera_follows_the_screen_frame_on_display() {
        // The screen still shows 1000 ms while the playhead is at 1200.
        assert_eq!(camera_ms(Some(1000.0), 1200.0, 250.0), 750.0);
        // No screen frame yet: the playhead.
        assert_eq!(camera_ms(None, 1200.0, 250.0), 950.0);
    }

    #[test]
    fn the_scene_finds_the_moment_of_an_older_frame() {
        let rec = crate::synth::recording(1920, 1080, 10_000.0);
        let edit = |slices: &[(f64, f64, f64)]| {
            let mut p = Project::default();
            p.timeline.slices = slices.iter().map(|&(start, end, speed)| Slice { start, end, speed, ..Default::default() }).collect();
            Scene::new(p, &rec, 60.0)
        };
        // The frame at playhead 1000 asked for source 1000.
        let asked = edit(&[]).source_ms(60);
        assert_eq!(scene_ms(&edit(&[]), asked), Some(1000.0));
        // An edit cuts the first 500 ms: that moment now plays at 500.
        assert_eq!(scene_ms(&edit(&[(500.0, 10_000.0, 1.0)]), asked), Some(500.0));
        // It cuts the moment itself.
        assert_eq!(scene_ms(&edit(&[(2000.0, 10_000.0, 1.0)]), asked), None);
        // A cut right after it: playhead 1000 now shows the next slice.
        assert_eq!(scene_ms(&edit(&[(0.0, 1000.0, 1.0), (2000.0, 10_000.0, 1.0)]), asked), None);
        // A speed that puts it between two preview frames.
        assert_eq!(scene_ms(&edit(&[(0.0, 10_000.0, 1.25)]), edit(&[]).source_ms(1)), None);
        // Speed slices without an edit: every frame finds its own moment.
        let fast = edit(&[(0.0, 4000.0, 2.0), (4000.0, 10_000.0, 0.5)]);
        assert!((0..300).all(|f| scene_ms(&fast, fast.source_ms(f)).map(|pb| fast.frame_index(pb)) == Some(f)));
    }
}
