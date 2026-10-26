//! The KM4 wall clock ([`wellfed_core::clock::wall`]). The device API's
//! `time` request is its only writer; the API's status and event stamps
//! read it. One instance, copied in and out with interrupts masked.

use wellfed_core::clock::wall::{Jump, Sync as ClockSync, SyncError, WallClock};

use crate::cell::TaskCell;
use crate::sdk::{self, IrqGuard};
use crate::startup;

/// A valid wall-clock reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockHealth {
    pub unix_ms: u64,
    pub offset_s: i32,
    pub uncertainty_ms: u64,
}

/// Every access is a short copy or update under `IrqGuard`.
static CLOCK: TaskCell<WallClock> = TaskCell::new(WallClock::new());

/// Adopt a sync from the server and log it as a `PLTIM1` record.
pub fn sync(sync: ClockSync) -> Result<Jump, SyncError> {
    let now = sdk::uptime_ms();
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
                sync.offset_s, sync.next_change_s
            ))
        }
        Err(error) => report.record(format_args!("PLTIM1 sync ok=0 error={}\r\n", name(error))),
    };
    if let Ok(line) = line {
        sdk::log(line);
    }
    result
}

fn name(error: SyncError) -> &'static str {
    match error {
        SyncError::Time => "time",
        SyncError::Offset => "offset",
        SyncError::NextChange => "next_change",
    }
}

/// The current reading, `None` while invalid, and the sync count.
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
