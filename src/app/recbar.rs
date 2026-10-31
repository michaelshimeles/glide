//! The floating recording control bar: a borderless, non-activating panel
//! (clicking it never brings Glide forward, so the menu bar being recorded
//! doesn't change) shaped as a pill with a vibrancy background, a pulsing
//! dot, a monospaced timer and SF Symbol buttons. Native AppKit, so it
//! looks and behaves like a system control; it's left out of recordings.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_foundation::*;

use super::Cmd;
use crate::util::ns;

/// Bar size in points.
const W: CGFloat = 286.0;
const H: CGFloat = 44.0;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GlideBarTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(cancel:))]
        fn cancel(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::Cancel);
        }
        #[unsafe(method(restart:))]
        fn restart(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::Restart);
        }
        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::TogglePause);
        }
        #[unsafe(method(stop:))]
        fn stop(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::ToggleRecording);
        }
    }
);

define_class!(
    /// The pill background: blurs what's behind it and drags the panel.
    #[unsafe(super(NSVisualEffectView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GlideBarBackground"]
    pub struct Background;

    impl Background {
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            true
        }
    }
);

/// What the bar shows.
#[derive(Clone, PartialEq)]
pub enum State {
    /// Seconds left before capture starts.
    Countdown(u64),
    /// The old take is being removed before a new one starts.
    Restarting,
    Recording { clock: String, paused: bool },
}

pub struct RecordBar {
    panel: Retained<NSPanel>,
    dot: Retained<NSView>,
    timer: Retained<NSTextField>,
    pause: Retained<NSButton>,
    restart: Retained<NSButton>,
    stop: Retained<NSButton>,
    _target: Retained<Target>,
    last: Option<State>,
}

fn symbol(name: &str, desc: &str, size: CGFloat) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(desc)))?;
    // SAFETY: plain AppKit constant.
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe { NSFontWeightSemibold });
    img.imageWithSymbolConfiguration(&config)
}

/// A borderless SF Symbol button that highlights on hover.
fn icon_button(name: &str, tip: &str, action: Sel, target: &Target, frame: CGRect, mtm: MainThreadMarker) -> Retained<NSButton> {
    let img = symbol(name, tip, 14.0).unwrap_or_default();
    // SAFETY: the target outlives the button (both owned by the bar).
    let b = unsafe { NSButton::buttonWithImage_target_action(&img, Some(target), Some(action), mtm) };
    b.setBezelStyle(NSBezelStyle::AccessoryBarAction);
    b.setShowsBorderOnlyWhileMouseInside(true);
    b.setContentTintColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.92)));
    b.setToolTip(Some(&ns(tip)));
    b.setFrame(frame);
    b
}

fn cg(c: &NSColor) -> impl std::ops::Deref<Target = objc2_core_graphics::CGColor> {
    c.CGColor()
}

fn rect(x: CGFloat, y: CGFloat, w: CGFloat, h: CGFloat) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}

/// Pulse a layer's opacity (the recording dot).
fn pulse(layer: &AnyObject, on: bool) {
    // SAFETY: CALayer/CABasicAnimation messages with matching types.
    unsafe {
        let key = ns("pulse");
        // Light mode: a steady dot (an endless animation keeps the
        // compositor busy for the whole recording).
        if !on || crate::hw::light() {
            let _: () = msg_send![layer, removeAnimationForKey: &*key];
            return;
        }
        let Some(cls) = AnyClass::get(c"CABasicAnimation") else { return };
        let anim: Retained<AnyObject> = msg_send![cls, animationWithKeyPath: &*ns("opacity")];
        let _: () = msg_send![&*anim, setFromValue: &*NSNumber::new_f64(1.0)];
        let _: () = msg_send![&*anim, setToValue: &*NSNumber::new_f64(0.3)];
        let _: () = msg_send![&*anim, setDuration: 0.85f64];
        let _: () = msg_send![&*anim, setAutoreverses: true];
        let _: () = msg_send![&*anim, setRepeatCount: f32::INFINITY];
        let _: () = msg_send![layer, addAnimation: &*anim, forKey: &*key];
    }
}

