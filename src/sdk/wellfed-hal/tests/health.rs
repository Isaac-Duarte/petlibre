//! The health snapshot's boot facts, read from the fake's registers.

use wellfed_hal::health::Boot;
use wellfed_hal::sdk::fake;
use wellfed_hal::startup::Snapshot;

#[test]
fn the_boot_count_leaves_out_the_trials_bits() {
    // KM0 keeps its boot count in bits 0-15 of register 4 and the trial's
    // state above it; a trial that confirmed leaves those set.
    fake::with(|h| h.backup[4] = 0x9701_0003);
    let snapshot = Snapshot {
        control: 0,
        start: 0,
        offset: 0,
        reset: 0,
    };

    assert_eq!(Boot::read(snapshot).boot_count, 3);
}
