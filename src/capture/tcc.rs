//! Privacy (TCC) handling: responsibility disclaim, permission status, requests.

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

const DISCLAIM_ENV: &str = "GLIDE_DISCLAIMED";

/// Re-exec ourselves with responsibility disclaimed so TCC attributes screen,
/// mic and camera access to `rec` itself rather than the terminal/tmux/IDE that
/// launched it. Returns only in the child (or if disclaim is unavailable).
pub fn disclaim_and_reexec() {
    if std::env::var_os(DISCLAIM_ENV).is_some() || std::env::var_os("GLIDE_NO_DISCLAIM").is_some() {
        return;
    }
    // Already responsible for ourselves (launched via `open Rec.app`, Login
    // Items, Finder): nothing to do, and LaunchServices expects this process
    // to be the app. Checking the actual responsible pid (not ppid == 1)
    // keeps backgrounded/orphaned CLI launches correct.
    type GetResponsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    let get = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr()) };
    if !get.is_null() {
        let get: GetResponsible = unsafe { std::mem::transmute::<*mut c_void, GetResponsible>(get) };
        let me = unsafe { libc::getpid() };
        if unsafe { get(me) } == me {
            return;
        }
    }
    type SetDisclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, c_int) -> c_int;
    let sym = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_spawnattrs_setdisclaim".as_ptr(),
        )
    };
    if sym.is_null() {
        eprintln!("glide: warning: responsibility disclaim unavailable; permissions attach to the launching app");
        return;
    }
    let set_disclaim: SetDisclaim = unsafe { std::mem::transmute::<*mut c_void, SetDisclaim>(sym) };

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let exe_c = CString::new(exe.as_os_str().as_encoded_bytes()).unwrap();
    let args: Vec<CString> = std::env::args_os()
        .map(|a| CString::new(a.as_encoded_bytes()).unwrap())
        .collect();
    let mut argv: Vec<*mut c_char> = args.iter().map(|a| a.as_ptr() as *mut c_char).collect();
    argv.push(std::ptr::null_mut());
    let mut envs: Vec<CString> = std::env::vars_os()
        .map(|(k, v)| {
            let mut s = k.as_encoded_bytes().to_vec();
            s.push(b'=');
            s.extend_from_slice(v.as_encoded_bytes());
            CString::new(s).unwrap()
        })
        .collect();
    envs.push(CString::new(format!("{DISCLAIM_ENV}=1")).unwrap());
    let mut envp: Vec<*mut c_char> = envs.iter().map(|e| e.as_ptr() as *mut c_char).collect();
    envp.push(std::ptr::null_mut());

    unsafe {
        let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
        if libc::posix_spawnattr_init(&mut attr) != 0 {
            return;
        }
        if set_disclaim(&mut attr, 1) != 0 {
            libc::posix_spawnattr_destroy(&mut attr);
            return;
        }
        let mut pid: libc::pid_t = 0;
        let rc = libc::posix_spawn(
            &mut pid,
            exe_c.as_ptr(),
            std::ptr::null(),
            &attr,
            argv.as_ptr(),
            envp.as_ptr(),
        );
        libc::posix_spawnattr_destroy(&mut attr);
        if rc != 0 {
            eprintln!("rec: warning: disclaimed spawn failed ({rc}); continuing in-process");
            return;
        }
        CHILD.store(pid, std::sync::atomic::Ordering::SeqCst);
        // Terminal Ctrl-C reaches the whole process group (child included), so
        // the parent ignores it; SIGTERM/SIGHUP are forwarded explicitly.
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, forward as *const () as usize);
        libc::signal(libc::SIGHUP, forward as *const () as usize);
        let mut status: c_int = 0;
        loop {
            let r = libc::waitpid(pid, &mut status, 0);
            if r == pid {
                break;
            }
            if r < 0 && *libc::__error() != libc::EINTR {
                std::process::exit(1);
            }
        }
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            128 + libc::WTERMSIG(status)
        };
        std::process::exit(code);
    }
}

static CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn forward(sig: c_int) {
    let pid = CHILD.load(std::sync::atomic::Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, sig) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Granted => "granted",
            Status::Denied => "DENIED",
            Status::NotDetermined => "not yet requested",
            Status::Restricted => "restricted",
        })
    }
}

pub fn screen_status() -> Status {
    if CGPreflightScreenCaptureAccess() { Status::Granted } else { Status::Denied }
}

fn av_status(video: bool) -> Status {
    let media = unsafe { if video { AVMediaTypeVideo } else { AVMediaTypeAudio } }.unwrap();
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) } {
        AVAuthorizationStatus::Authorized => Status::Granted,
        AVAuthorizationStatus::Denied => Status::Denied,
        AVAuthorizationStatus::Restricted => Status::Restricted,
        _ => Status::NotDetermined,
    }
}

