//! A small log file for problems that are hard to see from the app, mainly
//! iPhone/iPad discovery and capture. Lines go to
//! `~/Library/Logs/Glide/glide.log`, which Console.app and any editor open.
//! Only device names and ids, error texts, counts and timings belong here.
//!
//! Writing never fails the caller: any I/O error is ignored.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The file is cut to about half its size once it grows past this.
pub const LOG_CAP: usize = 512 * 1024;

/// The log file, or `GLIDE_LOG_FILE` when set.
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GLIDE_LOG_FILE") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/Glide/glide.log"))
}

/// One log line: `ts msg` on a single line (newlines in `msg` become spaces).
pub fn line(ts: &str, msg: &str) -> String {
    let msg: String = msg.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    format!("{ts} {}\n", msg.trim_end())
}

/// `content` cut to about its newer half when it is longer than `cap`: the
/// oldest lines go, and the rest starts at a line start. A single line longer
/// than that is cut from its front instead.
pub fn trimmed(content: &str, cap: usize) -> String {
    if content.len() <= cap {
        return content.to_string();
    }
    let mut start = content.len() - cap / 2;
    while !content.is_char_boundary(start) {
        start += 1;
    }
    let tail = &content[start..];
    // The cut fell inside a line unless the byte before it ends one.
    if content.as_bytes()[start - 1] == b'\n' {
        return tail.to_string();
    }
    match tail.find('\n') {
        Some(i) => tail[i + 1..].to_string(),
        None => tail.to_string(),
    }
}

/// Append `line` to `path`, first cutting the file down if it passed `cap`.
fn append(path: &Path, line: &str, cap: usize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() as usize > cap) {
        let old = String::from_utf8_lossy(&std::fs::read(path)?).into_owned();
        let tmp = path.with_extension("log.tmp");
        std::fs::write(&tmp, trimmed(&old, cap))?;
        std::fs::rename(&tmp, path)?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// Add `msg` to the log file. Does nothing in unit tests, so `cargo test`
/// doesn't write into the user's log.
pub fn log(msg: &str) {
    static LOCK: Mutex<()> = Mutex::new(());
    if cfg!(test) {
        return;
    }
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = path() {
        let _ = append(&p, &line(&crate::util::timestamp_name(), msg), LOG_CAP);
    }
}

/// Print `msg` to stderr, as before, and add it to the log file.
pub fn note(msg: &str) {
    eprintln!("{msg}");
    log(msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_stays_on_one_line() {
        assert_eq!(line("t", "a\nb\r\nc\n"), "t a b  c\n");
        assert_eq!(line("t", "plain"), "t plain\n");
    }

    #[test]
    fn trimmed_leaves_a_short_log_alone() {
        assert_eq!(trimmed("a\nb\n", 100), "a\nb\n");
        assert_eq!(trimmed("a\nb\n", 4), "a\nb\n");
    }

    #[test]
    fn trimmed_drops_the_oldest_half_and_starts_on_a_line() {
        let log: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        let cut = trimmed(&log, 500);
        assert!(cut.len() <= 250 + 9, "{}", cut.len());
        assert!(cut.starts_with("line "), "starts mid-line: {:?}", &cut[..10]);
        assert!(cut.ends_with("line 099\n"));
        let kept: Vec<&str> = cut.lines().collect();
        assert!(kept.windows(2).all(|w| w[0] < w[1]), "order lost");
        assert!(log.ends_with(&cut));
    }

    #[test]
    fn trimmed_cuts_one_endless_line_instead_of_looping() {
        let long = "x".repeat(1000);
        let cut = trimmed(&long, 100);
        assert_eq!(cut.len(), 50);
        // Multi-byte text is cut on a character boundary.
        let wide = "é".repeat(500);
        assert!(trimmed(&wide, 100).chars().all(|c| c == 'é'));
    }

    #[test]
    fn append_creates_the_folder_and_keeps_the_newest_lines_under_the_cap() {
        let dir = std::env::temp_dir().join(format!("glide-diag-test-{}", std::process::id()));
        let file = dir.join("nested/glide.log");
        let _ = std::fs::remove_dir_all(&dir);
        for i in 0..200 {
            append(&file, &line("t", &format!("entry {i:03}")), 1000).unwrap();
        }
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.len() <= 1000 + 20, "{}", text.len());
        assert!(text.ends_with("t entry 199\n"));
        assert!(text.starts_with("t entry "));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
