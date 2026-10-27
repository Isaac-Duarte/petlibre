//! The KM4 wall clock ([`wellfed_core::clock::wall`]). Its writers are the
//! device API's `time` request and the loop's NTP exchange, and the loop
//! sets the zone rule from the time settings; the API's status, time view
//! and event stamps read it. One instance, copied in and out with
//! interrupts masked.
//!
//! **Its milliseconds** come from the SysTimer ([`now_ms`]), not from the
//! scheduler's tick: every flash lease stops SysTick with interrupts masked
//! for the length of an erase (about 45 ms), so the tick count falls behind
//! by each one, about 11 s across a full bank's download, while the clock
//! would grow its error only by the rate bound over the shortened count.
//! The SysTimer is a free-running 32 kHz counter that runs through masked
//! interrupts. It is extended here at every clock access, which the loop
//! makes every pass ([`keep`]); a read that finds it moved backwards or by
//! half its range is a count that lost its place, and the clock forgets
//! the time rather than project across it.
//!
//! **Across a warm reset** the clock is kept in `wellfed_time_carry`, RAM
//! a watchdog or system reset leaves alone, stamped with the KM0 RTC's
//! second: after every accepted fix and every few seconds from the loop
//! ([`keep`]), and cleared while the clock is invalid, so a reset never
//! finds a clock older than the last correction. The start hook resumes it
//! once ([`resume`]); whatever it finds, it then stores this boot's own.

use wellfed_core::clock::tick::TickClock;
use wellfed_core::clock::wall::{
    CARRY_WORDS, Fix, Jump, NoResume, Origin, Restart, State, Sync as ClockSync, SyncError,
    UNLEARNED_PPM, WallClock,
};
use wellfed_core::clock::zone::Rule;

use crate::cell::TaskCell;
use crate::sdk::{self, IrqGuard};
use crate::startup;
use crate::supervision::config::SYSTIMER_HZ;

/// The clock, its millisecond count, and that count at the last store.
struct Kept {
    clock: WallClock,
    count: TickClock,
    kept_at: u32,
    /// The count lost its place and the clock forgot the time: logged by
    /// the loop's next [`keep`].
    lost: Option<Lost>,
}

/// Where the count stood when it lost its place.
#[derive(Clone, Copy)]
struct Lost {
    count_ms: u32,
    /// The clock had the time, so forgetting it changed what it reads.
    had_time: bool,
}

const UNSET: Kept = Kept {
    clock: WallClock::new(),
    count: TickClock::UNSET,
    kept_at: 0,
    lost: None,
};

/// Every access is a short copy or update under `IrqGuard`.
static CLOCK: TaskCell<Kept> = TaskCell::new(UNSET);

/// How often the loop stores the clock: its error grows at the RTC's
/// bound from the last store, 0.1 s over this.
const KEEP_MS: u32 = 10_000;

const _: () = assert!(CARRY_WORDS == sdk::TIME_CARRY_WORDS);

/// Run `change` on the clock at its millisecond count now, with interrupts
/// masked; it also gets the count at the last store.
fn update<T>(change: impl FnOnce(&mut WallClock, u32, &mut u32) -> T) -> T {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    let kept = unsafe { &mut *CLOCK.get() };
    let now = count(kept);
    change(&mut kept.clock, now, &mut kept.kept_at)
}

/// `PLTIM1 forget reason=count`, from [`keep`] at the top of the loop's
/// pass rather than from whichever reader found the count lost: a report
/// frame there would sit on the deepest stack path (a time view inside a
/// session's answer).
#[cold]
#[inline(never)]
fn log_lost(lost: Lost) {
    let mut report = startup::Report::new();
    if let Ok(line) = report.record(format_args!(
        "PLTIM1 forget reason=count had_time={} at_ms={} count_ms={}\r\n",
        u8::from(lost.had_time),
        sdk::uptime_ms(),
        lost.count_ms
    )) {
        sdk::log(line);
    }
}

/// The SysTimer extended to milliseconds. The first read starts the count
/// at the tick count, so early records read close to uptime; a read the
/// extension refuses starts it again from where it stood and forgets the
/// time and the record a reset would resume, since how long the count was
/// lost is unknown (`PLTIM1 forget reason=count` from the next [`keep`],
/// so a later `empty` carry has its explanation in the log).
fn count(kept: &mut Kept) -> u32 {
    let raw = sdk::systimer_ticks();
    if let Some(now) = kept.count.read(raw) {
        return now;
    }

    let from = if kept.count.ticks_per_second == 0 {
        sdk::uptime_ms()
    } else {
        let count_ms = kept.count.millis;
        kept.lost = Some(Lost {
            count_ms,
            had_time: kept.clock.read(count_ms).is_some(),
        });
        kept.clock.forget();
        sdk::time_carry_store(&[0; CARRY_WORDS]);
        count_ms
    };
    kept.count = TickClock::new(SYSTIMER_HZ, raw, from).unwrap_or_default();
    from
}

/// The clock's millisecond count now. The NTP exchange times its round
/// trip with it, so an erase during the exchange counts in its error.
pub fn now_ms() -> u32 {
    update(|_, now, _| now)
}

