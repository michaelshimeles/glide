//! Restarting a take: the old one is cancelled off the main thread, and the
//! new one starts only once that cancel has finished.

/// Where a restart is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Restart {
    #[default]
    Idle,
    /// The old take is being cancelled. `then_start` is false once the user
    /// cancelled the restart itself.
    Cancelling { then_start: bool },
}

impl Restart {
    /// A cancel is in flight: nothing new may start.
    pub fn busy(self) -> bool {
        self != Restart::Idle
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The user asked to restart; `recording` says whether a take exists.
    Requested { recording: bool },
    /// The old take is gone.
    Cancelled,
    /// The user cancelled while the old take was still being removed.
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    CancelOldTake,
    StartNewTake,
    /// Close the bar and show the window again.
    CloseBar,
}

pub fn step(state: Restart, event: Event) -> (Restart, Do) {
    match (state, event) {
        (Restart::Idle, Event::Requested { recording: true }) => (Restart::Cancelling { then_start: true }, Do::CancelOldTake),
        (Restart::Cancelling { then_start: true }, Event::Cancelled) => (Restart::Idle, Do::StartNewTake),
        (Restart::Cancelling { then_start: false }, Event::Cancelled) => (Restart::Idle, Do::CloseBar),
        (Restart::Cancelling { .. }, Event::Aborted) => (Restart::Cancelling { then_start: false }, Do::Nothing),
        (state, _) => (state, Do::Nothing),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Restart::{Cancelling, Idle};

    #[test]
    fn restart_while_recording_cancels_the_old_take() {
        assert_eq!(step(Idle, Event::Requested { recording: true }), (Cancelling { then_start: true }, Do::CancelOldTake));
    }

    #[test]
    fn restart_with_no_take_does_nothing() {
        assert_eq!(step(Idle, Event::Requested { recording: false }), (Idle, Do::Nothing));
    }

    #[test]
    fn a_second_request_while_cancelling_is_ignored() {
        for then_start in [true, false] {
            let s = Cancelling { then_start };
            assert_eq!(step(s, Event::Requested { recording: true }), (s, Do::Nothing));
            assert_eq!(step(s, Event::Requested { recording: false }), (s, Do::Nothing));
        }
    }

    #[test]
    fn a_finished_cancel_starts_the_new_take() {
        assert_eq!(step(Cancelling { then_start: true }, Event::Cancelled), (Idle, Do::StartNewTake));
    }

    #[test]
    fn a_finished_cancel_after_an_abort_closes_the_bar() {
        assert_eq!(step(Cancelling { then_start: false }, Event::Cancelled), (Idle, Do::CloseBar));
    }

    #[test]
    fn an_abort_while_cancelling_drops_the_new_take() {
        for then_start in [true, false] {
            assert_eq!(step(Cancelling { then_start }, Event::Aborted), (Cancelling { then_start: false }, Do::Nothing));
        }
    }

    #[test]
    fn stray_events_when_idle_do_nothing() {
        assert_eq!(step(Idle, Event::Cancelled), (Idle, Do::Nothing));
        assert_eq!(step(Idle, Event::Aborted), (Idle, Do::Nothing));
    }

    #[test]
    fn busy_only_while_cancelling() {
        assert!(!Idle.busy());
        assert!(Cancelling { then_start: true }.busy());
        assert!(Cancelling { then_start: false }.busy());
    }

    #[test]
    fn a_restart_cancels_once_and_starts_one_take() {
        let mut s = Idle;
        let mut dos = vec![];
        for e in [Event::Requested { recording: true }, Event::Requested { recording: true }, Event::Cancelled, Event::Cancelled] {
            let (next, d) = step(s, e);
            s = next;
            dos.push(d);
        }
        assert_eq!(dos, [Do::CancelOldTake, Do::Nothing, Do::StartNewTake, Do::Nothing]);
        assert_eq!(s, Idle);
    }

    #[test]
    fn cancel_during_a_restart_closes_the_bar_after_the_old_take_is_gone() {
        let (s, d) = step(Idle, Event::Requested { recording: true });
        assert_eq!(d, Do::CancelOldTake);
        let (s, d) = step(s, Event::Aborted);
        assert_eq!(d, Do::Nothing);
        assert!(s.busy());
        let (s, d) = step(s, Event::Cancelled);
        assert_eq!((s, d), (Idle, Do::CloseBar));
    }
}
