//! Keeps the list of connected iPhones and iPads fresh while recording setup
//! shows the iPhone source. A phone cabled after launch (or unplugged)
//! changes what discovery returns, but nothing else asks again.
//!
//! One background thread asks every `INTERVAL` while `set_wanted(true)`, off
//! the UI thread, and hands the list over only when it changed. It costs one
//! cheap discovery query, not `Devices::load`, which also lists windows.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

use super::home::Device;

const INTERVAL: Duration = Duration::from_secs(2);

static WANTED: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<Vec<Device>>> = Mutex::new(None);
static START: Once = Once::new();

/// Poll while true (the iPhone source is showing), idle otherwise.
pub fn set_wanted(on: bool) {
    WANTED.store(on, Ordering::Relaxed);
    if on {
        START.call_once(|| {
            std::thread::spawn(poll);
        });
    }
}

/// The list found by the poller since the last call, if it changed.
pub fn take() -> Option<Vec<Device>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// True when the two lists don't hold the same ids in the same order.
pub fn differs(a: &[Device], b: &[Device]) -> bool {
    a.len() != b.len() || a.iter().zip(b).any(|(x, y)| x.id != y.id)
}

fn poll() {
    let mut last: Option<Vec<Device>> = None;
    loop {
        std::thread::sleep(INTERVAL);
        if !WANTED.load(Ordering::Relaxed) {
            continue;
        }
        let list: Vec<Device> = objc2::rc::autoreleasepool(|_| {
            crate::capture::camera::ios_devices().into_iter().map(|d| Device { id: d.id, name: d.name }).collect()
        });
        if last.as_deref().is_none_or(|l| differs(l, &list)) {
            crate::diag::note(&format!("glide: ios devices: [{}]", list.iter().map(|d| format!("{} ({})", d.name, d.id)).collect::<Vec<_>>().join(", ")));
            *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(list.clone());
            last = Some(list);
            super::wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(id: &str) -> Device {
        Device { id: id.into(), name: "iPhone".into() }
    }

    #[test]
    fn differs_ignores_names_but_sees_added_removed_and_reordered_ids() {
        let a = [phone("1"), phone("2")];
        assert!(!differs(&a, &[phone("1"), Device { id: "2".into(), name: "Renamed".into() }]));
        assert!(differs(&a, &[phone("1")]));
        assert!(differs(&[], &a));
        assert!(differs(&a, &[phone("2"), phone("1")]));
        assert!(!differs(&[], &[]));
    }
}
