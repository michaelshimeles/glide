//! Damped springs. Every animated value in a project (zoom, pan, cursor,
//! camera, overlays) moves toward its target like a mass on a spring, which
//! is what makes motion feel physical rather than eased.
//!
//! Integration is semi-implicit Euler in 1 ms substeps, so the result does
//! not depend on the frame rate it is sampled at.

use serde::{Deserialize, Serialize};

/// Physical parameters of a spring.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpringConfig {
    pub stiffness: f64,
    pub damping: f64,
    /// Zero means "no animation": the value jumps to its target.
    pub mass: f64,
}

impl SpringConfig {
    pub const fn new(stiffness: f64, damping: f64, mass: f64) -> Self {
        Self { stiffness, damping, mass }
    }

    /// Screen pan/zoom: settles quickly so content stays readable.
    pub const SCREEN_FOCUSED: Self = Self::new(200.0, 40.0, 2.25);
    /// Screen pan/zoom: slower, more fluid.
    pub const SCREEN_SMOOTH: Self = Self::new(170.0, 50.0, 3.0);
    /// Cursor daisycap presets.
    pub const CURSOR_SMOOTH: Self = Self::new(470.0, 70.0, 3.0);
    pub const CURSOR_MEDIUM: Self = Self::new(340.0, 60.0, 3.0);
    pub const CURSOR_RAPID: Self = Self::new(530.0, 40.0, 1.0);
    /// Cursor right before a click: arrives quickly on the click point.
    pub const NEAR_CLICK: Self = Self::new(530.0, 40.0, 1.0);
    /// Cursor while dragging: follows almost exactly.
    pub const DRAG: Self = Self::new(1000.0, 40.0, 1.0);
    /// Press/fade feedback on the cursor.
    pub const PRESS: Self = Self::new(300.0, 30.0, 0.3);
    /// Click ripple/circle smoothing.
    pub const CLICK: Self = Self::new(500.0, 30.0, 0.3);
    /// Show/hide transitions (camera bubble, layouts).
    pub const FADE: Self = Self::new(530.0, 40.0, 1.0);
    /// Rotation and other secondary motion.
    pub const SOFT: Self = Self::new(340.0, 60.0, 3.0);
    /// Keyboard shortcut badges.
    pub const BADGE: Self = Self::new(470.0, 30.0, 1.0);
    /// No animation.
    pub const INSTANT: Self = Self::new(1.0, 1.0, 0.0);

    pub fn is_instant(&self) -> bool {
        self.mass <= 0.0
    }

    /// Time until a unit step settles, in ms (cached by callers).
    pub fn settle_time_ms(&self) -> f64 {
        if self.is_instant() {
            return 0.0;
        }
        let mut s = NumberSpring::new(0.0, *self);
        s.set_target(1000.0);
        let step = 1000.0 / 60.0;
        let mut t = 0.0;
        while !s.is_at_rest() {
            s.step_raw(step);
            t += step;
            if t > MAX_SETTLE_MS || s.value.is_nan() {
                break;
            }
        }
        t
    }
}

impl Default for SpringConfig {
    fn default() -> Self {
        Self::new(90.0, 9.0, 1.0)
    }
}

const PRECISION: f64 = 0.002;
const MAX_SETTLE_MS: f64 = 20_000.0;

/// A single animated number.
#[derive(Clone, Debug)]
pub struct NumberSpring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    pub config: SpringConfig,
    /// Rest threshold, relative to the size of the last jump.
    threshold: f64,
    settle_ms: Option<f64>,
}

impl NumberSpring {
    pub fn new(value: f64, config: SpringConfig) -> Self {
        Self { value, velocity: 0.0, target: value, config, threshold: PRECISION, settle_ms: None }
    }

    pub fn is_at_rest(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    pub fn set_target(&mut self, target: f64) {
        if target != self.target && target.is_finite() {
            self.threshold = (self.value - target).abs().max(1.0) * PRECISION;
            self.target = target;
        }
    }

    pub fn snap(&mut self, value: f64) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn snap_to_target(&mut self) {
        self.snap(self.target);
    }

    pub fn set_config(&mut self, config: SpringConfig) {
        if config != self.config {
            self.config = config;
            self.settle_ms = None;
        }
    }

    /// Advance by `dt` milliseconds.
    pub fn advance(&mut self, dt: f64) {
        if dt <= 0.0 || self.is_at_rest() {
            return;
        }
        if self.config.is_instant() {
            self.snap_to_target();
            return;
        }
        let settle = *self.settle_ms.get_or_insert_with(|| self.config.settle_time_ms());
        if dt >= settle {
            self.snap_to_target();
            return;
        }
        let whole = dt.floor() as usize;
        for _ in 0..whole {
            self.step_raw(1.0);
            if self.is_at_rest() {
                return;
            }
        }
        let rest = dt - whole as f64;
        if rest > 1e-9 {
            self.step_raw(rest);
        }
    }

    /// One integration step of `dt` ms.
    fn step_raw(&mut self, dt: f64) {
        let SpringConfig { stiffness, damping, mass } = self.config;
        let h = dt / 1000.0;
        let force = -(self.value - self.target) * stiffness - self.velocity * damping;
        let v = self.velocity + force / mass * h;
        let x = self.value + v * h;
        if (v.abs() < self.threshold) && ((x - self.target).abs() < self.threshold) {
            self.snap_to_target();
        } else {
            self.value = x;
            self.velocity = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaches_target_and_rests() {
        let mut s = NumberSpring::new(0.0, SpringConfig::SCREEN_FOCUSED);
        s.set_target(100.0);
        for _ in 0..600 {
            s.advance(1000.0 / 60.0);
        }
        assert!(s.is_at_rest());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn frame_rate_independent() {
        let run = |fps: f64| {
            let mut s = NumberSpring::new(0.0, SpringConfig::CURSOR_SMOOTH);
            s.set_target(500.0);
            let mut t = 0.0;
            while t < 250.0 {
                s.advance(1000.0 / fps);
                t += 1000.0 / fps;
            }
            s.value
        };
        // 250 ms sampled at 60 and 120 fps lands within a pixel.
        let (a, b) = (run(60.0), run(120.0));
        assert!((a - b).abs() < 25.0, "{a} vs {b}");
    }

    #[test]
    fn instant_snaps() {
        let mut s = NumberSpring::new(0.0, SpringConfig::INSTANT);
        s.set_target(42.0);
        s.advance(1.0);
        assert_eq!(s.value, 42.0);
    }

    #[test]
    fn underdamped_overshoots_overdamped_does_not() {
        let peak = |c: SpringConfig| {
            let mut s = NumberSpring::new(0.0, c);
            s.set_target(1.0);
            let mut m: f64 = 0.0;
            for _ in 0..2000 {
                s.advance(1.0);
                m = m.max(s.value);
            }
            m
        };
        assert!(peak(SpringConfig::new(1000.0, 5.0, 1.0)) > 1.05);
        assert!(peak(SpringConfig::SCREEN_SMOOTH) <= 1.0 + 1e-9);
    }

    #[test]
    fn settle_time_is_bounded() {
        let t = SpringConfig::SCREEN_FOCUSED.settle_time_ms();
        assert!(t > 100.0 && t < 3000.0, "{t}");
    }
}
