//! What this Mac's video hardware can take, so recording stays on it.
//!
//! Older (Intel) Macs have a hardware encoder that stops at 4096×2304: a 5K
//! display recorded at its native 5120×2880 falls back to the software
//! encoder, which can't keep up and stalls the recording. "Light mode"
//! (Intel Macs by default) also records and previews at fewer pixels.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use objc2_core_foundation::{CFDictionary, CFString};

use crate::util::{Dict, cf_ns, dict, fourcc, num_b};

pub const H264: u32 = fourcc(b"avc1");
pub const HEVC: u32 = fourcc(b"hvc1");

/// Longest edge recorded in light mode: a 5K display at its point size
/// (2560×1440), one video pixel per point.
pub const LIGHT_MAX_EDGE: usize = 2560;

/// Longest edge of the recording-setup preview in light mode.
pub const LIGHT_PREVIEW_EDGE: u32 = 1280;

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: &'static CFString;
    fn VTCompressionSessionCreate(
        allocator: *const c_void,
        width: i32,
        height: i32,
        codec: u32,
        encoder_specification: *const CFDictionary,
        source_attributes: *const c_void,
        compressed_allocator: *const c_void,
        output_callback: *const c_void,
        refcon: *mut c_void,
        session: *mut *mut c_void,
    ) -> i32;
    fn VTCompressionSessionInvalidate(session: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// Record and preview lighter: on by default on Intel Macs. GIMBAL_LIGHT=1
/// turns it on anywhere, GIMBAL_LIGHT=0 off.
pub fn light() -> bool {
    static LIGHT: OnceLock<bool> = OnceLock::new();
    *LIGHT.get_or_init(|| match std::env::var("GIMBAL_LIGHT").as_deref() {
        Ok("0") => false,
        Ok(_) => true,
        Err(_) => cfg!(target_arch = "x86_64"),
    })
}

/// Whether the hardware encoder takes `w`×`h` frames of `codec` (probed once
/// per size by opening a compression session that requires it).
pub fn hw_encodes(codec: u32, w: usize, h: usize) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<(u32, usize, usize), bool>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(&ok) = seen.lock().unwrap().get(&(codec, w, h)) {
        return ok;
    }
    // SAFETY: a session created with no callback, invalidated and released here.
    let ok = unsafe {
        let key = cf_ns(kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder);
        let spec = dict(vec![(key, num_b(true))]);
        let spec = &*spec as *const Dict as *const CFDictionary;
        let mut session = std::ptr::null_mut();
        let status = VTCompressionSessionCreate(
            std::ptr::null(),
            w as i32,
            h as i32,
            codec,
            spec,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut session,
        );
        if !session.is_null() {
            VTCompressionSessionInvalidate(session);
            CFRelease(session);
        }
        status == 0
    };
    seen.lock().unwrap().insert((codec, w, h), ok);
    ok
}

/// `size` scaled down (aspect ratio kept, even sides) to at most `max_edge`
/// on the long side and so the hardware encoder takes it. Only the
/// `max_edge` cap applies if no size fits (no hardware encoder at all).
pub fn fit_encoder(codec: u32, size: (usize, usize), max_edge: usize) -> (usize, usize) {
    let even = |v: f64| ((v / 2.0).round() as usize * 2).max(2);
    let scaled = |s: f64| (even(size.0 as f64 * s), even(size.1 as f64 * s));
    let long = size.0.max(size.1) as f64;
    let mut s = (max_edge as f64 / long).min(1.0);
    let start = scaled(s);
    let mut failed: Option<f64> = None;
    while long * s >= 640.0 {
        let candidate = scaled(s);
        if hw_encodes(codec, candidate.0, candidate.1) {
            // 10% steps can skip sizes the encoder takes (5K: 4148 px wide
            // fails, 3732 fits, yet 4096 fits too): bisect up to the limit.
            let Some(mut hi) = failed else { return candidate };
            let (mut lo, mut best) = (s, candidate);
            while (hi - lo) * long > 2.0 {
                let mid = (lo + hi) / 2.0;
                let m = scaled(mid);
                if hw_encodes(codec, m.0, m.1) {
                    (lo, best) = (mid, m);
                } else {
                    hi = mid;
                }
            }
            return best;
        }
        failed = Some(s);
        s *= 0.9;
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitted_sizes_are_even_and_keep_the_aspect() {
        let (w, h) = fit_encoder(HEVC, (5120, 2880), LIGHT_MAX_EDGE);
        assert!(w % 2 == 0 && h % 2 == 0);
        assert!(w <= LIGHT_MAX_EDGE && h <= 2880);
        assert!(((w as f64 / h as f64) - 16.0 / 9.0).abs() < 0.01);
    }

    #[test]
    fn a_5k_display_fits_the_hardware_encoder() {
        // Unless this Mac has no hardware encoder at all.
        if !hw_encodes(HEVC, 1920, 1080) {
            return;
        }
        for codec in [H264, HEVC] {
            let (w, h) = fit_encoder(codec, (5120, 2880), usize::MAX);
            assert!(hw_encodes(codec, w, h), "{w}x{h}");
            // As large as the encoder allows, not a 10% step below it.
            if hw_encodes(codec, 4096, 2304) {
                assert!(w >= 4090, "{w}x{h}");
            }
        }
    }
}
