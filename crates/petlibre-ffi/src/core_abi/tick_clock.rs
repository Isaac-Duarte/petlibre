//! Exports declared by `tick_clock.h`.

use petlibre_core::tick_clock::TickClock;

use super::store;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_tick_clock_init(
    clock: *mut TickClock,
    ticks_per_second: u32,
    raw: u32,
    initial_ms: u32,
) -> bool {
    store(
        // SAFETY: the C contract: `clock` is null or valid for the call.
        unsafe { clock.as_mut() },
        TickClock::new(ticks_per_second, raw, initial_ms),
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn petlibre_tick_clock_read(
    clock: *mut TickClock,
    raw: u32,
    now_ms: *mut u32,
) -> bool {
    // SAFETY: the C contract: `clock` is null or valid for the call.
    let Some(clock) = (unsafe { clock.as_mut() }) else {
        return false;
    };

    // SAFETY: the C contract: `now_ms` is null or valid for the call.
    match unsafe { now_ms.as_mut() } {
        Some(now) => store(Some(now), clock.read(raw)),
        None => {
            if clock.valid {
                clock.invalidate();
            }
            false
        }
    }
}
