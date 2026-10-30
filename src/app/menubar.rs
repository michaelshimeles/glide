//! Menu bar status item (adapted from rec-rs): record icon (red with the elapsed time while
//! recording) and a menu. Menu actions are queued as `Cmd`s for the app.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_foundation::*;

use super::Cmd;
use crate::util::ns;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "GlideMenuTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(toggleRecording:))]
        fn toggle_recording(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::ToggleRecording);
        }
        #[unsafe(method(recordDisplay:))]
        fn record_display(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::RecordSource(super::home::SourceKind::Display));
        }
        #[unsafe(method(recordWindow:))]
        fn record_window(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::RecordSource(super::home::SourceKind::Window));
        }
        #[unsafe(method(recordArea:))]
        fn record_area(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::RecordSource(super::home::SourceKind::Area));
        }
        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::TogglePause);
        }
        #[unsafe(method(showWindow:))]
        fn show_window(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::ShowWindow);
        }
        #[unsafe(method(openFolder:))]
        fn open_folder(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::OpenFolder);
        }
        #[unsafe(method(checkUpdates:))]
        fn check_updates(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::CheckForUpdates);
        }
        #[unsafe(method(quit:))]
        fn quit(&self, _s: Option<&AnyObject>) {
            super::push(Cmd::Quit);
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

pub struct MenuBar {
    status: Retained<NSStatusItem>,
    record: Retained<NSMenuItem>,
    pause: Retained<NSMenuItem>,
    sources: [Retained<NSMenuItem>; 3],
    hotkey: Retained<NSMenuItem>,
    _target: Retained<Target>,
    last: Option<(bool, bool, bool, String)>,
}

/// What the menu bar shows.
pub struct View<'a> {
    pub recording: bool,
    pub paused: bool,
    pub finishing: bool,
    pub clock: &'a str,
}

impl MenuBar {
    pub fn new(mtm: MainThreadMarker) -> MenuBar {
        let target = Target::new(mtm);
        let item = |title: &str, action: Option<Sel>, key: &str| {
            let it = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), action, &ns(key))
            };
            if action.is_some() {
                unsafe { it.setTarget(Some(&target)) };
            }
            it
        };
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let record = item("Start Recording", Some(sel!(toggleRecording:)), "r");
        record.setKeyEquivalentModifierMask(NSEventModifierFlags::Command | NSEventModifierFlags::Shift | NSEventModifierFlags::Option);
        menu.addItem(&record);
        let sources = [
            item("Record Display", Some(sel!(recordDisplay:)), ""),
            item("Record Window…", Some(sel!(recordWindow:)), ""),
            item("Record Area…", Some(sel!(recordArea:)), ""),
        ];
        for s in &sources {
            menu.addItem(s);
        }
        let pause = item("Pause", Some(sel!(togglePause:)), "");
        menu.addItem(&pause);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&item("Show DaisyCap", Some(sel!(showWindow:)), ""));
        menu.addItem(&item("Open Projects Folder", Some(sel!(openFolder:)), ""));
        menu.addItem(&item("Check for Updates…", Some(sel!(checkUpdates:)), ""));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let hotkey = item("Global hotkey: ⌥⇧⌘R", None, "");
        hotkey.setEnabled(false);
        menu.addItem(&hotkey);
        menu.addItem(&item("Quit DaisyCap", Some(sel!(quit:)), "q"));
        status.setMenu(Some(&menu));
        let mut mb = MenuBar { status, record, pause, sources, hotkey, _target: target, last: None };
        mb.update(&View { recording: false, paused: false, finishing: false, clock: "" });
        mb
    }

    pub fn set_hotkey(&self, label: &str, ok: bool) {
        let title = match (label.is_empty(), ok) {
            (true, _) => "No global shortcut".to_string(),
            (false, true) => format!("Global shortcut: {label}"),
            (false, false) => format!("Global shortcut {label} unavailable"),
        };
        self.hotkey.setTitle(&ns(&title));
    }

    pub fn title(&self) -> Option<String> {
        let mtm = MainThreadMarker::new()?;
        self.status.button(mtm).map(|b| b.title().to_string())
    }

    pub fn update(&mut self, v: &View) {
        let key = (v.recording, v.paused, v.finishing, v.clock.to_string());
        if self.last.as_ref() == Some(&key) {
            return;
        }
        self.last = Some(key);
        self.record.setTitle(&ns(if v.finishing {
            "Saving…"
        } else if v.recording {
            "Stop Recording"
        } else {
            "Start Recording"
        }));
        self.record.setEnabled(!v.finishing);
        self.pause.setTitle(&ns(if v.paused { "Resume" } else { "Pause" }));
        self.pause.setEnabled(v.recording);
        for s in &self.sources {
            s.setHidden(v.recording || v.finishing);
        }
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(button) = self.status.button(mtm) else { return };
        let (symbol, title, red) = if v.finishing {
            ("record.circle", " Saving…".to_string(), false)
        } else if v.paused {
            ("pause.circle.fill", format!(" {}", v.clock), false)
        } else if v.recording {
            ("record.circle.fill", format!(" {}", v.clock), true)
        } else {
            ("record.circle", String::new(), false)
        };
        if let Some(img) = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), Some(&ns("DaisyCap"))) {
            img.setTemplate(!red);
            button.setImage(Some(&img));
        }
        button.setContentTintColor(if red { Some(NSColor::systemRedColor()) } else { None }.as_deref());
        button.setImagePosition(NSCellImagePosition::ImageLeft);
        button.setTitle(&ns(&title));
    }
}
