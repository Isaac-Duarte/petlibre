//! The bench's read-only page: the feeder, bench progress, the integration
//! suite's runs, and the evidence sessions.
//!
//! The views read the progress document, the evidence directories and the
//! local server's status only. The crate links no relay driver and no flash
//! client, so nothing here can switch a relay or open a serial port: it
//! cannot disturb an update, a dump or a capture, and nothing writes. The
//! `wellfed-dashboard` binary serves it alone; `wellfed-bench --ui` serves
//! it beside a command. The views always build, so their tests and
//! TypeScript bindings run in the contributor gate; the server and its
//! embedded web app need the `ui` feature and a built
//! `src/web/apps/bench/dist` (`make ui`).

#![forbid(unsafe_code)]

#[cfg(any(feature = "ui", test))]
mod feeder;
#[cfg(any(feature = "ui", test))]
mod runs;
#[cfg(any(feature = "ui", test))]
mod views;

#[cfg(feature = "ui")]
mod serve;
#[cfg(feature = "ui")]
pub use serve::{Listen, start};

#[cfg(test)]
mod bindings;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("--listen needs a literal IP address")]
    Listen,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Whether a recorded process still exists (one owned by another user counts).
#[cfg(any(feature = "ui", test))]
fn alive(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(error) => error == rustix::io::Errno::PERM,
    }
}

/// Recorded as running by a process that still exists.
#[cfg(any(feature = "ui", test))]
fn live(current: &wellfed_formats::bench::progress::Current) -> bool {
    current.outcome == wellfed_formats::bench::progress::Outcome::Running && alive(current.pid)
}
