//! Updating the installed app with Sparkle.
//!
//! `scripts/release-macos.sh` embeds Sparkle.framework, and Info.plist names the update feed and
//! the key every update must be signed with. Sparkle is loaded at run time rather than linked, so
//! `scripts/build.sh` and `cargo run` builds run without it, and [`check_for_updates`] opens the
//! latest release instead.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{NSApplication, NSMenuItem, NSWorkspace};
use objc2_foundation::{NSBundle, NSURL};

use crate::util::ns;

const LATEST_RELEASE: &str = "https://github.com/michaelshimeles/daisycap/releases/latest";

thread_local! {
    /// The `SPUStandardUpdaterController`, kept for the life of the app; `None` without Sparkle.
    static CONTROLLER: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
}

/// Starts Sparkle, which checks the feed once a day and shows its update window when a newer
/// release is out, and adds "Check for Updates…" to the app menu. Main thread; later calls do
/// nothing.
pub fn start() {
    with(|_| ());
}

/// Checks for updates now in Sparkle's window, or opens the latest release without Sparkle.
pub fn check_for_updates() {
    // SAFETY: `-[SPUStandardUpdaterController checkForUpdates:]` takes a nullable sender and
    // returns nothing.
    let checked = with(|controller| unsafe {
        let () = msg_send![controller, checkForUpdates: None::<&AnyObject>];
    });
    if checked.is_none()
        && let Some(url) = NSURL::URLWithString(&ns(LATEST_RELEASE))
    {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Runs `f` with the updater controller, loading Sparkle on first use. Main thread only.
fn with<R>(f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
    MainThreadMarker::new()?;
    CONTROLLER.with(|c| c.get_or_init(load).as_deref().map(f))
}

fn load() -> Option<Retained<AnyObject>> {
    let mtm = MainThreadMarker::new()?;
    let frameworks = NSBundle::mainBundle().privateFrameworksPath()?;
    let bundle = NSBundle::bundleWithPath(&ns(&format!("{frameworks}/Sparkle.framework")))?;
    // SAFETY: the framework is sealed in this app's signature; loading it registers its classes.
    // The hardened runtime refuses it unless it is signed by the app's team.
    if !unsafe { bundle.load() } {
        return None;
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")?;
    // SAFETY: `initWithStartingUpdater:updaterDelegate:userDriverDelegate:` takes a BOOL and two
    // nullable delegates, and returns the initialised controller.
    let controller: Retained<AnyObject> = unsafe {
        let controller: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            controller,
            initWithStartingUpdater: Bool::YES,
            updaterDelegate: None::<&AnyObject>,
            userDriverDelegate: None::<&AnyObject>
        ]
    };
    add_menu_item(mtm, &controller);
    Some(controller)
}

/// "Check for Updates…" under "About DaisyCap" in the app menu. The controller is the item's target,
/// so Sparkle also greys it out while a check is running.
fn add_menu_item(mtm: MainThreadMarker, controller: &AnyObject) {
    let Some(app_menu) =
        NSApplication::sharedApplication(mtm).mainMenu().and_then(|m| m.itemAtIndex(0)).and_then(|i| i.submenu())
    else {
        return;
    };
    // SAFETY: `checkForUpdates:` is the controller's action method; the menu holds its target
    // weakly, and `CONTROLLER` keeps it alive for the life of the app.
    unsafe {
        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns("Check for Updates…"),
            Some(sel!(checkForUpdates:)),
            &ns(""),
        );
        item.setTarget(Some(controller));
        app_menu.insertItem_atIndex(&item, 1);
    }
}
