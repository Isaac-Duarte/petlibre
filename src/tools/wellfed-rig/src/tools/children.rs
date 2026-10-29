//! The child processes a bench job has running, as one registry the bench
//! server reads for its live state and signals on a cancel.
//!
//! Run from a terminal, a Ctrl+C reached every child at once through the
//! terminal's process group. The bench server's jobs have no terminal, so a
//! cancel signals each registered child itself: SIGINT to `wellfed-flash`,
//! which then stops its session terminally and records it, and SIGTERM to
//! the rest. The flash client is never sent SIGTERM or SIGKILL by a cancel:
//! either would end it without that record, possibly mid-write.

use std::process::Child;
use std::sync::{Arc, Mutex, MutexGuard};

use rustix::process::{Pid, Signal, kill_process};
use wellfed_formats::bench::live::{Activity, ActivityKind};
use wellfed_formats::bench::now;

/// Told the whole list each time a child starts or is reaped. It runs with
/// the registry locked, so lists arrive in order; it must not call back in.
type Watcher = Box<dyn Fn(&[Activity]) + Send + Sync>;

/// Shared by every clone: a job's tools and its suite register into the
/// same one.
#[derive(Clone, Default)]
pub struct Children {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    running: Mutex<Vec<Activity>>,
    watcher: Mutex<Option<Watcher>>,
}

impl Children {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell `watcher` of every change from now on, starting with the list
    /// as it is.
    pub fn watch(&self, watcher: impl Fn(&[Activity]) + Send + Sync + 'static) {
        let running = self.running_list();
        watcher(&running);
        *lock(&self.shared.watcher) = Some(Box::new(watcher));
    }

    /// The children registered and not yet reaped.
    pub fn running(&self) -> Vec<Activity> {
        self.running_list().clone()
    }

    /// Register `child` until the returned guard drops, which its owner does
    /// once the child is reaped. `label` is its subcommand words.
    pub fn track(&self, child: &Child, kind: ActivityKind, label: &str) -> Tracked {
        let pid = child.id();
        let mut running = self.running_list();
        running.push(Activity {
            kind,
            pid,
            label: label.to_string(),
            started: now(),
        });
        self.notify(&running);
        Tracked {
            children: self.clone(),
            pid,
        }
    }

    /// Signal every registered child as a cancel stops it: SIGINT to the
    /// flash client, SIGTERM to the others. Nothing is waited on here; each
    /// owner reaps its child through its own cleanup.
    pub fn interrupt(&self) {
        let running = self.running_list();
        for child in running.iter() {
            let signal = match child.kind {
                ActivityKind::Flash => Signal::INT,
                ActivityKind::Build | ActivityKind::Stock | ActivityKind::Analyzer => Signal::TERM,
            };
            let Some(pid) = i32::try_from(child.pid).ok().and_then(Pid::from_raw) else {
                continue;
            };
            // Owners drop a child's entry as soon as they reap it, so a
            // listed pid is still that child's (exited at worst, never
            // reused) but for the instant between the two.
            let _ = kill_process(pid, signal);
        }
    }

    fn running_list(&self) -> MutexGuard<'_, Vec<Activity>> {
        lock(&self.shared.running)
    }

    fn notify(&self, running: &[Activity]) {
        if let Some(watcher) = &*lock(&self.shared.watcher) {
            watcher(running);
        }
    }
}

/// A child's place in the registry, held beside the child and dropped once
/// it is reaped.
pub struct Tracked {
    children: Children,
    pid: u32,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let mut running = self.children.running_list();
        running.retain(|child| child.pid != self.pid);
        self.children.notify(&running);
    }
}

/// The lists are plain data, so one a panicking holder left is still whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn the_watcher_sees_each_child_start_and_go() {
        let children = Children::new();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        children.watch(move |list| {
            record
                .lock()
                .unwrap()
                .push(list.iter().map(|c| c.label.clone()).collect::<Vec<_>>());
        });

        let mut child = Command::new("true").spawn().unwrap();
        let tracked = children.track(&child, ActivityKind::Build, "build firstboot");
        assert_eq!(children.running()[0].pid, child.id());
        child.wait().unwrap();
        drop(tracked);

        assert_eq!(
            *seen.lock().unwrap(),
            [vec![], vec!["build firstboot".to_string()], vec![]]
        );
        assert!(children.running().is_empty());
    }

    /// A shell that records which signal reached it, then exits.
    fn trapping(dir: &std::path::Path) -> Child {
        let script = format!(
            "trap 'echo INT > {0}/got; kill $!; exit 1' INT; \
             trap 'echo TERM > {0}/got; kill $!; exit 1' TERM; \
             sleep 30 & echo ready > {0}/ready; wait $!",
            dir.display()
        );
        let child = Command::new("sh").arg("-c").arg(script).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.join("ready").exists() {
            assert!(Instant::now() < deadline, "the shell never started");
            std::thread::sleep(Duration::from_millis(10));
        }
        child
    }

    #[test]
    fn a_cancel_interrupts_the_flash_client_and_terminates_the_rest() {
        let (flash_dir, build_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let children = Children::new();
        let mut flash = trapping(flash_dir.path());
        let mut build = trapping(build_dir.path());
        let _flash = children.track(&flash, ActivityKind::Flash, "update execute");
        let _build = children.track(&build, ActivityKind::Build, "build firstboot");

        children.interrupt();
        flash.wait().unwrap();
        build.wait().unwrap();

        let got = |dir: &std::path::Path| std::fs::read_to_string(dir.join("got")).unwrap();
        assert_eq!(got(flash_dir.path()).trim(), "INT");
        assert_eq!(got(build_dir.path()).trim(), "TERM");
    }
}
