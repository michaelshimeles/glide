//! GPU compositor (wgpu on Metal). Draws an engine [`Frame`] into a texture:
//! background, shadowed rounded window with motion blur, cursor and click
//! effects, camera and text overlays.

pub mod art;
pub mod bgcheck;
pub mod gpu;
pub mod icon;
pub mod interop;
pub mod text;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use bytemuck::{Pod, Zeroable};

use crate::engine::geom::{Rect, Vec2, v2};
use crate::engine::scene::{CameraLayer, Frame, TextLayer};
use crate::project::model::{Background, Color, CursorSkin, MaskKind, Style};
use crate::project::recording::Recording;
pub use gpu::Gpu;

/// Format of the final frame (matches CoreVideo BGRA buffers).
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const LAYER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DRAW_STRIDE: u64 = 512;
const MAX_DRAWS: u64 = 256;

const MODE_TEXTURE: f32 = 0.0;
const MODE_SHADOW: f32 = 1.0;
const MODE_WINDOW: f32 = 2.0;
const MODE_CIRCLE: f32 = 3.0;
const MODE_GRADIENT: f32 = 4.0;
const MODE_COMPOSITE: f32 = 5.0;
const MODE_MASK: f32 = 6.0;
const MODE_DIM: f32 = 7.0;
const MODE_FILL: f32 = 8.0;
const MODE_LOUPE: f32 = 9.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Draw {
    rect: [f32; 4],
    xform: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
    shape: [f32; 4],
    p0: [f32; 4],
    p1: [f32; 4],
    p2: [f32; 4],
    p3: [f32; 4],
    p4: [f32; 4],
    out: [f32; 4],
    g0: [f32; 4],
    stops: [[f32; 4]; 4],
    stop_pos: [f32; 4],
}

fn r4(r: Rect) -> [f32; 4] {
    [r.x as f32, r.y as f32, r.w as f32, r.h as f32]
}

impl Draw {
    fn new(mode: f32, rect: Rect, size: Vec2) -> Self {
        Draw {
            rect: r4(rect),
            xform: [0.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            shape: r4(rect),
            p0: [0.0, 0.0, 0.0, 1.0],
            out: [size.x as f32, size.y as f32, mode, 0.0],
            ..Default::default()
        }
    }
}

/// A CPU image in premultiplied RGBA8.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn premultiply(mut self) -> Self {
        for px in self.rgba.chunks_exact_mut(4) {
            let a = px[3] as u32;
            for c in &mut px[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
        self
    }
    pub fn from_png(path: &Path) -> Result<Self> {
        let img = image::open(path)?.to_rgba8();
        Ok(Image { width: img.width(), height: img.height(), rgba: img.into_raw() }.premultiply())
    }
}

/// A decoded video frame in BGRA8 (as CoreVideo produces).
pub struct VideoFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bgra: &'a [u8],
}

struct Tex {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    size: (u32, u32),
}

/// Which texture a draw samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TexId {
    Dummy,
    Screen,
    Camera,
    Background,
    Layer,
    Cursor(u32),
    Text(u64),
}

pub struct Renderer {
    gpu: Gpu,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    tex_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    textures: HashMap<TexId, Tex>,
    layer_view: Option<(wgpu::TextureView, (u32, u32))>,
    cursor_art: HashMap<u32, (Vec2, f64)>,
    background: Option<Background>,
    background_size: Vec2,
    text: text::TextCache,
    text_used: Vec<u64>,
    has_screen: bool,
    has_camera: bool,
    /// Zero-copy frames still possibly in use by the GPU.
    keep: std::collections::VecDeque<interop::Wrapped>,
    /// Downscale the screen with a cheap 4-tap filter instead of the sharp
    /// Catmull-Rom one (live preview on slow GPUs; never for exports).
    pub fast_sampling: bool,
}