impl RecordBar {
    /// Create the bar near the bottom of `display` (CoreGraphics id; 0 = main),
    /// or at `origin` (bottom-left, screen points) where the user left it.
    /// `stop_hint` is the stop shortcut for its tooltip.
    pub fn new(mtm: MainThreadMarker, display: u32, origin: Option<CGPoint>, stop_hint: &str, pause_hint: &str) -> RecordBar {
        let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
        let origin = origin.unwrap_or_else(|| default_origin(mtm, display));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), rect(origin.x, origin.y, W, H), style, NSBackingStoreType::Buffered, false);
        // SAFETY: plain configuration of a panel we own.
        unsafe {
            panel.setReleasedWhenClosed(false);
            panel.setFloatingPanel(true);
            panel.setBecomesKeyOnlyIfNeeded(true);
            panel.setHidesOnDeactivate(false);
            panel.setLevel(NSStatusWindowLevel);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(true);
            panel.setMovableByWindowBackground(true);
            // Never in screenshots or recordings (the recorder also leaves out
            // Glide's own windows). The selftest's screenshots need it visible.
            if std::env::var_os("GLIDE_SELFTEST_SHOTS").is_none() {
                panel.setSharingType(NSWindowSharingType::None);
            }
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary
                    | NSWindowCollectionBehavior::IgnoresCycle,
            );
            if let Some(dark) = NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) {
                panel.setAppearance(Some(&dark));
            }
        }

        // The pill: blur behind, a faint tint, a hairline border.
        let bg: Retained<Background> = unsafe { msg_send![Background::alloc(mtm), initWithFrame: rect(0.0, 0.0, W, H)] };
        bg.setMaterial(NSVisualEffectMaterial::HUDWindow);
        bg.setWantsLayer(true);
        if crate::hw::light() {
            // A plain dark pill: blurring what's behind it means
            // re-compositing it whenever the screen changes.
            bg.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
            bg.setState(NSVisualEffectState::Inactive);
            if let Some(l) = bg.layer() {
                l.setBackgroundColor(Some(&cg(&NSColor::colorWithWhite_alpha(0.12, 0.96))));
            }
        } else {
            bg.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            bg.setState(NSVisualEffectState::Active);
        }
        if let Some(l) = bg.layer() {
            l.setCornerRadius(H / 2.0);
            l.setMasksToBounds(true);
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&cg(&NSColor::colorWithWhite_alpha(1.0, 0.14))));
        }
        panel.setContentView(Some(&bg));

        // Pulsing dot and the timer.
        let dot = NSView::initWithFrame(NSView::alloc(mtm), rect(18.0, (H - 10.0) / 2.0, 10.0, 10.0));
        dot.setWantsLayer(true);
        if let Some(l) = dot.layer() {
            l.setCornerRadius(5.0);
            l.setBackgroundColor(Some(&cg(&NSColor::systemRedColor())));
        }
        bg.addSubview(&dot);
        let timer = NSTextField::labelWithString(&ns("00:00"), mtm);
        // SAFETY: plain AppKit constant.
        timer.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(15.0, unsafe { NSFontWeightSemibold })));
        timer.setTextColor(Some(&NSColor::whiteColor()));
        timer.setFrame(rect(36.0, (H - 20.0) / 2.0, 82.0, 20.0));
        bg.addSubview(&timer);

        let sep = |x: CGFloat| {
            let v = NSView::initWithFrame(NSView::alloc(mtm), rect(x, (H - 20.0) / 2.0, 1.0, 20.0));
            v.setWantsLayer(true);
            if let Some(l) = v.layer() {
                l.setBackgroundColor(Some(&cg(&NSColor::colorWithWhite_alpha(1.0, 0.18))));
            }
            bg.addSubview(&v);
        };
        sep(124.0);
        let y = (H - 30.0) / 2.0;
        let cancel = icon_button("xmark", "Cancel — discard this recording", sel!(cancel:), &target, rect(132.0, y, 30.0, 30.0), mtm);
        let restart = icon_button("arrow.counterclockwise", "Restart — discard and start over", sel!(restart:), &target, rect(164.0, y, 30.0, 30.0), mtm);
        let pause = icon_button("pause.fill", &format!("Pause  {pause_hint}"), sel!(togglePause:), &target, rect(196.0, y, 30.0, 30.0), mtm);
        for b in [&cancel, &restart, &pause] {
            bg.addSubview(b);
        }
        sep(234.0);

        // Stop: a red disc with a white square.
        let img = symbol("stop.fill", "Stop", 12.0).unwrap_or_default();
        let stop = unsafe { NSButton::buttonWithImage_target_action(&img, Some(&target), Some(sel!(stop:)), mtm) };
        stop.setBordered(false);
        stop.setContentTintColor(Some(&NSColor::whiteColor()));
        stop.setToolTip(Some(&ns(&format!("Stop and edit  {stop_hint}"))));
        stop.setFrame(rect(244.0, (H - 30.0) / 2.0, 30.0, 30.0));
        stop.setWantsLayer(true);
        if let Some(l) = stop.layer() {
            l.setCornerRadius(15.0);
            l.setBackgroundColor(Some(&cg(&NSColor::systemRedColor())));
        }
        bg.addSubview(&stop);

        panel.orderFrontRegardless();
        RecordBar { panel, dot, timer, pause, restart, stop, _target: target, last: None }
    }

    /// Show the countdown or the timer and pause state (cheap when unchanged).
    pub fn update(&mut self, state: State) {
        if self.last.as_ref() == Some(&state) {
            return;
        }
        let was = self.last.replace(state.clone());
        let Some(dot) = self.dot.layer() else { return };
        match state {
            State::Countdown(n) => {
                self.timer.setStringValue(&ns(&format!("Starts in {n}")));
                self.timer.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightSemibold })));
                dot.setBackgroundColor(Some(&cg(&NSColor::systemGrayColor())));
                pulse(&dot, false);
                for b in [&self.pause, &self.restart, &self.stop] {
                    b.setEnabled(false);
                }
            }
            State::Restarting => {
                self.timer.setStringValue(&ns("Restarting…"));
                self.timer.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightSemibold })));
                dot.setBackgroundColor(Some(&cg(&NSColor::systemGrayColor())));
                pulse(&dot, false);
                for b in [&self.pause, &self.restart, &self.stop] {
                    b.setEnabled(false);
                }
            }
            State::Recording { clock, paused } => {
                self.timer.setStringValue(&ns(&clock));
                let first = !matches!(was, Some(State::Recording { .. }));
                let was_paused = matches!(was, Some(State::Recording { paused: true, .. }));
                if first {
                    self.timer.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(15.0, unsafe { NSFontWeightSemibold })));
                    for b in [&self.pause, &self.restart, &self.stop] {
                        b.setEnabled(true);
                    }
                }
                if first || paused != was_paused {
                    let (color, name, tip) = if paused {
                        (NSColor::systemOrangeColor(), "play.fill", "Resume")
                    } else {
                        (NSColor::systemRedColor(), "pause.fill", "Pause")
                    };
                    dot.setBackgroundColor(Some(&cg(&color)));
                    pulse(&dot, !paused);
                    if let Some(img) = symbol(name, tip, 14.0) {
                        self.pause.setImage(Some(&img));
                    }
                    let hint = self.pause.toolTip().map(|t| t.to_string()).unwrap_or_default();
                    let shortcut = hint.split("  ").nth(1).unwrap_or("").to_string();
                    self.pause.setToolTip(Some(&ns(&format!("{tip}  {shortcut}"))));
                }
            }
        }
    }

    /// Bottom-left corner in screen points (to put the next bar where this one was).
    pub fn origin(&self) -> CGPoint {
        self.panel.frame().origin
    }

    /// The panel's window number (for tests and capture exclusion checks).
    pub fn window_number(&self) -> isize {
        self.panel.windowNumber()
    }

    /// Can the bar be dragged by its background?
    pub fn draggable(&self) -> bool {
        let bg_moves: bool = self.panel.contentView().is_some_and(|v| unsafe { msg_send![&*v, mouseDownCanMoveWindow] });
        self.panel.isMovableByWindowBackground() && bg_moves
    }
}

impl Drop for RecordBar {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        self.panel.close();
    }
}

/// Centred near the bottom of the display, above the Dock.
fn default_origin(mtm: MainThreadMarker, display: u32) -> CGPoint {
    let screens = NSScreen::screens(mtm);
    let id_of = |s: &NSScreen| -> Option<u32> {
        let n = s.deviceDescription().objectForKey(&ns("NSScreenNumber"))?;
        n.downcast::<NSNumber>().ok().map(|n| n.unsignedIntValue())
    };
    // Display 0 is the primary display (the first screen; NSScreen's
    // mainScreen is whichever has the key window).
    let screen = screens.iter().find(|s| display != 0 && id_of(s) == Some(display)).or_else(|| screens.iter().next());
    let f = screen.map_or(rect(0.0, 0.0, 1440.0, 900.0), |s| s.visibleFrame());
    CGPoint::new(f.origin.x + (f.size.width - W) / 2.0, f.origin.y + 28.0)
}
