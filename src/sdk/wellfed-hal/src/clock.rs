//! The KM4 wall clock ([`wellfed_core::clock::wall`]). Its writers are the
//! device API's `time` request and the loop's NTP exchange, and the loop
//! sets the zone rule from the time settings; the API's status, time view
//! and event stamps read it. One instance, copied in and out with
//! interrupts masked.

use wellfed_core::clock::wall::{
    Fix, Jump, Origin, State, Sync as ClockSync, SyncError, WallClock,
};
use wellfed_core::clock::zone::Rule;

use crate::cell::TaskCell;
use crate::sdk::{self, IrqGuard};
use crate::startup;

/// Every access is a short copy or update under `IrqGuard`.
static CLOCK: TaskCell<WallClock> = TaskCell::new(WallClock::new());

fn update<T>(change: impl FnOnce(&mut WallClock) -> T) -> T {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    change(unsafe { &mut *CLOCK.get() })
}

/// Adopt a client's sync, as `origin`, and log it as a `PLTIM1` record.
pub fn sync(sync: ClockSync, origin: Origin) -> Result<Jump, SyncError> {
    let now = sdk::uptime_ms();
    let result = update(|clock| clock.sync(now, sync, origin));
    log(origin, result);
    result
}

/// Adopt an NTP fix measured now, and log it as a `PLTIM1` record.
pub fn fix(fix: Fix) -> Result<Jump, SyncError> {
    let now = sdk::uptime_ms();
    let result = update(|clock| clock.fix(now, fix));
    log(fix.origin, result);
    result
}

fn log(origin: Origin, result: Result<Jump, SyncError>) {
    let now = sdk::uptime_ms();
    let error_ms = state().reading.map_or(0, |reading| reading.error_ms);
    let mut report = startup::Report::new();
    let line = match result {
        Ok(jump) => {
            let (kind, delta) = match jump {
                Jump::First => ("first", 0),
                Jump::Step(delta) => ("step", delta),
                Jump::Jumped(delta) => ("jump", delta),
            };
            report.record(format_args!(
                "PLTIM1 sync ok=1 source={} kind={kind} delta_ms={delta} error_ms={error_ms} at_ms={now}\r\n",
                origin_name(origin)
            ))
        }
        Err(error) => report.record(format_args!(
            "PLTIM1 sync ok=0 source={} error={}\r\n",
            origin_name(origin),
            name(error)
        )),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
}

/// A record word for each origin.
pub const fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::Owner => "owner",
        Origin::Client => "client",
        Origin::Ntp => "ntp",
    }
}

fn name(error: SyncError) -> &'static str {
    match error {
        SyncError::Time => "time",
        SyncError::Offset => "offset",
        SyncError::NextChange => "next_change",
        SyncError::Bounded => "bounded",
    }
}

/// Set or clear the zone's rule.
pub fn set_rule(rule: Option<Rule>) {
    update(|clock| clock.set_rule(rule));
}

/// The clock's state now: its reading (`None` while invalid), the learned
/// rate and the last fix.
pub fn state() -> State {
    let now = sdk::uptime_ms();
    let clock = update(|clock| *clock);
    clock.state(now)
}

/// Log the KM0 RTC's registers as `PLTIM1 rtc`, once at boot: whether it
/// runs (`ISR` bit 4, the calendar initialized), and its count (`TR`: day
/// count in bits 23-31, then BCD hours, minutes and seconds). The firmware
/// never sets it; this is the evidence for whether a warm reset keeps it.
pub fn log_rtc() {
    let [tr, cr, isr, prer] = sdk::rtc_registers();
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLTIM1 rtc tr={tr:08x} cr={cr:08x} isr={isr:08x} prer={prer:08x} at_ms={}\r\n",
        sdk::uptime_ms()
    )) {
        sdk::log(line);
    }
}

/// Forget every sync (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    update(|clock| *clock = WallClock::new());
}