impl Renderer {
    pub fn new(gpu: Gpu) -> Self {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Draw>() as u64),
                },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draws"),
            size: DRAW_STRIDE * MAX_DRAWS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draws"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<Draw>() as u64),
                }),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mut r = Renderer {
            gpu,
            pipelines: HashMap::new(),
            shader,
            pipeline_layout,
            tex_layout,
            uniforms,
            uniform_bind,
            sampler,
            textures: HashMap::new(),
            layer_view: None,
            cursor_art: HashMap::new(),
            background: None,
            background_size: Vec2::ZERO,
            text: text::TextCache::new(),
            text_used: vec![],
            has_screen: false,
            has_camera: false,
            keep: std::collections::VecDeque::new(),
            fast_sampling: false,
        };
        r.upload(TexId::Dummy, &Image { width: 1, height: 1, rgba: vec![0, 0, 0, 0] }, false);
        r
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    fn pipeline(&mut self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (device, shader, layout) = (&self.gpu.device, &self.shader, &self.pipeline_layout);
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("quad"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn make_bind(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Create or replace a texture, optionally with a CPU-built mip chain.
    fn upload(&mut self, id: TexId, img: &Image, mips: bool) {
        let levels = if mips { mip_chain(img) } else { vec![img.clone()] };
        let size = wgpu::Extent3d { width: img.width.max(1), height: img.height.max(1), depth_or_array_layers: 1 };
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, l) in levels.iter().enumerate() {
            self.gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: i as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &l.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * l.width), rows_per_image: None },
                wgpu::Extent3d { width: l.width, height: l.height, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&Default::default());
        let bind = self.make_bind(&view);
        self.textures.insert(id, Tex { texture, bind, size: (img.width, img.height) });
    }

    /// Upload a BGRA video frame into a reusable texture.
    fn upload_video(&mut self, id: TexId, f: &VideoFrame) {
        let reuse = self.textures.get(&id).is_some_and(|t| t.size == (f.width, f.height));
        if !reuse {
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video"),
                size: wgpu::Extent3d { width: f.width, height: f.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = self.make_bind(&view);
            self.textures.insert(id, Tex { texture, bind, size: (f.width, f.height) });
        }
        let tex = &self.textures[&id].texture;
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            f.bgra,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(f.stride as u32), rows_per_image: None },
            wgpu::Extent3d { width: f.width, height: f.height, depth_or_array_layers: 1 },
        );
    }

    fn set_wrapped(&mut self, id: TexId, w: interop::Wrapped) {
        let bind = self.make_bind(&w.view);
        self.textures.insert(id, Tex { texture: w.texture.clone(), bind, size: w.size });
        self.keep.push_back(w);
        while self.keep.len() > 8 {
            self.keep.pop_front();
        }
    }

    /// Use an IOSurface-backed frame directly (no copy).
    pub fn set_screen_wrapped(&mut self, w: interop::Wrapped) {
        self.set_wrapped(TexId::Screen, w);
        self.has_screen = true;
    }

    pub fn set_camera_wrapped(&mut self, w: interop::Wrapped) {
        self.set_wrapped(TexId::Camera, w);
        self.has_camera = true;
    }

    pub fn set_screen(&mut self, f: &VideoFrame) {
        self.upload_video(TexId::Screen, f);
        self.has_screen = true;
    }

    pub fn set_camera(&mut self, f: &VideoFrame) {
        self.upload_video(TexId::Camera, f);
        self.has_camera = true;
    }

    /// Load style-dependent assets (background, cursors). Cheap when unchanged.
    pub fn prepare(&mut self, style: &Style, rec: &Recording, package_dir: Option<&Path>) {
        if self.background.as_ref() != Some(&style.background) {
            self.background = Some(style.background.clone());
            let img = match &style.background {
                Background::Wallpaper { id } => Some(art::wallpaper(id, 1920, 1200)),
                Background::Image { path } => {
                    // Relative paths live inside the package (imported images).
                    let p = Path::new(path);
                    let full = match package_dir {
                        Some(dir) if p.is_relative() => dir.join(p),
                        _ => p.to_path_buf(),
                    };
                    Image::from_png(&full).ok()
                }
                _ => None,
            };
            if let Some(img) = img {
                self.background_size = v2(img.width as f64, img.height as f64);
                self.upload(TexId::Background, &img, true);
            } else {
                self.textures.remove(&TexId::Background);
            }
        }
        let skin_key = match style.cursor.skin {
            CursorSkin::System => 0,
            CursorSkin::Vector => 1,
            CursorSkin::Touch => 2,
        };
        if self.cursor_art.get(&u32::MAX).map(|v| v.1) != Some(skin_key as f64) {
            self.cursor_art.clear();
            for k in self.textures.keys().copied().collect::<Vec<_>>() {
                if matches!(k, TexId::Cursor(_)) {
                    self.textures.remove(&k);
                }
            }
            for c in &rec.cursors {
                let (img, hotspot, scale) = match style.cursor.skin {
                    CursorSkin::System => {
                        let loaded =
                            c.file.as_ref().zip(package_dir).and_then(|(f, dir)| Image::from_png(&dir.join("recording/cursors").join(f)).ok());
                        match loaded {
                            Some(img) => {
                                // Height relative to a standard 23 pt arrow.
                                let scale = c.size.1 / 23.0;
                                (img, v2(c.hotspot.0, c.hotspot.1), scale)
                            }
                            None => art::vector_cursor(c.system.as_deref()),
                        }
                    }
                    CursorSkin::Vector => art::vector_cursor(c.system.as_deref()),
                    CursorSkin::Touch => art::touch_cursor(),
                };
                self.upload(TexId::Cursor(c.id), &img, true);
                self.cursor_art.insert(c.id, (hotspot, scale));
            }
            if rec.cursors.is_empty() {
                let (img, hotspot, scale) = match style.cursor.skin {
                    CursorSkin::Touch => art::touch_cursor(),
                    _ => art::vector_cursor(Some("arrow")),
                };
                self.upload(TexId::Cursor(0), &img, true);
                self.cursor_art.insert(0, (hotspot, scale));
            }
            self.cursor_art.insert(u32::MAX, (Vec2::ZERO, skin_key as f64));
        }
    }

    fn ensure_layer(&mut self, w: u32, h: u32) {
        if self.layer_view.as_ref().is_some_and(|(_, s)| *s == (w, h)) {
            return;
        }
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: LAYER_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = self.make_bind(&view);
        self.textures.insert(TexId::Layer, Tex { texture, bind, size: (w, h) });
        self.layer_view = Some((view, (w, h)));
    }

    fn text_tex(&mut self, t: &TextLayer) -> Option<(u64, (u32, u32))> {
        let px = t.size.round().max(4.0) as u32;
        let key = text::key(&t.text, px, t.badge);
        if !self.textures.contains_key(&TexId::Text(key)) {
            let img = self.text.render(&t.text, px as f32, t.badge)?;
            self.upload(TexId::Text(key), &img, false);
        }
        self.text_used.push(key);
        Some((key, self.textures[&TexId::Text(key)].size))
    }

    /// Render `frame` into `target` (which must be `frame.size` and OUTPUT_FORMAT).
    pub fn render(&mut self, frame: &Frame, style: &Style, target: &wgpu::TextureView, encoder: &mut wgpu::CommandEncoder) {
        let size = frame.size;
        let (w, h) = (size.x.round() as u32, size.y.round() as u32);
        self.ensure_layer(w.max(1), h.max(1));
        let full = Rect::from_min_size(Vec2::ZERO, size);
        let k = size.y / 1080.0;

        let mut layer: Vec<(Draw, TexId)> = vec![];
        let mut main: Vec<(Draw, TexId)> = vec![];

        // Background.
        let bg_blur = (style.background_blur * k + frame.backdrop_blur) as f32;
        match &style.background {
            Background::Gradient { angle, stops } => main.push((gradient_draw(full, size, *angle, stops), TexId::Dummy)),
            Background::Color { color } => main.push((gradient_draw(full, size, 0.0, &[(*color, 0.0)]), TexId::Dummy)),
            _ if self.textures.contains_key(&TexId::Background) => {
                let mut d = Draw::new(MODE_TEXTURE, full, size);
                d.uv = r4(cover_uv(self.background_size, size));
                d.p2[0] = bg_blur;
                main.push((d, TexId::Background));
            }
            _ => main.push((gradient_draw(full, size, 0.0, &[(Color::rgb(30, 30, 34), 0.0)]), TexId::Dummy)),
        }

        // Window layer: shadow + window, drawn to an offscreen layer so the
        // whole moving window can be motion-blurred when composited.
        let win = &frame.window;
        let s = win.shadow;
        if s.intensity > 0.0 {
            let ws = win.scale;
            let dist = s.distance * ws;
            let a = s.angle.to_radians();
            let off = v2(a.cos() * dist, a.sin() * dist);
            let sigma = (s.blur * ws).max(1.0) * 0.6;
            // With a device frame the shadow follows the device body.
            let (outline, radius) = frame.device_body.unwrap_or((win.rect, win.radius));
            let shape = outline.translate(off);
            let rect = shape.inset(-(sigma * 3.0 + 2.0));
            let mut d = Draw::new(MODE_SHADOW, rect, size);
            d.shape = r4(shape);
            d.color = [0.0, 0.0, 0.0, (s.intensity * 0.85) as f32];
            d.p0[0] = radius as f32;
            d.p2[0] = sigma as f32;
            layer.push((d, TexId::Dummy));
        }
        let device_draw = |p: &crate::engine::device::Part| {
            let mut d = Draw::new(MODE_FILL, p.rect.inset(-1.0), size);
            d.shape = r4(p.rect);
            d.color = p.top.to_f32();
            d.p1 = p.bottom.to_f32();
            d.p4 = p.border.to_f32();
            d.p0 = [p.radius as f32, 0.0, 0.0, 1.0];
            d.p2[0] = p.border_width as f32;
            d
        };
        for p in frame.device.iter().filter(|p| !p.over) {
            layer.push((device_draw(p), TexId::Dummy));
        }
        {
            let mut d = Draw::new(MODE_WINDOW, win.rect.inset(-1.0), size);
            d.shape = r4(win.rect);
            d.uv = r4(win.uv);
            d.p0 = [win.radius as f32, win.inner_radius as f32, win.inset as f32, 1.0];
            let ic = win.inset_color.to_f32();
            d.p1 = [ic[0], ic[1], ic[2], (win.inset_alpha as f32) * ic[3]];
            if let Some(r) = frame.ripples.last() {
                d.p3 = [r.center.x as f32, r.center.y as f32, r.progress as f32, r.strength as f32];
                d.p4[0] = r.reach as f32;
            }
            d.p4[1] = if self.fast_sampling { 1.0 } else { 0.0 };
            let tex = if self.has_screen { TexId::Screen } else { TexId::Dummy };
            layer.push((d, tex));
        }
        for p in frame.device.iter().filter(|p| p.over) {
            layer.push((device_draw(p), TexId::Dummy));
        }
        // Masks move with the window, so they go into the same layer.
        for m in &frame.masks {
            match m.kind {
                MaskKind::Blur | MaskKind::Pixelate => {
                    if !self.has_screen {
                        continue;
                    }
                    let mut d = Draw::new(MODE_MASK, m.rect, size);
                    d.shape = r4(m.rect);
                    d.uv = r4(m.uv);
                    d.p0 = [m.radius as f32, 0.0, 0.0, m.alpha as f32];
                    let flags = if m.kind == MaskKind::Pixelate { 4.0 } else { 0.0 };
                    d.p2 = [m.amount as f32, 0.0, 0.0, flags];
                    d.p3 = r4(win.uv);
                    layer.push((d, TexId::Screen));
                }
                MaskKind::Highlight => {
                    let feather = 3.0 * win.scale;
                    let mut d = Draw::new(MODE_DIM, m.screen, size);
                    d.shape = r4(m.rect);
                    d.color = [0.0, 0.0, 0.0, m.amount.clamp(0.0, 1.0) as f32];
                    d.p0 = [m.radius as f32, m.screen_radius as f32, feather as f32, m.alpha as f32];
                    d.p3 = r4(m.screen);
                    layer.push((d, TexId::Dummy));
                }
            }
        }
        {
            let mut d = Draw::new(MODE_COMPOSITE, full, size);
            d.p2 = [frame.backdrop_blur as f32, win.blur.velocity.x as f32, win.blur.velocity.y as f32, 0.0];
            d.p4 = [0.0, win.blur.zoom as f32, win.blur.zoom_center.x as f32, win.blur.zoom_center.y as f32];
            main.push((d, TexId::Layer));
        }

        // Loupe magnifier over the window (under the cursor and click FX).
        // It samples the composited window layer — masks and all — so it can
        // never reveal detail a blur or pixelate mask covers.
        if self.has_screen
            && let Some(l) = &frame.loupe
        {
            let r = l.radius.max(1.0);
            let shape = Rect::new(l.center.x - r, l.center.y - r, r * 2.0, r * 2.0);
            let mut d = Draw::new(MODE_LOUPE, shape, size);
            d.p0 = [r as f32, (size.y * 0.005).max(1.5) as f32, 0.0, l.alpha as f32];
            let half = r / l.mag.max(1.0);
            d.uv = r4(Rect::new((l.center.x - half) / size.x, (l.center.y - half) / size.y, half * 2.0 / size.x, half * 2.0 / size.y));
            main.push((d, TexId::Layer));
        }

        // Click circles under the cursor.
        for c in &frame.circles {
            let r = c.radius.max(0.5);
            let shape = Rect::new(c.pos.x - r, c.pos.y - r, 2.0 * r, 2.0 * r);
            let mut d = Draw::new(MODE_CIRCLE, shape.inset(-(c.blur + 3.0)), size);
            d.shape = r4(shape);
            d.color = [0.87, 0.87, 0.87, 1.0];
            d.p0[3] = c.alpha as f32;
            d.p2[0] = c.blur.max(1.0) as f32;
            main.push((d, TexId::Dummy));
        }

        // Cursor.
        if let Some(c) = &frame.cursor
            && c.alpha > 0.002
        {
            let id = if self.textures.contains_key(&TexId::Cursor(c.image)) { c.image } else { 0 };
            if let (Some(t), Some(&(hot, scale))) = (self.textures.get(&TexId::Cursor(id)), self.cursor_art.get(&id)) {
                let (tw, th) = (t.size.0 as f64, t.size.1 as f64);
                let hgt = c.height * scale * c.scale;
                let wid = hgt * tw / th;
                let rect = Rect::new(c.pos.x - hot.x * wid, c.pos.y - hot.y * hgt, wid, hgt);
                let mut d = Draw::new(MODE_TEXTURE, rect, size);
                d.xform = [c.pos.x as f32, c.pos.y as f32, c.rotation.to_radians() as f32, (c.velocity.length() + c.blur * 2.0 + 2.0) as f32];
                d.p0[3] = c.alpha as f32;
                d.p2 = [c.blur as f32, c.velocity.x as f32, c.velocity.y as f32, 0.0];
                main.push((d, TexId::Cursor(id)));
            }
        }

        // Camera.
        if self.has_camera {
            if let Some(cam) = &frame.camera {
                if cam.shadow {
                    // The screen's shadow, or a soft one when that is off.
                    let (blur, angle, distance, intensity) = if style.shadow.intensity > 0.0 {
                        (style.shadow.blur * k, style.shadow.angle, style.shadow.distance * k, style.shadow.intensity * 0.85)
                    } else {
                        (size.y * 0.03, 90.0, size.y * 0.008, 0.45)
                    };
                    let sigma = blur.max(1.0) * 0.6;
                    let a = angle.to_radians();
                    let off = v2(a.cos(), a.sin()) * (distance * 0.6);
                    let shape = cam.rect.translate(off);
                    let mut d = Draw::new(MODE_SHADOW, shape.inset(-(sigma * 3.0 + 2.0)), size);
                    d.shape = r4(shape);
                    d.color = [0.0, 0.0, 0.0, (intensity * cam.alpha) as f32];
                    d.p0[0] = cam.radius as f32;
                    d.p2[0] = sigma as f32;
                    main.push((d, TexId::Dummy));
                }
                main.push((camera_draw(cam, size, true), TexId::Camera));
            }
            if let Some(cam) = &frame.camera_full {
                main.push((camera_draw(cam, size, false), TexId::Camera));
            }
        }

        // Text overlays.
        self.text_used.clear();
        for t in &frame.texts {
            if let Some((key, (tw, th))) = self.text_tex(t) {
                let (tw, th) = (tw as f64, th as f64);
                let rect = Rect::new(t.center.x - tw / 2.0, t.center.y - th / 2.0, tw, th);
                let mut d = Draw::new(MODE_TEXTURE, rect, size);
                d.p0[3] = t.alpha as f32;
                d.p2[0] = t.blur as f32;
                d.xform[3] = (t.blur * 2.0) as f32;
                main.push((d, TexId::Text(key)));
            }
        }
        if self.textures.len() > 96 {
            let used = &self.text_used;
            self.textures.retain(|k, _| !matches!(k, TexId::Text(id) if !used.contains(id)));
        }

        // Upload all draw uniforms, then encode both passes.
        let draws: Vec<Draw> = layer.iter().chain(main.iter()).map(|(d, _)| *d).collect();
        let mut bytes = vec![0u8; (draws.len() as u64 * DRAW_STRIDE) as usize];
        for (i, d) in draws.iter().enumerate() {
            let at = i * DRAW_STRIDE as usize;
            bytes[at..at + std::mem::size_of::<Draw>()].copy_from_slice(bytemuck::bytes_of(d));
        }
        if !bytes.is_empty() {
            self.gpu.queue.write_buffer(&self.uniforms, 0, &bytes);
        }
        let layer_view = self.layer_view.as_ref().unwrap().0.clone();
        self.pipeline(LAYER_FORMAT);
        self.pipeline(OUTPUT_FORMAT);
        let passes: [(&wgpu::TextureView, wgpu::TextureFormat, &[(Draw, TexId)], usize, wgpu::Color); 2] = [
            (&layer_view, LAYER_FORMAT, &layer, 0, wgpu::Color::TRANSPARENT),
            (target, OUTPUT_FORMAT, &main, layer.len(), wgpu::Color::BLACK),
        ];
        for (view, format, list, base, clear) in passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("compose"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipelines[&format]);
            for (i, (_, tex)) in list.iter().enumerate() {
                let offset = ((base + i) as u64 * DRAW_STRIDE) as u32;
                pass.set_bind_group(0, &self.uniform_bind, &[offset]);
                let bind = &self.textures.get(tex).unwrap_or(&self.textures[&TexId::Dummy]).bind;
                pass.set_bind_group(1, bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }

    /// Render a frame to a new texture and read it back as RGBA8.
    pub fn render_to_image(&mut self, frame: &Frame, style: &Style) -> Image {
        let (w, h) = (frame.size.x.round() as u32, frame.size.y.round() as u32);
        let target = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("snapshot"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut enc = self.gpu.device.create_command_encoder(&Default::default());
        self.render(frame, style, &view, &mut enc);
        let mut bgra = gpu::read_texture(&self.gpu, &target, w, h, enc);
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Image { width: w, height: h, rgba: bgra }
    }
}

fn camera_draw(cam: &CameraLayer, size: Vec2, rounded: bool) -> Draw {
    let mut d = Draw::new(MODE_TEXTURE, cam.rect, size);
    d.uv = r4(cam.uv);
    d.shape = r4(cam.rect);
    d.p0 = [cam.radius as f32, cam.border as f32, cam.squircle as f32, cam.alpha as f32];
    d.p4 = cam.border_color;
    let flags = (cam.mirror as u32) | if rounded { 2 } else { 0 };
    d.p2 = [cam.blur as f32, 0.0, 0.0, flags as f32];
    d.xform[3] = 1.0;
    d
}

fn gradient_draw(rect: Rect, size: Vec2, angle: f64, stops: &[(Color, f64)]) -> Draw {
    let mut d = Draw::new(MODE_GRADIENT, rect, size);
    // Angle in CSS convention: 0 = to top, 90 = to right.
    let a = angle.to_radians();
    d.g0 = [a.sin() as f32, -a.cos() as f32, 0.0, 0.0];
    let n = stops.len().clamp(1, 4);
    for i in 0..4 {
        let (c, p) = stops.get(i.min(stops.len().saturating_sub(1))).copied().unwrap_or((Color::BLACK, 0.0));
        d.stops[i] = c.to_f32();
        d.stop_pos[i] = p as f32;
    }
    d.out[3] = n as f32;
    d
}

/// UV rect that makes an image cover a target size (centre crop).
fn cover_uv(img: Vec2, target: Vec2) -> Rect {
    if img.x <= 0.0 || img.y <= 0.0 {
        return Rect::UNIT;
    }
    crate::engine::scene::centre_crop(img.aspect(), target.aspect())
}

fn mip_chain(img: &Image) -> Vec<Image> {
    let mut levels = vec![img.clone()];
    let mut cur = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()).unwrap();
    while cur.width() > 1 || cur.height() > 1 {
        let (w, h) = ((cur.width() / 2).max(1), (cur.height() / 2).max(1));
        cur = image::imageops::resize(&cur, w, h, image::imageops::FilterType::Triangle);
        levels.push(Image { width: w, height: h, rgba: cur.as_raw().clone() });
        if levels.len() >= 12 {
            break;
        }
    }
    levels
}
