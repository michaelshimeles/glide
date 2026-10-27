//! Turns a project + recording into a description of each output frame.
//! The renderer draws a [`Frame`]; nothing here touches the GPU.

use std::collections::HashMap;

use super::animator::{Animator, Target};
use super::camera;
use super::captions::{self, Line};
use super::cursor::{self, ClickFx, CursorModel};
use super::events::Events;
use super::geom::{Rect, Vec2, v2};
use super::keys::{self, Badge};
use super::sizes::Sizes;
use super::spring::SpringConfig;
use super::timemap::TimeMap;
use super::zoom;
use crate::project::model::{ClickEffect, Color, LayoutKind, MaskKind, Project, Shadow};
use crate::project::recording::Recording;

/// Directional or radial blur for a moving layer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Blur {
    /// Streak in output pixels (move blur).
    pub velocity: Vec2,
    /// Radial blur strength (zoom blur) and its centre in output pixels.
    pub zoom: f64,
    pub zoom_center: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowLayer {
    /// Outer window rect (including inset) in output pixels.
    pub rect: Rect,
    pub radius: f64,
    /// Inset thickness in output pixels.
    pub inset: f64,
    pub inset_color: Color,
    pub inset_alpha: f64,
    /// Inner (screen) corner radius.
    pub inner_radius: f64,
    /// Crop of the screen video, normalised.
    pub uv: Rect,
    pub shadow: Shadow,
    /// Output pixels per 1080p-frame pixel times the zoom (for shadow sizing).
    pub scale: f64,
    pub blur: Blur,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CursorLayer {
    /// Hotspot position in output pixels.
    pub pos: Vec2,
    /// Sprite height in output pixels (before press scale).
    pub height: f64,
    pub rotation: f64,
    pub alpha: f64,
    pub scale: f64,
    pub blur: f64,
    pub image: u32,
    pub velocity: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CircleFx {
    pub pos: Vec2,
    pub radius: f64,
    pub alpha: f64,
    pub blur: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RippleFx {
    pub center: Vec2,
    pub progress: f64,
    pub strength: f64,
    /// Ring radius at the end of the effect, output pixels.
    pub reach: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraLayer {
    pub rect: Rect,
    pub radius: f64,
    /// Superellipse exponent of the outline (0 = rounded rectangle).
    pub squircle: f64,
    /// Border width in output pixels and its colour.
    pub border: f64,
    pub border_color: [f32; 4],
    pub shadow: bool,
    pub alpha: f64,
    pub blur: f64,
    pub mirror: bool,
    /// Centre crop of the camera video to the bubble aspect, normalised.
    pub uv: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextLayer {
    pub text: String,
    /// Font size in output pixels.
    pub size: f64,
    /// Baseline centre position (bottom-centred).
    pub center: Vec2,
    pub alpha: f64,
    pub blur: f64,
    /// Draw as a key badge (pill) instead of a caption.
    pub badge: bool,
}

/// A region effect on the screen (blur, pixelate or highlight).
#[derive(Clone, Debug, PartialEq)]
pub struct MaskLayer {
    pub kind: MaskKind,
    /// Region in output px (already clipped to the visible screen).
    pub rect: Rect,
    /// The same region in screen-texture uv.
    pub uv: Rect,
    pub radius: f64,
    /// Blur/pixel size in output px, or dim opacity for highlights.
    pub amount: f64,
    pub alpha: f64,
    /// The window's inner (screen) rect and corner radius, for clipping.
    pub screen: Rect,
    pub screen_radius: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Output size in pixels.
    pub size: Vec2,
    pub playback_ms: f64,
    pub source_ms: f64,
    /// Extra background/screen blur (camera-only layout), output pixels.
    pub backdrop_blur: f64,
    pub window: WindowLayer,
    pub cursor: Option<CursorLayer>,
    pub circles: Vec<CircleFx>,
    pub ripples: Vec<RippleFx>,
    pub camera: Option<CameraLayer>,
    pub camera_full: Option<CameraLayer>,
    pub texts: Vec<TextLayer>,
    pub masks: Vec<MaskLayer>,
    /// Device frame shapes around the window, back to front.
    pub device: Vec<super::device::Part>,
    /// The device body outline for the drop shadow.
    pub device_body: Option<(Rect, f64)>,
}

struct Anim {
    screen: Animator,
    cursor: Animator,
    fade: Animator,
    bubble: Animator,
    bubble_vis: Animator,
    full: Animator,
    backdrop: Animator,
    badges: HashMap<usize, Animator>,
    annotations: HashMap<u64, Animator>,
}

impl Anim {
    fn new(fps: f64) -> Self {
        Anim {
            screen: Animator::new(fps),
            cursor: Animator::new(fps),
            fade: Animator::new(fps),
            bubble: Animator::new(fps),
            bubble_vis: Animator::new(fps),
            full: Animator::new(fps),
            backdrop: Animator::new(fps),
            badges: HashMap::new(),
            annotations: HashMap::new(),
        }
    }
}

pub struct Scene {
    pub project: Project,
    pub sizes: Sizes,
    pub map: TimeMap,
    events: Events,
    badges: Vec<Badge>,
    lines: Vec<Line>,
    camera_video: Option<Vec2>,
    has_cursor: bool,
    duration: f64,
    fps: f64,
    anim: Anim,
}

impl Scene {
    pub fn new(project: Project, rec: &Recording, fps: f64) -> Self {
        let (w, h) = rec.screen_size();
        let sizes = Sizes::new(&project.style, v2(w, h));
        let map = TimeMap::new(&project.timeline.slices, rec.meta.duration_ms);
        Scene {
            sizes,
            map,
            events: Events::new(rec.mouse.clone()),
            badges: keys::badges(&rec.keys, project.style.shortcuts.single_keys),
            lines: captions::lines(&rec.transcript),
            camera_video: rec.meta.camera.as_ref().map(|c| v2(c.width as f64, c.height as f64)),
            has_cursor: !rec.mouse.is_empty(),
            duration: rec.meta.duration_ms,
            project,
            fps,
            anim: Anim::new(fps),
        }
    }

    /// Replace the project (after an edit) and drop cached animation state.
    pub fn set_project(&mut self, project: Project, rec: &Recording) {
        let fps = self.fps;
        *self = Scene::new(project, rec, fps);
    }

    pub fn fps(&self) -> f64 {
        self.fps
    }

    pub fn duration(&self) -> f64 {
        self.map.duration()
    }

    pub fn frame_count(&self) -> u64 {
        (self.duration() * self.fps / 1000.0).ceil() as u64
    }

    pub fn frame_index(&self, playback_ms: f64) -> i64 {
        (playback_ms * self.fps / 1000.0).round() as i64
    }

    fn screen_values(&mut self, f: i64) -> Vec<f64> {
        let (map, sizes, tl, events) = (&self.map, &self.sizes, &self.project.timeline, &self.events);
        let spring = self.project.style.screen_spring;
        let cm = CursorModel {
            events,
            style: &self.project.style.cursor,
            slices: &tl.slices,
            source_start: map.source_start(),
            source_end: map.source_end().min(self.duration),
        };
        self.anim.screen.values_at(f, |pb| {
            let src = map.to_source(pb);
            let focus = zoom::focus_at(tl, events, sizes, src, cm.target(src));
            let (scale, off) = zoom::place(sizes, focus);
            Target { values: vec![scale, off.x, off.y], spring, snap: zoom::is_instant_edge(&tl.zooms, src) }
        })
    }

    fn cursor_values(&mut self, f: i64) -> Vec<f64> {
        let map = &self.map;
        let cm = CursorModel {
            events: &self.events,
            style: &self.project.style.cursor,
            slices: &self.project.timeline.slices,
            source_start: map.source_start(),
            source_end: map.source_end().min(self.duration),
        };
        self.anim.cursor.values_at(f, |pb| {
            let src = map.to_source(pb);
            let p = cm.target(src).unwrap_or_default();
            Target::new(vec![p.x, p.y, cm.tilt(src)], cm.spring(src))
        })
    }

    /// Window rect in output pixels for a frame, plus its zoom scale.
    fn window_rect(&mut self, f: i64, k: f64) -> (Rect, f64) {
        let v = self.screen_values(f);
        let body = self.sizes.default_body();
        let pad = Vec2::splat(self.sizes.pad);
        let min = pad + body.min() + v2(v[1], v[2]);
        (Rect::from_min_size(min, body.size() * v[0]).scale_about_origin(k), v[0])
    }

    /// Map an output-pixel point back to screen-video pixels given the window rect.
    pub fn output_to_screen(&self, p: Vec2, window: Rect) -> Vec2 {
        let s = &self.sizes;
        let local = (p - window.min()).div(window.size().div(s.window));
        local + s.crop.min() - Vec2::splat(s.inset)
    }

    /// Map a screen-video point to output pixels given the window rect.
    pub fn screen_to_output(&self, p: Vec2, window: Rect) -> Vec2 {
        let s = &self.sizes;
        let local = p - s.crop.min() + Vec2::splat(s.inset);
        window.min() + local.mul(window.size().div(s.window))
    }

    /// Describe the frame at a playback time for an output of `height` px.
    pub fn frame(&mut self, playback_ms: f64, height: f64) -> Frame {
        let f = self.frame_index(playback_ms);
        let pb = f as f64 * 1000.0 / self.fps;
        let src = self.map.to_source(pb);
        let k = height / self.sizes.output.y;
        let size = self.sizes.output * k;
        let style = self.project.style.clone();
        let blur_gain = self.fps / 60.0;

        // Window and its motion.
        let (rect, zoom_scale) = self.window_rect(f, k);
        let prev = if f > 0 { Some(self.window_rect(f - 1, k).0) } else { None };
        let mut wblur = Blur::default();
        if let Some(prev) = prev {
            let mb = &style.motion_blur;
            let d_center = rect.center() - prev.center();
            let d_size = (rect.size() - prev.size()).length();
            if d_size > d_center.length() && d_size >= 1.0 {
                let ratio = rect.diagonal() / prev.diagonal();
                wblur.zoom = (1.0 - ratio).abs() * mb.amount * mb.screen_zoom * blur_gain;
                if (prev.w - rect.w).abs() > 1e-6 {
                    wblur.zoom_center = (rect.min() * prev.w - prev.min() * rect.w) / (prev.w - rect.w);
                }
            } else if d_center.length() >= 1.0 {
                wblur.velocity = d_center * (mb.amount * mb.screen_move * blur_gain);
            }
        }
        // Roundness and shadow are px in a 1080p-tall frame at rest, whatever
        // the recording's resolution (like the background blur), growing
        // with the zoom.
        let style_px = k * self.sizes.output.y / 1080.0 * zoom_scale / self.sizes.initial_zoom;
        let rscale = rect.w / self.sizes.window.x;
        let inset = self.sizes.inset * rscale;
        let radius = style.corner_radius * style_px;
        let inner_radius = (radius - inset).max(4.0 * style_px);
        let rec = self.sizes.recording;
        let crop = self.sizes.crop;
        let device_screen = super::device::screen_radius(style.device, rect);
        let radius = device_screen.unwrap_or(radius);
        let inner_radius = device_screen.map_or(inner_radius, |r| (r - inset).max(0.0));
        let window = WindowLayer {
            rect,
            radius,
            inset,
            inset_color: style.inset.color,
            inset_alpha: style.inset.alpha,
            inner_radius: if inset > 0.0 { inner_radius } else { radius },
            uv: Rect::new(crop.x / rec.x, crop.y / rec.y, crop.w / rec.x, crop.h / rec.y),
            shadow: style.shadow,
            scale: style_px,
            blur: wblur,
        };

        // The visible (cropped) screen in output px.
        let screen_rect = Rect::from_min_size(
            self.screen_to_output(self.sizes.crop.min(), rect),
            self.sizes.crop.size().mul(rect.size().div(self.sizes.window)),
        );

        // Cursor.
        let mut cursor = None;
        let mut circles = vec![];
        let mut ripples = vec![];
        if self.has_cursor && !style.cursor.hide {
            let cv = self.cursor_values(f);
            let pv = if f > 0 { Some(self.cursor_values(f - 1)) } else { None };
            let map = &self.map;
            let cm = CursorModel {
                events: &self.events,
                style: &self.project.style.cursor,
                slices: &self.project.timeline.slices,
                source_start: map.source_start(),
                source_end: map.source_end().min(self.duration),
            };
            let fv = self.anim.fade.values_at(f, |pb| Target::new(cm.fade(map.to_source(pb)).to_vec(), SpringConfig::PRESS));
            let pos = self.screen_to_output(v2(cv[0], cv[1]), rect);
            let velocity = match (&pv, prev) {
                (Some(p), Some(prev)) => {
                    let before = self.screen_to_output(v2(p[0], p[1]), prev);
                    (pos - before) * (style.motion_blur.amount * style.motion_blur.cursor * blur_gain)
                }
                _ => Vec2::ZERO,
            };
            let image = Events::last_at(&self.events.all, src).map_or(0, |e| e.cursor);
            // A cursor outside the cropped screen isn't shown.
            let visible = screen_rect.inset(-2.0).contains(pos);
            cursor = Some(CursorLayer {
                pos,
                height: cursor::BASE_HEIGHT * style.cursor.size * k * zoom_scale,
                rotation: cv[2],
                alpha: if visible { fv[0].clamp(0.0, 1.0) } else { 0.0 },
                scale: fv[1],
                blur: fv[2].max(0.0),
                image,
                velocity,
            });
            let fx_px = 16.0 * style.cursor.size * k * zoom_scale;
            match style.cursor.click_effect {
                ClickEffect::Circle => {
                    circles = cursor::circle_effects(&self.events, src)
                        .into_iter()
                        .map(|c: ClickFx| CircleFx {
                            pos: self.screen_to_output(c.pos, rect),
                            radius: fx_px * c.scale,
                            alpha: c.alpha * 0.6,
                            blur: c.blur * k,
                        })
                        .collect();
                }
                ClickEffect::Ripple { strength } => {
                    ripples = cursor::ripple_effects(&self.events, src)
                        .into_iter()
                        .map(|c| RippleFx {
                            center: self.screen_to_output(c.pos, rect),
                            progress: c.progress,
                            strength,
                            reach: 400.0 * k * zoom_scale,
                        })
                        .collect();
                }
                ClickEffect::None => {}
            }
        }

        // Camera.
        let layout = camera::kind_at(&self.project.timeline.layouts, src);
        let map = self.map.clone();
        let layouts = self.project.timeline.layouts.clone();
        let backdrop = self.anim.backdrop.values_at(f, |pb| {
            let only_cam = camera::kind_at(&layouts, map.to_source(pb)) == LayoutKind::CameraOnly;
            Target::new(vec![if only_cam { 15.0 } else { 0.0 }], SpringConfig::PRESS)
        })[0];
        let (mut cam, mut cam_full) = (None, None);
        if let Some(video) = self.camera_video.filter(|_| !style.camera.hide) {
            let cs = style.camera;
            let zooms = self.project.timeline.zooms.clone();
            let out = self.sizes.output;
            let b = self.anim.bubble.values_at(f, |pb| {
                let s = map.to_source(pb);
                let zoomed = zoom::active(&zooms, s).is_some();
                let r = camera::bubble_at(&cs, &layouts, out, video, s, zoomed);
                Target { values: vec![r.x, r.y, r.w, r.h], spring: style.screen_spring, snap: zoom::is_instant_edge(&zooms, s) }
            });
            let vis = self.anim.bubble_vis.values_at(f, |pb| {
                let shown = camera::kind_at(&layouts, map.to_source(pb)) == LayoutKind::Both;
                Target::new(if shown { vec![1.0, 0.0] } else { vec![0.0, 10.0] }, SpringConfig::FADE)
            });
            let r = Rect::new(b[0], b[1], b[2], b[3]).scale_about_origin(k);
            let aspect = camera::aspect(&cs, video);
            let uv = centre_crop(video.aspect(), aspect);
            if vis[0] > 0.002 {
                cam = Some(CameraLayer {
                    rect: r,
                    radius: cs.radius(r.w, r.h),
                    squircle: cs.squircle_exponent(),
                    border: cs.border_px(size.y),
                    border_color: cs.border_color.to_f32(),
                    shadow: cs.shadow,
                    alpha: vis[0].clamp(0.0, 1.0),
                    blur: vis[1].max(0.0) * k,
                    mirror: cs.mirror,
                    uv,
                });
            }
            let full = self.anim.full.values_at(f, |pb| {
                let on = camera::kind_at(&layouts, map.to_source(pb)) == LayoutKind::CameraOnly;
                Target::new(if on { vec![1.0, 0.0, 1.0] } else { vec![0.0, 30.0, 1.05] }, SpringConfig::FADE)
            });
            if full[0] > 0.002 {
                let fr = Rect::from_min_size(Vec2::ZERO, size);
                let grown = Rect::from_min_size(fr.center() - size * (full[2] / 2.0), size * full[2]);
                cam_full = Some(CameraLayer {
                    rect: grown,
                    radius: 0.0,
                    squircle: 0.0,
                    border: 0.0,
                    border_color: [0.0; 4],
                    shadow: false,
                    alpha: full[0].clamp(0.0, 1.0),
                    blur: full[1].max(0.0) * k,
                    mirror: cs.mirror,
                    uv: centre_crop(video.aspect(), size.aspect()),
                });
            }
        }
        let _ = layout;

        // Overlays.
        let mut texts = vec![];
        let base_font = (size.y * 0.05).min(size.x * 0.07);
        if style.captions.show
            && let Some(line) = captions::line_at(&self.lines, src)
        {
            let text = line.text();
            texts.push(TextLayer {
                size: fit_font(&text, base_font * style.captions.size, size.x * 0.92),
                text,
                center: v2(size.x / 2.0, size.y - base_font * 1.6),
                alpha: 1.0,
                blur: 0.0,
                badge: false,
            });
        }
        if style.shortcuts.show {
            let font = base_font * style.shortcuts.size;
            let caption_lift = if texts.is_empty() { 0.0 } else { base_font * style.captions.size * 1.8 };
            let near: Vec<usize> = (0..self.badges.len())
                .filter(|&i| {
                    let b = &self.badges[i];
                    src >= b.start - 600.0 && src <= b.end + 1500.0
                })
                .collect();
            for i in near {
                let b = self.badges[i].clone();
                let map = self.map.clone();
                let anim = self.anim.badges.entry(i).or_insert_with(|| Animator::new(self.fps));
                let lift = font * 0.6;
                let v = anim.values_at(f, |pb| {
                    let s = map.to_source(pb);
                    let vals = if s < b.start {
                        vec![0.0, lift, 6.0]
                    } else if s <= b.end {
                        vec![1.0, 0.0, 0.0]
                    } else {
                        vec![0.0, -lift, 6.0]
                    };
                    Target::new(vals, SpringConfig::BADGE)
                });
                if v[0] > 0.01 {
                    let label = if b.count > 1 { format!("{}  ×{}", b.label, b.count) } else { b.label.clone() };
                    texts.push(TextLayer {
                        text: label,
                        size: font,
                        center: v2(size.x / 2.0, size.y - font * 1.8 - caption_lift + v[1]),
                        alpha: v[0].clamp(0.0, 1.0),
                        blur: v[2] * k,
                        badge: true,
                    });
                }
            }
        }

        // Masks, clipped to the visible (cropped) screen.
        let zk = k * zoom_scale;
        let masks = self
            .project
            .timeline
            .masks
            .iter()
            .filter_map(|m| {
                let alpha = m.alpha_at(src, self.duration);
                if alpha <= 0.002 {
                    return None;
                }
                let rec_px = Rect::new(m.rect.x * rec.x, m.rect.y * rec.y, m.rect.w * rec.x, m.rect.h * rec.y).intersect(&self.sizes.crop);
                if rec_px.w <= 0.0 || rec_px.h <= 0.0 {
                    return None;
                }
                let out = Rect::from_points(self.screen_to_output(rec_px.min(), rect), self.screen_to_output(rec_px.max(), rect));
                Some(MaskLayer {
                    kind: m.kind,
                    rect: out,
                    uv: Rect::new(rec_px.x / rec.x, rec_px.y / rec.y, rec_px.w / rec.x, rec_px.h / rec.y),
                    radius: m.radius * zk,
                    amount: if m.kind == MaskKind::Highlight { m.amount } else { m.amount * zk },
                    alpha,
                    screen: screen_rect,
                    screen_radius: window.inner_radius,
                })
            })
            .collect();

        // Annotations: a dot on the anchor plus a pill label that springs in
        // like a key badge. The label sits above the anchor, flipping below
        // when it would clip the top edge.
        let annot_font = base_font * 0.55;
        let annot_ids: Vec<u64> = self
            .project
            .timeline
            .annotations
            .iter()
            .filter(|a| src >= a.start - 600.0 && src <= a.end + 1500.0)
            .map(|a| a.id)
            .collect();
        for id in annot_ids {
            let Some(a) = self.project.timeline.annotations.iter().find(|a| a.id == id) else { continue };
            let rec_pt = v2(a.pos.x * rec.x, a.pos.y * rec.y);
            if !self.sizes.crop.contains(rec_pt) {
                continue;
            }
            let anchor = self.screen_to_output(rec_pt, rect);
            let map = self.map.clone();
            let anim = self.anim.annotations.entry(id).or_insert_with(|| Animator::new(self.fps));
            let lift = annot_font * 0.6;
            let (start, end) = (a.start, a.end);
            let v = anim.values_at(f, |pb| {
                let s = map.to_source(pb);
                let vals = if s < start {
                    vec![0.0, lift, 6.0]
                } else if s <= end {
                    vec![1.0, 0.0, 0.0]
                } else {
                    vec![0.0, -lift, 6.0]
                };
                Target::new(vals, SpringConfig::BADGE)
            });
            if v[0] <= 0.01 {
                continue;
            }
            let dot_r = 4.5 * k;
            circles.push(CircleFx { pos: anchor, radius: dot_r, alpha: v[0].clamp(0.0, 1.0), blur: 0.0 });
            if !a.text.is_empty() {
                let fs = fit_font(&a.text, annot_font, size.x * 0.5);
                let half_w = (a.text.chars().count() as f64 * fs * 0.56 + fs) / 2.0;
                let cx = anchor.x.clamp(half_w, (size.x - half_w).max(half_w));
                let up = anchor.y - dot_r - fs * 1.1;
                let cy = if up - fs * 1.1 > 0.0 { up } else { anchor.y + dot_r + fs * 1.1 };
                let cy = cy.clamp(fs * 1.1, (size.y - fs * 1.1).max(fs * 1.1));
                texts.push(TextLayer {
                    text: a.text.clone(),
                    size: fs,
                    center: v2(cx, cy + v[1]),
                    alpha: v[0].clamp(0.0, 1.0),
                    blur: v[2] * k,
                    badge: true,
                });
            }
        }

        Frame {
            size,
            playback_ms: pb,
            source_ms: src,
            backdrop_blur: backdrop * k,
            window,
            cursor,
            circles,
            ripples,
            camera: cam,
            camera_full: cam_full,
            texts,
            masks,
            device: super::device::parts(style.device, rect),
            device_body: super::device::body(style.device, rect),
        }
    }
}

/// Shrink a one-line label's font so it fits `max_w` (narrow outputs).
fn fit_font(text: &str, size: f64, max_w: f64) -> f64 {
    // Inter SemiBold averages ~0.56 em per character, plus the pill padding.
    let est = text.chars().count() as f64 * size * 0.56 + size;
    if est > max_w { size * max_w / est } else { size }
}

/// Normalised centre crop that turns `from` aspect into `to` aspect.
pub fn centre_crop(from: f64, to: f64) -> Rect {
    if from > to {
        let w = to / from;
        Rect::new((1.0 - w) / 2.0, 0.0, w, 1.0)
    } else {
        let h = from / to;
        Rect::new(0.0, (1.0 - h) / 2.0, 1.0, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::model::{Zoom, ZoomMode};
    use crate::project::recording::{MouseEvent, MouseKind, RecordingMeta, VideoTrack};

    fn recording() -> Recording {
        let mut mouse = vec![];
        for i in 0..200 {
            let t = i as f64 * 50.0;
            mouse.push(MouseEvent { t, x: 100.0 + i as f64 * 5.0, y: 300.0, kind: MouseKind::Move, button: 0, cursor: 0 });
        }
        mouse.push(MouseEvent { t: 3000.0, x: 1500.0, y: 800.0, kind: MouseKind::Down, button: 0, cursor: 0 });
        mouse.push(MouseEvent { t: 3080.0, x: 1500.0, y: 800.0, kind: MouseKind::Up, button: 0, cursor: 0 });
        Recording {
            meta: RecordingMeta {
                duration_ms: 10_000.0,
                screen: VideoTrack { file: "screen.mov".into(), width: 1920, height: 1080, fps: 60.0, offset_ms: 0.0 },
                scale: 2.0,
                ..Default::default()
            },
            mouse,
            ..Default::default()
        }
    }

    #[test]
    fn frame_is_deterministic_and_zooms() {
        let rec = recording();
        let mut p = Project::default();
        p.timeline.zooms.push(Zoom { start: 2500.0, end: 6000.0, scale: 2.0, ..Default::default() });
        let mut a = Scene::new(p.clone(), &rec, 60.0);
        let before = a.frame(1000.0, 1080.0);
        let during = a.frame(5000.0, 1080.0);
        assert!(during.window.rect.w > before.window.rect.w * 1.9, "zoomed in");
        // A fresh scene seeking directly gives the same frame.
        let mut b = Scene::new(p, &rec, 60.0);
        assert_eq!(b.frame(5000.0, 1080.0).window, during.window);
    }

    #[test]
    fn zoom_transition_has_blur() {
        let rec = recording();
        let mut p = Project::default();
        p.timeline.zooms.push(Zoom { start: 2500.0, end: 6000.0, scale: 2.0, ..Default::default() });
        let mut s = Scene::new(p, &rec, 60.0);
        let f = s.frame(2600.0, 1080.0);
        assert!(f.window.blur.zoom > 0.0 || f.window.blur.velocity.length() > 0.0);
    }

    #[test]
    fn cursor_inside_window() {
        let rec = recording();
        let mut s = Scene::new(Project::default(), &rec, 60.0);
        let f = s.frame(1000.0, 1080.0);
        let c = f.cursor.unwrap();
        assert!(f.window.rect.contains(c.pos), "{:?} in {:?}", c.pos, f.window.rect);
    }

    #[test]
    fn annotation_shows_dot_and_label_over_its_range() {
        use crate::project::model::Annotation;
        let rec = recording();
        let mut p = Project::default();
        p.timeline.annotations.push(Annotation { id: 7, start: 2000.0, end: 6000.0, pos: v2(0.3, 0.5), text: "Click here".into() });
        let mut s = Scene::new(p, &rec, 60.0);

        let f = s.frame(4500.0, 1080.0);
        let anchor = s.screen_to_output(v2(0.3 * 1920.0, 0.5 * 1080.0), f.window.rect);
        let dot = f.circles.iter().find(|c| (c.pos - anchor).length() < 1.0).expect("anchor dot at the note position");
        assert!(dot.alpha > 0.9, "{dot:?}");
        let label = f.texts.iter().find(|t| t.text == "Click here").expect("note label");
        assert!(label.badge, "notes draw as pills");
        assert!(label.alpha > 0.9, "{label:?}");
        assert!(label.center.y < anchor.y, "label above the anchor");

        let after = s.frame(8000.0, 1080.0);
        assert!(!after.texts.iter().any(|t| t.text == "Click here"), "label gone after the range");
        assert!(!after.circles.iter().any(|c| (c.pos - anchor).length() < 1.0), "dot gone after the range");
    }

    #[test]
    fn annotation_label_flips_below_near_the_top() {
        use crate::project::model::Annotation;
        let rec = recording();
        let mut p = Project::default();
        // A zoom focused near the bottom pushes a top anchor above the output
        // top, so the label has to sit below the dot instead.
        p.timeline.zooms.push(Zoom { id: 9, start: 1000.0, end: 9000.0, scale: 3.0, mode: ZoomMode::Manual { target: v2(0.5, 0.95) }, ..Default::default() });
        p.timeline.annotations.push(Annotation { id: 7, start: 1000.0, end: 9000.0, pos: v2(0.5, 0.02), text: "Top edge".into() });
        let mut s = Scene::new(p, &rec, 60.0);
        let f = s.frame(4000.0, 1080.0);
        let anchor = s.screen_to_output(v2(0.5 * 1920.0, 0.02 * 1080.0), f.window.rect);
        assert!(anchor.y < 0.0, "anchor pushed above the viewport by the zoom");
        let label = f.texts.iter().find(|t| t.text == "Top edge").expect("note label");
        assert!(label.center.y > anchor.y, "label below the anchor when the top would clip");
    }

    #[test]
    fn annotation_without_text_shows_only_the_dot() {
        use crate::project::model::Annotation;
        let rec = recording();
        let mut p = Project::default();
        p.timeline.annotations.push(Annotation { id: 7, start: 0.0, end: 10_000.0, pos: v2(0.5, 0.5), text: String::new() });
        let mut s = Scene::new(p, &rec, 60.0);
        let f = s.frame(4000.0, 1080.0);
        let anchor = s.screen_to_output(v2(0.5 * 1920.0, 0.5 * 1080.0), f.window.rect);
        assert!(f.circles.iter().any(|c| (c.pos - anchor).length() < 1.0), "dot present");
        assert!(f.texts.is_empty(), "no empty pill");
    }
}
