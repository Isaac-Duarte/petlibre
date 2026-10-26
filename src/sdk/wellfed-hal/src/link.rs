//! The C Wi-Fi task's watch over a joined link.
//!
//! The shim samples the link every watch tick (whether it is still
//! associated, whether lwIP still holds the leased address) and calls
//! `wellfed_wifi_watch` with the sample and the result of any gateway
//! probe it ran since. The decision is [`Liveness`], host-tested in
//! `wellfed-core`; this module only carries one join's machine between
//! calls and publishes whether the link is live to [`net_status`], which
//! trial confirmation reads. A successful join (the join outcome the C task
//! reports, `wellfed_wifi_outcome`) starts a new machine.

use wellfed_core::liveness::{Liveness, Probe};

use crate::cell::TaskCell;
use crate::{net_status, sdk};

/// Touched only by the C Wi-Fi task, through `wellfed_wifi_watch` and
/// `wellfed_wifi_outcome`, which it never runs at once.
static WATCH: TaskCell<Liveness> = TaskCell::new(Liveness::new());

/// A join got an address: watch it from scratch, not live until it settles.
///
/// # Safety
/// Called only from the Wi-Fi task, which owns `WATCH`.
pub unsafe fn joined() {
    // SAFETY: the caller's contract makes this the only reference to `WATCH`.
    unsafe { *WATCH.get() = Liveness::new() };
    net_status::set_live(false);
}

/// Every watch tick while joined: `associated` and `leased` are nonzero
/// while they hold, `probe` is [`Probe::from_code`] of the gateway probe
/// the previous call asked for (0 when none ran). The answer is
/// [`wellfed_core::liveness::Verdict::code`]: hold, probe now and call again, settled (reset the
/// rejoin backoff), or leave with a cause.
///
/// # Safety
/// Called only from the Wi-Fi task, between a successful join's
/// `wellfed_wifi_outcome` and its leave.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_wifi_watch(associated: i32, leased: i32, probe: i32) -> i32 {
    // SAFETY: the caller's contract makes this the only reference to `WATCH`.
    let watch = unsafe { &mut *WATCH.get() };

    let verdict = watch.step(
        sdk::uptime_ms(),
        associated != 0,
        leased != 0,
        Probe::from_code(probe),
    );
    net_status::set_live(watch.live());

    verdict.code()
}
