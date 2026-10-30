//! Resize another app's window before recording it (Accessibility API).
//! Needs the Accessibility permission; the system asks the first time.

use std::ffi::c_void;

use anyhow::{Context, Result, bail};
use objc2_core_foundation::{CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGDisplayBounds;

use super::screen;

type AxRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: AxRef) -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AxRef;
    fn AXUIElementCopyAttributeValue(element: AxRef, attribute: AxRef, value: *mut AxRef) -> i32;
    fn AXUIElementSetAttributeValue(element: AxRef, attribute: AxRef, value: AxRef) -> i32;
    fn AXUIElementPerformAction(element: AxRef, action: AxRef) -> i32;
    fn AXValueCreate(kind: u32, value: *const c_void) -> AxRef;
    fn AXValueGetValue(value: AxRef, kind: u32, out: *mut c_void) -> bool;
    /// Private but long-standing: the CGWindowID behind an AX window.
    fn _AXUIElementGetWindow(element: AxRef, id: *mut u32) -> i32;
    static kAXTrustedCheckOptionPrompt: AxRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: AxRef);
    fn CFRetain(cf: AxRef) -> AxRef;
    fn CFArrayGetCount(array: AxRef) -> isize;
    fn CFArrayGetValueAtIndex(array: AxRef, index: isize) -> AxRef;
    fn CFDictionaryCreate(
        allocator: AxRef,
        keys: *const AxRef,
        values: *const AxRef,
        count: isize,
        key_callbacks: AxRef,
        value_callbacks: AxRef,
    ) -> AxRef;
    static kCFBooleanTrue: AxRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

const AX_POINT: u32 = 1;
const AX_SIZE: u32 = 2;

/// Common recording sizes (points), grouped like a size menu.
pub const PRESETS: &[(&str, &[(u32, u32)])] = &[
    ("16:9", &[(1280, 720), (1920, 1080), (2560, 1440)]),
    ("16:10", &[(1280, 800), (1440, 900), (1680, 1050), (1920, 1200)]),
    ("4:3", &[(800, 600), (1024, 768), (1280, 960), (1600, 1200)]),
    ("9:16", &[(720, 1280), (900, 1600), (1080, 1920)]),
    ("Square", &[(800, 800), (1080, 1080), (1280, 1280)]),
];

/// Is DaisyCap allowed to control other apps' windows? With `prompt`, macOS
/// shows its permission dialog when it isn't.
pub fn trusted(prompt: bool) -> bool {
    // SAFETY: plain CF/AX calls; the options dictionary is released after use.
    unsafe {
        if !prompt {
            return AXIsProcessTrusted();
        }
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let opts = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        );
        let ok = AXIsProcessTrustedWithOptions(opts);
        CFRelease(opts);
        ok
    }
}

/// An owned AX/CF object.
struct Owned(AxRef);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we hold one reference from a Create/Copy call.
            unsafe { CFRelease(self.0) };
        }
    }
}

fn attr(name: &str) -> CFRetained<CFString> {
    CFString::from_str(name)
}

fn copy_attr(el: AxRef, name: &str) -> Option<Owned> {
    let mut v: AxRef = std::ptr::null();
    // SAFETY: el is a live AXUIElement; on success v is +1.
    let err = unsafe { AXUIElementCopyAttributeValue(el, CFRetained::as_ptr(&attr(name)).as_ptr() as AxRef, &mut v) };
    (err == 0 && !v.is_null()).then_some(Owned(v))
}

fn get_rect(el: AxRef) -> Option<CGRect> {
    let (pos, size) = (copy_attr(el, "AXPosition")?, copy_attr(el, "AXSize")?);
    let mut r = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
    // SAFETY: AXPosition/AXSize hold AXValues of these kinds.
    unsafe {
        if !AXValueGetValue(pos.0, AX_POINT, &mut r.origin as *mut CGPoint as *mut c_void)
            || !AXValueGetValue(size.0, AX_SIZE, &mut r.size as *mut CGSize as *mut c_void)
        {
            return None;
        }
    }
    Some(r)
}

fn set_attr<T>(el: AxRef, name: &str, kind: u32, v: &T) -> i32 {
    // SAFETY: the AXValue is created from a value of the matching kind and
    // released after the call.
    unsafe {
        let value = Owned(AXValueCreate(kind, v as *const T as *const c_void));
        AXUIElementSetAttributeValue(el, CFRetained::as_ptr(&attr(name)).as_ptr() as AxRef, value.0)
    }
}