/// Adopt a client's sync, as `origin`, and log it as a `PLTIM1` record.
pub fn sync(sync: ClockSync, origin: Origin) -> Result<Jump, SyncError> {
    let rtc = rtc_seconds();
    let (result, now) = update(|clock, now, kept_at| {
        let result = clock.sync(now, sync, origin);
        store(clock, now, rtc, kept_at);
        (result, now)
    });
    log(origin, result, now);
    result
}

/// Adopt an NTP fix measured at count `at` ([`now_ms`]), and log it as a
/// `PLTIM1` record. It is applied now, moved on by the count since `at`
/// with that span's error at the unlearned bound, so the clock's anchors
/// stay in order even if a sync came in between, and the time between the
/// answer and this call is neither lost nor unaccounted for. A count `at`
/// ahead of now is not a measurement of this count: refused as `Time`.
pub fn fix(fix: Fix, at: u32) -> Result<Jump, SyncError> {
    let rtc = rtc_seconds();
    let (result, now) = update(|clock, now, kept_at| {
        let result = advance(fix, now.wrapping_sub(at)).and_then(|fix| clock.fix(now, fix));
        store(clock, now, rtc, kept_at);
        (result, now)
    });
    log(fix.origin, result, now);
    result
}

/// `fix` carried `elapsed_ms` of the clock's count on.
fn advance(fix: Fix, elapsed_ms: u32) -> Result<Fix, SyncError> {
    if elapsed_ms >= wellfed_core::HALF_RANGE {
        return Err(SyncError::Time);
    }
    let grown = u64::from(elapsed_ms) * u64::from(UNLEARNED_PPM);
    let grown = u32::try_from(grown.div_ceil(1_000_000)).map_err(|_| SyncError::Time)?;
    Ok(Fix {
        unix_ms: fix.unix_ms + u64::from(elapsed_ms),
        error_ms: fix.error_ms.saturating_add(grown),
        origin: fix.origin,
    })
}

/// Resume the clock a warm reset left in `wellfed_time_carry`, once at
/// boot before anything reads the clock, and log what came of it as
/// `PLTIM1 carry`. `restart` comes from the ROM's reset flags: only a
/// watchdog or software reset kept the RTC's count.
pub fn resume(restart: Restart) {
    let record = sdk::time_carry_load();
    let rtc = rtc_seconds();
    let (result, now) = update(|clock, now, kept_at| {
        let result = clock.resume(now, &record, rtc, restart);
        store(clock, now, rtc, kept_at);
        (result, now)
    });

    let rtc = rtc.unwrap_or(u32::MAX);
    let at = sdk::uptime_ms();
    let mut report = startup::Report::new();
    let line = match result {
        Ok(resumed) => report.record(format_args!(
            "PLTIM1 carry result=resumed gap_s={} error_ms={} rtc_s={rtc} at_ms={at} count_ms={now}\r\n",
            resumed.gap_s, resumed.error_ms
        )),
        Err(why) => report.record(format_args!(
            "PLTIM1 carry result={} rtc_s={rtc} at_ms={at} count_ms={now}\r\n",
            no_resume_name(why)
        )),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
}

/// Store the clock for a warm reset if the last store is `KEEP_MS` old,
/// and log a count that lost its place since the last pass. The loop
/// calls it every pass, which also keeps the SysTimer's extension read
/// well inside its half range (about 18 h).
pub fn keep() {
    let (due, lost) = {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        let kept = unsafe { &mut *CLOCK.get() };
        let now = count(kept);
        (now.wrapping_sub(kept.kept_at) >= KEEP_MS, kept.lost.take())
    };
    if let Some(lost) = lost {
        log_lost(lost);
    }

    if !due {
        return;
    }
    let rtc = rtc_seconds();
    update(|clock, now, kept_at| store(clock, now, rtc, kept_at));
}

/// Store the clock at count `now` and RTC second `rtc`, or clear the
/// record while it is invalid or the RTC unreadable.
fn store(clock: &WallClock, now: u32, rtc: Option<u32>, kept_at: &mut u32) {
    let record = rtc.and_then(|rtc| clock.carry(now, rtc));
    sdk::time_carry_store(&record.unwrap_or([0; CARRY_WORDS]));
    *kept_at = now;
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

/// `at_ms` is uptime, as in every record; `count_ms` is the clock's own
/// count, which runs ahead of it by the ticks flash erases stopped.
fn log(origin: Origin, result: Result<Jump, SyncError>, count_ms: u32) {
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
                "PLTIM1 sync ok=1 source={} kind={kind} delta_ms={delta} error_ms={error_ms} at_ms={now} count_ms={count_ms}\r\n",
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
    update(|clock, _, _| clock.set_rule(rule));
}

/// The clock's state now: its reading (`None` while invalid), the learned
/// rate and the last fix (at the clock's count, not uptime).
pub fn state() -> State {
    let (clock, now) = update(|clock, now, _| (*clock, now));
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

/// Forget every sync and the count, as a reset does (host tests).
#[cfg(not(target_os = "none"))]
pub fn reset() {
    let _guard = IrqGuard::lock();
    // SAFETY: interrupts are masked, so this is the only reference.
    let kept = unsafe { &mut *CLOCK.get() };
    *kept = UNSET;
}