pub fn camera_status() -> Status {
    av_status(true)
}
pub fn mic_status() -> Status {
    av_status(false)
}

/// What to do about Camera access before opening an iPhone/iPad screen. macOS
/// may treat these muxed devices as cameras, and without a grant the session
/// runs and delivers no frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// Not asked yet: request it.
    Ask,
    /// Can't be used; the text says why and where to change it.
    Blocked(&'static str),
}

pub const CAMERA_OFF: &str = "Camera access is off for Glide. Turn it on in System Settings > Privacy & Security > Camera.";
pub const CAMERA_RESTRICTED: &str = "Camera access is restricted on this Mac. Check System Settings > Privacy & Security > Camera, or ask whoever manages this Mac.";
pub const CAMERA_PENDING: &str = "Camera access hasn't been answered yet. Answer the prompt, then try again.";

pub fn camera_gate(status: Status) -> Gate {
    match status {
        Status::Granted => Gate::Go,
        Status::NotDetermined => Gate::Ask,
        Status::Denied => Gate::Blocked(CAMERA_OFF),
        Status::Restricted => Gate::Blocked(CAMERA_RESTRICTED),
    }
}

/// Make sure Camera access is granted, asking if it hasn't been asked yet
/// and waiting up to `wait` for the answer. Blocks: call it from a background
/// thread. A denied or restricted grant fails at once without asking again.
pub fn require_camera(wait: Duration) -> anyhow::Result<()> {
    match camera_gate(camera_status()) {
        Gate::Go => Ok(()),
        Gate::Blocked(msg) => anyhow::bail!("{msg}"),
        Gate::Ask => {
            request_all(false, false, true, wait);
            match camera_gate(camera_status()) {
                Gate::Go => Ok(()),
                Gate::Blocked(msg) => anyhow::bail!("{msg}"),
                Gate::Ask => anyhow::bail!("{CAMERA_PENDING}"),
            }
        }
    }
}

/// Like `require_camera`, for the UI thread: never waits. When the answer is
/// still open it shows the prompt and fails, so the caller tries again.
pub fn check_camera_now() -> anyhow::Result<()> {
    match camera_gate(camera_status()) {
        Gate::Go => Ok(()),
        Gate::Blocked(msg) => anyhow::bail!("{msg}"),
        Gate::Ask => {
            drop(av_request_async(true));
            anyhow::bail!("{CAMERA_PENDING}")
        }
    }
}

fn av_request_async(video: bool) -> mpsc::Receiver<bool> {
    let media = unsafe { if video { AVMediaTypeVideo } else { AVMediaTypeAudio } }.unwrap();
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: Bool| {
        let _ = tx.send(granted.as_bool());
    });
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
    rx
}

/// Request every permission not yet granted, all at once so the user sees every
/// prompt in one go. Screen recording cannot be granted inline: the request
/// registers `rec` in System Settings > Privacy & Security > Screen & System
/// Audio Recording; the user toggles it on and relaunches rec.
pub fn request_all(screen: bool, mic: bool, camera: bool, wait: Duration) {
    let mic_rx = (mic && mic_status() == Status::NotDetermined).then(|| av_request_async(false));
    let cam_rx = (camera && camera_status() == Status::NotDetermined).then(|| av_request_async(true));
    if screen && screen_status() != Status::Granted {
        let ok = CGRequestScreenCaptureAccess();
        eprintln!("screen recording: request issued (granted now: {ok})");
    }
    let deadline = std::time::Instant::now() + wait;
    for (name, rx) in [("microphone", mic_rx), ("camera", cam_rx)] {
        let Some(rx) = rx else { continue };
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(g) => crate::diag::note(&format!("{name}: {}", if g { "granted" } else { "denied" })),
            Err(_) => crate::diag::note(&format!("{name}: prompt still pending (answer it on screen)")),
        }
    }
}

pub fn report() -> (Status, Status, Status) {
    (screen_status(), mic_status(), camera_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_gate_goes_ahead_only_when_granted_and_asks_when_not_determined() {
        assert_eq!(camera_gate(Status::Granted), Gate::Go);
        assert_eq!(camera_gate(Status::NotDetermined), Gate::Ask);
    }

    #[test]
    fn camera_gate_points_denied_to_system_settings() {
        let Gate::Blocked(msg) = camera_gate(Status::Denied) else { panic!("denied must block") };
        assert!(msg.starts_with("Camera access is off for Glide"), "{msg}");
        assert!(msg.contains("System Settings"), "{msg}");
    }

    #[test]
    fn camera_gate_points_restricted_to_system_settings() {
        let Gate::Blocked(msg) = camera_gate(Status::Restricted) else { panic!("restricted must block") };
        assert!(msg.contains("restricted") && msg.contains("System Settings"), "{msg}");
    }
}