/// The AX element of the window with CGWindowID `id` owned by `pid`.
/// Falls back to matching the frame when the window ID isn't available.
fn find_ax_window(pid: i32, id: u32, frame: CGRect) -> Option<(Owned, Owned)> {
    // SAFETY: AXUIElementCreateApplication returns +1.
    let app = Owned(unsafe { AXUIElementCreateApplication(pid) });
    let list = copy_attr(app.0, "AXWindows")?;
    let mut by_frame = None;
    // SAFETY: AXWindows is a CFArray of AXUIElements kept alive by `list`;
    // each element we keep is retained.
    for i in 0..unsafe { CFArrayGetCount(list.0) } {
        let w = Owned(unsafe { CFRetain(CFArrayGetValueAtIndex(list.0, i)) });
        let mut wid = 0u32;
        // SAFETY: w is a live AXUIElement.
        if unsafe { _AXUIElementGetWindow(w.0, &mut wid) } == 0 && wid == id {
            return Some((app, w));
        }
        if by_frame.is_none()
            && let Some(r) = get_rect(w.0)
            && (r.origin.x - frame.origin.x).abs() < 2.0
            && (r.origin.y - frame.origin.y).abs() < 2.0
            && (r.size.width - frame.size.width).abs() < 2.0
            && (r.size.height - frame.size.height).abs() < 2.0
        {
            by_frame = Some(w);
        }
    }
    by_frame.map(|w| (app, w))
}

/// Bounds (points, top-left origin) of the display under the centre of `r`.
fn display_in(bounds: &[CGRect], r: CGRect) -> Option<CGRect> {
    let c = (r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0);
    bounds
        .iter()
        .find(|b| c.0 >= b.origin.x && c.0 < b.origin.x + b.size.width && c.1 >= b.origin.y && c.1 < b.origin.y + b.size.height)
        .or(bounds.first())
        .copied()
}

fn display_for(r: CGRect) -> Option<CGRect> {
    display_in(&display_bounds(), r)
}

/// Bounds of the active displays, straight from CoreGraphics (no
/// ScreenCaptureKit round trip, which can stall for seconds).
fn display_bounds() -> Vec<CGRect> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    }
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: the buffer holds 16 ids; n says how many were filled.
    if unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) } != 0 {
        return vec![];
    }
    ids[..n as usize].iter().map(|&id| CGDisplayBounds(id)).collect()
}

/// The frontmost normal on-screen window of process `pid`, from the
/// CoreGraphics window list.
pub fn window_of_pid(pid: i32) -> Option<u32> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> AxRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryGetValue(dict: AxRef, key: AxRef) -> AxRef;
        fn CFNumberGetValue(number: AxRef, kind: isize, out: *mut c_void) -> bool;
    }
    const ON_SCREEN_ONLY: u32 = 1;
    const SINT32: isize = 3;
    // SAFETY: a +1 array of dictionaries; values are borrowed while it lives,
    // and the key strings outlive the lookups.
    unsafe {
        let list = Owned(CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0));
        if list.0.is_null() {
            return None;
        }
        let keys = [attr("kCGWindowOwnerPID"), attr("kCGWindowLayer"), attr("kCGWindowNumber")];
        let int = |info: AxRef, k: usize| {
            let v = CFDictionaryGetValue(info, CFRetained::as_ptr(&keys[k]).as_ptr() as AxRef);
            let mut n = 0i32;
            (!v.is_null() && CFNumberGetValue(v, SINT32, &mut n as *mut i32 as *mut c_void)).then_some(n)
        };
        (0..CFArrayGetCount(list.0))
            .map(|i| CFArrayGetValueAtIndex(list.0, i))
            .find(|&info| int(info, 0) == Some(pid) && int(info, 1) == Some(0))
            .and_then(|info| int(info, 2))
            .map(|n| n as u32)
    }
}

/// Size (points) of window `id`, from the CoreGraphics window list.
pub fn window_size(id: u32) -> Result<(f64, f64)> {
    owner_and_frame(id).map(|(_, f)| (f.size.width, f.size.height))
}

/// Owning process and frame (points, top-left origin) of window `id`, from
/// the CoreGraphics window list.
fn owner_and_frame(id: u32) -> Result<(i32, CGRect)> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> AxRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: AxRef, rect: *mut CGRect) -> bool;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryGetValue(dict: AxRef, key: AxRef) -> AxRef;
        fn CFNumberGetValue(number: AxRef, kind: isize, out: *mut c_void) -> bool;
    }
    const INCLUDING_WINDOW: u32 = 1 << 3;
    const SINT32: isize = 3;
    // SAFETY: CGWindowListCopyWindowInfo returns a +1 array of dictionaries;
    // values read from them are borrowed while `list` is alive.
    unsafe {
        let list = Owned(CGWindowListCopyWindowInfo(INCLUDING_WINDOW, id));
        if list.0.is_null() || CFArrayGetCount(list.0) == 0 {
            bail!("that window is no longer on screen");
        }
        let info = CFArrayGetValueAtIndex(list.0, 0);
        // The key strings must outlive the lookups.
        let (pid_key, bounds_key) = (attr("kCGWindowOwnerPID"), attr("kCGWindowBounds"));
        let pid_ref = CFDictionaryGetValue(info, CFRetained::as_ptr(&pid_key).as_ptr() as AxRef);
        let bounds_ref = CFDictionaryGetValue(info, CFRetained::as_ptr(&bounds_key).as_ptr() as AxRef);
        let mut pid = 0i32;
        if pid_ref.is_null() || !CFNumberGetValue(pid_ref, SINT32, &mut pid as *mut i32 as *mut c_void) {
            bail!("window has no owning app");
        }
        let mut frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0));
        if bounds_ref.is_null() || !CGRectMakeWithDictionaryRepresentation(bounds_ref, &mut frame) {
            bail!("couldn't read the window's frame");
        }
        Ok((pid, frame))
    }
}

