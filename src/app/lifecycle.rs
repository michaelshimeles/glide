//! What to do with a running export when the editor is about to go away, and
//! the order in which one editor replaces another.

use std::path::Path;

/// The way the editor is being left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaving {
    Quit,
    CloseProject,
    OpenProject,
    /// A recording just finished and wants to open itself.
    OpenRecording,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportGuard {
    /// No export is running: carry on.
    Proceed,
    /// An export is running and nobody can be asked: cancel it and carry on.
    ProceedCancelling,
    /// An export is running: ask first, and cancel it only on a yes.
    Ask { prompt: &'static str },
}

pub fn export_guard(export_running: bool, scripted: bool, leaving: Leaving) -> ExportGuard {
    match (export_running, scripted) {
        (false, _) => ExportGuard::Proceed,
        (true, true) => ExportGuard::ProceedCancelling,
        (true, false) => ExportGuard::Ask {
            prompt: match leaving {
                Leaving::Quit => "An export is running. Cancel it and quit?",
                Leaving::CloseProject => "An export is running. Cancel it and close this recording?",
                Leaving::OpenProject => "An export is running. Cancel it and open another recording?",
                Leaving::OpenRecording => "An export is running. Cancel it and open the new recording?",
            },
        },
    }
}

/// The part of the open editor that `switch_to` needs.
pub trait Outgoing {
    fn export_running(&self) -> bool;
    fn cancel_export(&mut self);
    /// Write pending edits to disk.
    fn flush(&mut self);
}

impl Outgoing for super::editor::Editor {
    fn export_running(&self) -> bool {
        Self::export_running(self)
    }
    fn cancel_export(&mut self) {
        Self::cancel_export(self)
    }
    fn flush(&mut self) {
        Self::flush(self)
    }
}

/// Whether `requested` names the project that is already `open`, however it is spelled.
pub fn same_project(open: &Path, requested: &Path) -> bool {
    match (open.canonicalize(), requested.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => open == requested,
    }
}

/// How a switch to another project ended.
#[derive(Debug)]
pub enum Switch<N> {
    /// The requested project is already open: nothing changed.
    Same,
    /// The new editor is ready and any running export was cancelled.
    Opened(N),
    /// The user chose to keep the running export; the new editor was dropped.
    Declined,
    /// The new editor could not be built; the old one and its export are untouched.
    Failed(anyhow::Error),
}

/// Replace the open editor with the one `open` builds. The old editor's edits
/// are flushed first so the new one reads them. Its export is only cancelled
/// (after `ask`, unless `scripted`) once the new editor exists.
pub fn switch_to<O: Outgoing, N>(
    mut current: Option<&mut O>,
    same: bool,
    scripted: bool,
    why: Leaving,
    open: impl FnOnce() -> anyhow::Result<N>,
    ask: impl FnOnce(&'static str) -> bool,
) -> Switch<N> {
    if same {
        return Switch::Same;
    }
    if let Some(old) = &mut current {
        old.flush();
    }
    let new = match open() {
        Ok(new) => new,
        Err(e) => return Switch::Failed(e),
    };
    let running = current.as_ref().is_some_and(|old| old.export_running());
    match export_guard(running, scripted, why) {
        ExportGuard::Proceed => {}
        ExportGuard::ProceedCancelling => cancel(&mut current),
        ExportGuard::Ask { prompt } => {
            if !ask(prompt) {
                return Switch::Declined;
            }
            cancel(&mut current);
        }
    }
    Switch::Opened(new)
}

fn cancel<O: Outgoing>(current: &mut Option<&mut O>) {
    if let Some(old) = current {
        old.cancel_export();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    const ALL: [Leaving; 4] = [Leaving::Quit, Leaving::CloseProject, Leaving::OpenProject, Leaving::OpenRecording];

    #[test]
    fn nothing_running_never_asks() {
        for scripted in [false, true] {
            for leaving in ALL {
                assert_eq!(export_guard(false, scripted, leaving), ExportGuard::Proceed);
            }
        }
    }

    #[test]
    fn scripted_runs_cancel_without_asking() {
        for leaving in ALL {
            assert_eq!(export_guard(true, true, leaving), ExportGuard::ProceedCancelling);
        }
    }

    #[test]
    fn an_interactive_run_asks_in_words_that_match_the_action() {
        let prompt = |leaving| match export_guard(true, false, leaving) {
            ExportGuard::Ask { prompt } => prompt,
            other => panic!("expected a question, got {other:?}"),
        };
        assert_eq!(prompt(Leaving::Quit), "An export is running. Cancel it and quit?");
        assert_eq!(prompt(Leaving::CloseProject), "An export is running. Cancel it and close this recording?");
        assert_eq!(prompt(Leaving::OpenProject), "An export is running. Cancel it and open another recording?");
        assert_eq!(prompt(Leaving::OpenRecording), "An export is running. Cancel it and open the new recording?");
    }

    type Log = Rc<RefCell<Vec<&'static str>>>;

    struct Fake {
        log: Log,
        running: bool,
    }
    impl Outgoing for Fake {
        fn export_running(&self) -> bool {
            self.running
        }
        fn cancel_export(&mut self) {
            self.log.borrow_mut().push("cancel");
            self.running = false;
        }
        fn flush(&mut self) {
            self.log.borrow_mut().push("flush");
        }
    }

    /// Counts how many times it has been dropped.
    struct Token(Rc<Cell<u32>>);
    impl Drop for Token {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn fake(running: bool) -> (Fake, Log) {
        let log = Log::default();
        (Fake { log: log.clone(), running }, log)
    }

    /// Run `switch_to` for a different project whose open and question are logged.
    fn run(old: &mut Fake, log: &Log, scripted: bool, why: Leaving, open_ok: bool, answer: bool, dropped: &Rc<Cell<u32>>) -> (Switch<Token>, Vec<&'static str>) {
        let (open_log, ask_log, dropped) = (log.clone(), log.clone(), dropped.clone());
        let out = switch_to(
            Some(old),
            false,
            scripted,
            why,
            || {
                open_log.borrow_mut().push("open");
                if open_ok { Ok(Token(dropped)) } else { Err(anyhow::anyhow!("bad project")) }
            },
            |_| {
                ask_log.borrow_mut().push("ask");
                answer
            },
        );
        let calls = log.borrow().clone();
        (out, calls)
    }

    #[test]
    fn a_failed_open_leaves_a_running_export_alone() {
        let (mut old, log) = fake(true);
        let (out, calls) = run(&mut old, &log, false, Leaving::OpenProject, false, true, &Rc::default());
        assert!(matches!(out, Switch::Failed(_)));
        assert_eq!(calls, ["flush", "open"]);
        assert!(old.running);
    }

    #[test]
    fn a_declined_question_keeps_the_export_and_drops_the_new_editor() {
        let (mut old, log) = fake(true);
        let dropped = Rc::new(Cell::new(0));
        let (out, calls) = run(&mut old, &log, false, Leaving::OpenProject, true, false, &dropped);
        assert!(matches!(out, Switch::Declined));
        assert_eq!(calls, ["flush", "open", "ask"]);
        assert!(old.running);
        assert_eq!(dropped.get(), 1);
    }

    #[test]
    fn an_accepted_question_cancels_after_the_new_editor_is_ready() {
        let (mut old, log) = fake(true);
        let dropped = Rc::new(Cell::new(0));
        let (out, calls) = run(&mut old, &log, false, Leaving::OpenProject, true, true, &dropped);
        assert!(matches!(out, Switch::Opened(_)));
        assert_eq!(calls, ["flush", "open", "ask", "cancel"]);
        assert_eq!(dropped.get(), 0);
    }

    #[test]
    fn scripted_switches_cancel_without_asking() {
        let (mut old, log) = fake(true);
        let (out, calls) = run(&mut old, &log, true, Leaving::OpenRecording, true, false, &Rc::default());
        assert!(matches!(out, Switch::Opened(_)));
        assert_eq!(calls, ["flush", "open", "cancel"]);
    }

    #[test]
    fn nothing_running_never_asks_or_cancels() {
        let (mut old, log) = fake(false);
        let (out, calls) = run(&mut old, &log, false, Leaving::OpenProject, true, false, &Rc::default());
        assert!(matches!(out, Switch::Opened(_)));
        assert_eq!(calls, ["flush", "open"]);
        let none = switch_to(None::<&mut Fake>, false, false, Leaving::OpenRecording, || Ok(1), |_| panic!("no question without an editor"));
        assert!(matches!(none, Switch::Opened(1)));
    }

    #[test]
    fn reopening_the_open_project_only_shows_it() {
        let (mut old, log) = fake(true);
        let out = switch_to(Some(&mut old), true, false, Leaving::OpenProject, || -> anyhow::Result<()> { panic!("no second editor") }, |_| panic!("no question"));
        assert!(matches!(out, Switch::Same));
        assert!(log.borrow().is_empty());
        assert!(old.running);
    }

    #[test]
    fn pending_edits_are_flushed_before_the_new_editor_reads_the_file() {
        let (mut old, log) = fake(false);
        let (_, calls) = run(&mut old, &log, false, Leaving::OpenProject, true, true, &Rc::default());
        assert_eq!(calls.iter().position(|c| *c == "flush"), Some(0));
        assert!(calls.iter().position(|c| *c == "flush") < calls.iter().position(|c| *c == "open"));
    }

    #[test]
    fn the_recording_path_asks_in_its_own_words() {
        let (mut old, _) = fake(true);
        let mut prompt = None;
        let out = switch_to(Some(&mut old), false, false, Leaving::OpenRecording, || Ok(()), |p| {
            prompt = Some(p);
            true
        });
        assert!(matches!(out, Switch::Opened(())));
        assert_eq!(prompt, Some("An export is running. Cancel it and open the new recording?"));
    }

    #[test]
    fn same_project_sees_through_dot_segments_trailing_slashes_and_symlinks() {
        let root = std::env::temp_dir().join(format!("glide-same-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("Clip.glide");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(root.join("other.glide")).unwrap();
        let link = root.join("link.glide");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(same_project(&real, &real));
        assert!(same_project(&real, &root.join("other.glide").join("..").join("Clip.glide")));
        assert!(same_project(&real, &root.join("./Clip.glide/")));
        assert!(same_project(&real, &link));
        assert!(!same_project(&real, &root.join("other.glide")));
        // A path that does not exist falls back to plain equality.
        assert!(same_project(&root.join("gone.glide"), &root.join("gone.glide")));
        assert!(!same_project(&root.join("gone.glide"), &root.join("other.glide")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
