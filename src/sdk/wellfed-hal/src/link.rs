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
//!
//! **Before an intentional restart** the runtime calls
//! [`leave_for_restart`]. An intentional restart sends the access point
//! nothing, so it still holds the old boot's protected association and
//! rejects the next boot's with status 30, and the vendor's `wifi_connect`
//! can then hang the whole KM4 into a silent watchdog reset (observed on
//! the bench). So the next watch answers [`Cause::Restart`] whatever
//! the link shows, and the shim disconnects and never joins again; it
//! checks [`wellfed_wifi_restarting`] between its waits, so the leave
//! starts within one of them rather than at the next watch tick. The
//! restart itself never waits for this: the watchdog feeder stops at
//! once, and the leave runs inside the watchdog's expiry or not at all.
//!
//! **Battery mode** asks for the radio off ([`set_radio`]): the next watch
//! answers [`Cause::Radio`], and the shim leaves as it does for a restart,
//! then turns the radio off (`wifi_off`) and waits, checking
//! [`wellfed_wifi_radio`] between its polls, until it is asked on again:
//! then `wifi_on` and a fresh join. A shim between attempts checks it too,
//! so it neither scans nor joins while the radio should be off. A restart
//! wins over both.

use core::sync::atomic::{AtomicBool, Ordering};

use wellfed_core::liveness::{Cause, Liveness, Probe, Radio, Verdict};

use crate::cell::TaskCell;
use crate::{net_status, sdk};

/// Set once, by the restart; nothing clears it but the reset.
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// The feeder is about to restart: leave the network for good.
pub fn leave_for_restart() {
    RESTARTING.store(true, Ordering::Release);
}

/// Nonzero once a restart asked the link to leave: the shim stops joining,
/// and leaves a joined link at once.
#[unsafe(no_mangle)]
pub extern "C" fn wellfed_wifi_restarting() -> i32 {
    i32::from(RESTARTING.load(Ordering::Acquire))
}

/// Set while battery mode wants the radio off; the boot starts on.
static RADIO_OFF: AtomicBool = AtomicBool::new(false);

/// The loop's wish for the radio, every pass; the Wi-Fi task acts on it.
pub fn set_radio(radio: Radio) {
    RADIO_OFF.store(radio == Radio::Off, Ordering::Release);
}

/// Whether battery mode wants the radio off.
pub fn radio_off() -> bool {
    RADIO_OFF.load(Ordering::Acquire)
}

/// 1 while the radio should be up, 0 while battery mode wants it off.
#[unsafe(no_mangle)]
pub extern "C" fn wellfed_wifi_radio() -> i32 {
    i32::from(!RADIO_OFF.load(Ordering::Acquire))
}

/// Host tests only: back to the boot state.
#[cfg(not(target_os = "none"))]
pub fn reset() {
    RESTARTING.store(false, Ordering::Release);
    RADIO_OFF.store(false, Ordering::Release);
}

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
/// [`Verdict::code`]: hold, probe now and call again, settled (reset the
/// rejoin backoff), or leave with a cause; once a restart was asked for,
/// always a leave for [`Cause::Restart`], and while the radio should be
/// off, one for [`Cause::Radio`].
///
/// # Safety
/// Called only from the Wi-Fi task, between a successful join's
/// `wellfed_wifi_outcome` and its leave.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wellfed_wifi_watch(associated: i32, leased: i32, probe: i32) -> i32 {
    if RESTARTING.load(Ordering::Acquire) {
        net_status::set_live(false);
        return Verdict::Leave(Cause::Restart).code();
    }
    if RADIO_OFF.load(Ordering::Acquire) {
        net_status::set_live(false);
        return Verdict::Leave(Cause::Radio).code();
    }

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