/// Size (points) of the display showing each window, to rule out sizes
/// that wouldn't fit.
pub fn display_sizes(windows: &[screen::WindowInfo]) -> Vec<(f64, f64)> {
    let bounds: Vec<CGRect> = screen::displays().map(|ds| ds.iter().map(|d| CGDisplayBounds(d.id)).collect()).unwrap_or_default();
    windows
        .iter()
        .map(|w| {
            // SAFETY: SCWindow accessor.
            let r = unsafe { w.window.frame() };
            display_in(&bounds, r).map_or((f64::MAX, f64::MAX), |d| (d.size.width, d.size.height))
        })
        .collect()
}

/// On-screen window IDs, frontmost first.
pub fn front_to_back() -> Vec<u32> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreate(option: u32, relative_to: u32) -> AxRef;
    }
    const ON_SCREEN_ONLY: u32 = 1;
    // SAFETY: CGWindowListCreate returns a +1 CFArray whose values are
    // CGWindowIDs stored directly in the pointer slots.
    unsafe {
        let list = Owned(CGWindowListCreate(ON_SCREEN_ONLY, 0));
        if list.0.is_null() {
            return vec![];
        }
        (0..CFArrayGetCount(list.0)).map(|i| CFArrayGetValueAtIndex(list.0, i) as usize as u32).collect()
    }
}

/// Bring window `id` and its app to the front.
pub fn raise(id: u32) -> Result<()> {
    if !trusted(true) {
        bail!("DaisyCap needs Accessibility access to bring windows to the front");
    }
    let (pid, frame) = owner_and_frame(id)?;
    // SAFETY: plain AX calls on elements we hold.
    unsafe {
        let (app, win) = find_ax_window(pid, id, frame).context("couldn't find that window through Accessibility")?;
        AXUIElementPerformAction(win.0, CFRetained::as_ptr(&attr("AXRaise")).as_ptr() as AxRef);
        AXUIElementSetAttributeValue(app.0, CFRetained::as_ptr(&attr("AXFrontmost")).as_ptr() as AxRef, kCFBooleanTrue);
    }
    Ok(())
}

/// Resize window `id` to `width`×`height` points, moving it if needed so it
/// stays on its display, and bring it to the front. Returns the size the
/// app actually accepted (apps can enforce minimum/maximum sizes).
pub fn resize(id: u32, width: f64, height: f64) -> Result<(f64, f64)> {
    if !trusted(true) {
        bail!("DaisyCap needs Accessibility access to resize windows (System Settings › Privacy & Security › Accessibility)");
    }
    let (pid, frame) = owner_and_frame(id)?;
    let (_app, win) = find_ax_window(pid, id, frame).context("couldn't find that window through Accessibility")?;
    let current = get_rect(win.0).unwrap_or(frame);
    let mut origin = current.origin;
    if let Some(d) = display_for(current) {
        // Keep it on its display (below the menu bar).
        let (min_x, min_y) = (d.origin.x, d.origin.y + 25.0);
        let (max_x, max_y) = (d.origin.x + d.size.width - width, d.origin.y + d.size.height - height);
        origin.x = origin.x.min(max_x).max(min_x);
        origin.y = origin.y.min(max_y).max(min_y);
    }
    if origin.x != current.origin.x || origin.y != current.origin.y {
        set_attr(win.0, "AXPosition", AX_POINT, &origin);
    }
    let err = set_attr(win.0, "AXSize", AX_SIZE, &CGSize::new(width, height));
    if err != 0 {
        bail!("the app refused the new size (AX error {err})");
    }
    // SAFETY: raising is a plain AX action on a live element.
    unsafe { AXUIElementPerformAction(win.0, CFRetained::as_ptr(&attr("AXRaise")).as_ptr() as AxRef) };
    let got = get_rect(win.0).map_or((width, height), |r| (r.size.width, r.size.height));
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_ordered_and_match_their_ratio() {
        for (name, sizes) in PRESETS {
            assert!(sizes.windows(2).all(|w| w[0].0 < w[1].0), "{name}");
            for &(w, h) in *sizes {
                let r = w as f64 / h as f64;
                let want = match *name {
                    "16:9" => 16.0 / 9.0,
                    "16:10" => 1.6,
                    "4:3" => 4.0 / 3.0,
                    "9:16" => 9.0 / 16.0,
                    _ => 1.0,
                };
                assert!((r - want).abs() < 1e-3, "{name} {w}x{h}");
            }
        }
    }
}
