//! The KM4 wall clock ([`wellfed_core::clock::wall`]). Its writers are the
//! device API's `time` request and the loop's NTP exchange, and the loop
//! sets the zone rule from the time settings; the API's status, time view
//! and event stamps read it. One instance, copied in and out with
//! interrupts masked.
//!
//! **Across a warm reset** the clock is kept in `wellfed_time_carry`, RAM
//! a watchdog or system reset leaves alone, stamped with the KM0 RTC's
//! second: after every accepted fix and every few seconds from the loop
//! ([`keep`]), and cleared while the clock is invalid, so a reset never
//! finds a clock older than the last correction. The start hook resumes it
//! once ([`resume`]); whatever it finds, it then stores this boot's own.

use core::sync::atomic::{AtomicU32, Ordering};

use wellfed_core::clock::wall::{
    CARRY_WORDS, Fix, Jump, NoResume, Origin, Restart, State, Sync as ClockSync, SyncError,
    WallClock,
};
use wellfed_core::clock::zone::Rule;

use crate::cell::TaskCell;
use crate::sdk::{self, IrqGuard};
use crate::startup;

/// Every access is a short copy or update under `IrqGuard`.
static CLOCK: TaskCell<WallClock> = TaskCell::new(WallClock::new());

/// How often the loop stores the clock: its error grows at the RTC's
/// bound from the last store, 0.1 s over this.
const KEEP_MS: u32 = 10_000;
/// Uptime of the last store.
static KEPT_AT: AtomicU32 = AtomicU32::new(0);

const _: () = assert!(CARRY_WORDS == sdk::TIME_CARRY_WORDS);

fn update<T>(change: impl FnOnce(&mut WallClock) -> T) -> T {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    change(unsafe { &mut *CLOCK.get() })
}

/// Adopt a client's sync, as `origin`, and log it as a `PLTIM1` record.
pub fn sync(sync: ClockSync, origin: Origin) -> Result<Jump, SyncError> {
    let now = sdk::uptime_ms();
    let rtc = rtc_seconds();
    let result = update(|clock| {
        let result = clock.sync(now, sync, origin);
        store(clock, now, rtc);
        result
    });
    log(origin, result);
    result
}

/// Adopt an NTP fix measured now, and log it as a `PLTIM1` record.
pub fn fix(fix: Fix) -> Result<Jump, SyncError> {
    let now = sdk::uptime_ms();
    let rtc = rtc_seconds();
    let result = update(|clock| {
        let result = clock.fix(now, fix);
        store(clock, now, rtc);
        result
    });
    log(fix.origin, result);
    result
}

/// Resume the clock a warm reset left in `wellfed_time_carry`, once at
/// boot before anything reads the clock, and log what came of it as
/// `PLTIM1 carry`. `restart` comes from the ROM's reset flags: only a
/// watchdog or software reset kept the RTC's count.
pub fn resume(restart: Restart) {
    let now = sdk::uptime_ms();
    let record = sdk::time_carry_load();
    let rtc = rtc_seconds();
    let result = update(|clock| {
        let result = clock.resume(now, &record, rtc, restart);
        store(clock, now, rtc);
        result
    });

    let rtc = rtc.unwrap_or(u32::MAX);
    let mut report = startup::Report::new();
    let line = match result {
        Ok(resumed) => report.record(format_args!(
            "PLTIM1 carry result=resumed gap_s={} error_ms={} rtc_s={rtc} at_ms={now}\r\n",
            resumed.gap_s, resumed.error_ms
        )),
        Err(why) => report.record(format_args!(
            "PLTIM1 carry result={} rtc_s={rtc} at_ms={now}\r\n",
            no_resume_name(why)
        )),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
}

/// Store the clock for a warm reset if the last store is `KEEP_MS` old;
/// the loop calls it every pass.
pub fn keep() {
    let now = sdk::uptime_ms();
    if now.wrapping_sub(KEPT_AT.load(Ordering::Relaxed)) < KEEP_MS {
        return;
    }
    let rtc = rtc_seconds();
    update(|clock| store(clock, now, rtc));
}

/// Store the clock at uptime `now` and RTC second `rtc`, or clear the
/// record while it is invalid or the RTC unreadable.
fn store(clock: &WallClock, now: u32, rtc: Option<u32>) {
    let record = rtc.and_then(|rtc| clock.carry(now, rtc));
    sdk::time_carry_store(&record.unwrap_or([0; CARRY_WORDS]));
    KEPT_AT.store(now, Ordering::Relaxed);
}

/// The KM0 RTC's count now, in seconds; `None` if two reads in a row never
/// agree or it is not a 24-hour count. A read that straddles a tick can mix
/// the old and new digits, so a value counts only when read twice.
pub fn rtc_seconds() -> Option<u32> {
    let [first, cr, ..] = sdk::rtc_registers();
    let [second, ..] = sdk::rtc_registers();
    let tr = if first == second {
        first
    } else {
        let [third, ..] = sdk::rtc_registers();
        (third == second).then_some(third)?
    };
    rtc_count(tr, cr)
}

/// Seconds in the RTC's time register: a day count in bits 23-31 and BCD
/// hours, minutes and seconds. `None` in the 12-hour format (`CR` bit 7),
/// with the PM flag, or with a digit out of range. Nothing sets the RTC, so
/// it counts from its power-on reset value: seconds since power-on.
pub fn rtc_count(tr: u32, cr: u32) -> Option<u32> {
    const TWELVE_HOUR: u32 = 1 << 7;
    const PM: u32 = 1 << 22;
    if cr & TWELVE_HOUR != 0 || tr & PM != 0 {
        return None;
    }

    let digit = |shift: u32, bits: u32| (tr >> shift) & ((1 << bits) - 1);
    let bcd = |tens: u32, units: u32, below: u32| {
        (units <= 9 && tens * 10 + units < below).then_some(tens * 10 + units)
    };
    let seconds = bcd(digit(4, 3), digit(0, 4), 60)?;
    let minutes = bcd(digit(12, 3), digit(8, 4), 60)?;
    let hours = bcd(digit(20, 2), digit(16, 4), 24)?;
    let days = tr >> 23;
    Some(((days * 24 + hours) * 60 + minutes) * 60 + seconds)
}

/// A record word for each reason a saved clock was not resumed.
const fn no_resume_name(why: NoResume) -> &'static str {
    match why {
        NoResume::Set => "set",
        NoResume::Cold => "cold",
        NoResume::Empty => "empty",
        NoResume::Corrupt => "corrupt",
        NoResume::Rtc => "rtc",
        NoResume::Backwards => "backwards",
        NoResume::Stale => "stale",
        NoResume::Expired => "expired",
    }
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
