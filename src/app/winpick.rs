//! Window picker: the windows on a display, front to back, positioned in
//! the overlay's coordinates so they can be highlighted and clicked.

use std::collections::HashSet;
use std::sync::Mutex;

use super::home::WindowEntry;
use super::macos;
use crate::capture::{screen, winsize};

/// A window the picker can offer.
#[derive(Clone, Debug)]
pub struct PickWindow {
    pub entry: WindowEntry,
    pub app: String,
    pub title: String,
    /// Frame in overlay points (display-relative).
    pub rect: egui::Rect,
    /// The display (overlay) it is drawn on.
    pub display: u32,
}

/// Windows hidden from the picker for the rest of the session.
static IGNORED: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

pub fn ignore(id: u32) {
    IGNORED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().insert(id);
}

pub fn reset_ignored() {
    *IGNORED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn is_ignored(id: u32) -> bool {
    IGNORED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&id))
}

/// Recordable windows on every display, frontmost first per display.
pub fn list_all() -> Vec<PickWindow> {
    macos::display_ids().into_iter().flat_map(list).collect()
}

/// Recordable windows (not DaisyCap's own) that show on `display`,
/// frontmost first.
pub fn list(display: u32) -> Vec<PickWindow> {
    let (display_id, dx, dy, dw, dh) = macos::display_frame(display);
    let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(dw as f32, dh as f32));
    let Ok(all) = screen::windows() else { return vec![] };
    let me = std::process::id() as i32;
    let all: Vec<_> = all
        .into_iter()
        // SAFETY: SCWindow/SCRunningApplication accessors.
        .filter(|w| unsafe { w.window.owningApplication().is_none_or(|a| a.processID() != me) })
        .collect();
    let displays = winsize::display_sizes(&all);
    let order = winsize::front_to_back();
    let mut out: Vec<(usize, PickWindow)> = all
        .into_iter()
        .zip(displays)
        .filter_map(|(w, display)| {
            // SAFETY: SCWindow accessor.
            let f = unsafe { w.window.frame() };
            let rect = egui::Rect::from_min_size(
                egui::pos2((f.origin.x - dx) as f32, (f.origin.y - dy) as f32),
                egui::vec2(f.size.width as f32, f.size.height as f32),
            );
            let visible = rect.intersect(screen_rect);
            if visible.width() < 40.0 || visible.height() < 40.0 {
                return None;
            }
            let z = order.iter().position(|&id| id == w.id).unwrap_or(usize::MAX);
            let label = if w.title.is_empty() { w.app.clone() } else { format!("{} — {}", w.app, w.title) };
            let entry = WindowEntry { id: w.id, label, size: w.size, display };
            Some((z, PickWindow { entry, app: w.app, title: w.title, rect, display: display_id }))
        })
        .collect();
    out.sort_by_key(|(z, _)| *z);
    out.into_iter().map(|(_, w)| w).collect()
}

/// The frontmost picker window under `p`.
pub fn under(windows: &[PickWindow], p: egui::Pos2) -> Option<&PickWindow> {
    windows.iter().find(|w| !is_ignored(w.entry.id) && w.rect.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u32, x: f32, y: f32, w: f32, h: f32) -> PickWindow {
        PickWindow {
            entry: WindowEntry { id, label: String::new(), size: (w as f64, h as f64), display: (2560.0, 1440.0) },
            app: String::new(),
            title: String::new(),
            rect: egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)),
            display: 1,
        }
    }

    #[test]
    fn frontmost_window_wins_and_ignored_ones_are_skipped() {
        let ws = vec![win(1, 100.0, 100.0, 400.0, 300.0), win(2, 0.0, 0.0, 1000.0, 800.0)];
        assert_eq!(under(&ws, egui::pos2(200.0, 200.0)).map(|w| w.entry.id), Some(1));
        assert_eq!(under(&ws, egui::pos2(900.0, 700.0)).map(|w| w.entry.id), Some(2));
        assert!(under(&ws, egui::pos2(1200.0, 900.0)).is_none());
        ignore(1);
        assert_eq!(under(&ws, egui::pos2(200.0, 200.0)).map(|w| w.entry.id), Some(2));
        reset_ignored();
        assert_eq!(under(&ws, egui::pos2(200.0, 200.0)).map(|w| w.entry.id), Some(1));
    }
}
