//! The KM4 wall clock ([`wellfed_core::clock::wall`]). The transport's
//! `time` request is its only writer; health reads it. One instance, copied
//! in and out with interrupts masked.

use wellfed_core::clock::wall::{Jump, Sync as ClockSync, SyncError, WallClock};
use wellfed_core::transport::{ClockHealth, TimeFields, TimeResult};

use crate::cell::TaskCell;
use crate::sdk::{self, IrqGuard};
use crate::startup;

/// Every access is a short copy or update under `IrqGuard`.
static CLOCK: TaskCell<WallClock> = TaskCell::new(WallClock::new());

/// Adopt a sync from the server and log it as a `PLTIM1` record.
pub fn sync(fields: TimeFields) -> TimeResult {
    let now = sdk::uptime_ms();
    let sync = ClockSync {
        unix_ms: fields.unix_ms,
        offset_s: fields.offset_s,
        next_change_s: fields.next_change_s,
        next_offset_s: fields.next_offset_s,
    };
    let result = {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { &mut *CLOCK.get() }.sync(now, sync)
    };

    let mut report = startup::Report::new();
    let line = match result {
        Ok(jump) => {
            let (kind, delta) = match jump {
                Jump::First => ("first", 0),
                Jump::Step(delta) => ("step", delta),
                Jump::Jumped(delta) => ("jump", delta),
            };
            report.record(format_args!(
                "PLTIM1 sync ok=1 kind={kind} delta_ms={delta} offset_s={} next_change_s={}\r\n",
                fields.offset_s, fields.next_change_s
            ))
        }
        Err(error) => report.record(format_args!("PLTIM1 sync ok=0 error={}\r\n", name(error))),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }

    match result {
        Ok(Jump::First) => TimeResult::Synced { jump_ms: None },
        Ok(Jump::Step(delta) | Jump::Jumped(delta)) => TimeResult::Synced {
            jump_ms: Some(delta),
        },
        Err(error) => TimeResult::Refused(name(error)),
    }
}

fn name(error: SyncError) -> &'static str {
    match error {
        SyncError::Time => "time",
        SyncError::Offset => "offset",
        SyncError::NextChange => "next_change",
    }
}

/// The current reading for health, `None` while invalid, and the sync count.
pub fn health() -> (Option<ClockHealth>, u32) {
    let now = sdk::uptime_ms();
    let clock = {
        let _guard = IrqGuard::lock();
        // SAFETY: interrupts are masked, so this is the only reference.
        unsafe { *CLOCK.get() }
    };
    let reading = clock.read(now).map(|r| ClockHealth {
        unix_ms: r.unix_ms,
        offset_s: r.offset_s,
        uncertainty_ms: r.uncertainty_ms,
    });
    (reading, clock.syncs())
}
