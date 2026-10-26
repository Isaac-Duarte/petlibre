//! Waits that end on the operator's interrupt: a plain sleep, and polling a
//! child process until it exits, something it writes is ready, or a
//! deadline passes. The rig's captures and the suite's servers share the
//! child loop, so each decides only what its outcomes mean.

use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

use crate::{Cancel, Error, Result};

/// The longest a [`sleep`] runs on past an interrupt.
const SLICE: Duration = Duration::from_millis(50);

/// Sleep in short slices so an interrupt ends the wait promptly.
pub fn sleep(cancel: &Cancel, span: Duration) -> Result<()> {
    let end = Instant::now() + span;
    loop {
        if cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(SLICE));
    }
}

/// How a [`wait_child`] ended. The child is never killed here: whether one
/// still running is killed, left for a later stop or waited on further is
/// the caller's to decide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// One of the handles was cancelled.
    Cancelled,
    Exited(ExitStatus),
    /// `ready` held while the child still ran.
    Ready,
    TimedOut,
}

/// Poll `child` every `poll`. Each round first gives up once `deadline` has
/// passed, then checks, in this order, whether any of `cancels` is set,
/// whether the child exited and whether `ready` holds. With no `cancels`
/// the wait runs through an interrupt, as cleanup after one must.
///
/// An error is the child's status being unreadable.
pub fn wait_child(
    child: &mut Child,
    cancels: &[&Cancel],
    deadline: Option<Instant>,
    poll: Duration,
    mut ready: impl FnMut() -> bool,
) -> std::io::Result<Waited> {
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(Waited::TimedOut);
        }
        if cancels.iter().any(|cancel| cancel.is_cancelled()) {
            return Ok(Waited::Cancelled);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(Waited::Exited(status));
        }
        if ready() {
            return Ok(Waited::Ready);
        }
        std::thread::sleep(poll);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    use super::*;

    const POLL: Duration = Duration::from_millis(10);

    fn sleeper() -> Child {
        Command::new("sleep").arg("5").spawn().unwrap()
    }

    #[test]
    fn a_cancelled_sleep_is_interrupted() {
        let cancel = Cancel::new();
        cancel.cancel();

        let slept = sleep(&cancel, Duration::from_secs(5));

        assert!(matches!(slept, Err(Error::Interrupted)));
    }

    #[test]
    fn an_exit_ends_the_wait() {
        let mut child = Command::new("true").spawn().unwrap();

        let waited = wait_child(&mut child, &[], None, POLL, || false).unwrap();

        assert!(matches!(waited, Waited::Exited(status) if status.success()));
    }

    #[test]
    fn a_cancel_comes_before_the_exit_and_kills_nothing() {
        let mut child = sleeper();
        let (first, second) = (Cancel::new(), Cancel::new());
        second.cancel();

        let waited = wait_child(&mut child, &[&first, &second], None, POLL, || true).unwrap();

        assert_eq!(waited, Waited::Cancelled);
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn ready_and_the_deadline_end_a_running_child_s_wait() {
        let mut child = sleeper();

        let ready = wait_child(&mut child, &[], None, POLL, || true).unwrap();
        let deadline = Some(Instant::now() + POLL);
        let timed_out = wait_child(&mut child, &[], deadline, POLL, || false).unwrap();

        assert_eq!((ready, timed_out), (Waited::Ready, Waited::TimedOut));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
