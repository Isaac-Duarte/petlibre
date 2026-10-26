//! The silent-reset breadcrumb: its register encoding, how the next boot
//! reads a hang from it, and the tick hook that writes it.

use std::sync::{Mutex, MutexGuard};

use wellfed_hal::sdk::fake;
use wellfed_hal::supervision::crumb::{
    self, Crumb, REGISTER, SINCE_STEP_MS, SPIN_MS, Stall, wellfed_tick_hook,
};

/// The crumb's counters are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    crumb::reset();
    guard
}

fn register() -> u32 {
    fake::with(|h| h.backup[REGISTER as usize])
}

fn tick(name: &core::ffi::CStr, suspended: bool) {
    // SAFETY: a NUL-terminated name, as the shim passes.
    unsafe { wellfed_tick_hook(name.as_ptr().cast(), suspended.into()) };
}

fn ticks(name: &core::ffi::CStr, count: u32) {
    for _ in 0..count {
        tick(name, false);
    }
}

#[test]
fn a_crumb_round_trips_through_its_register_word() {
    for crumb in [
        Crumb::new(b"wellfed_wifi", false, false, 3_968),
        Crumb::new(b"wellfed-wdg", true, true, 0),
        Crumb::new(b"wellfed", false, false, 128),
        Crumb::new(b"IDLE", false, false, 5_300),
        Crumb::new(b"Tmr Svc", true, false, 250),
        Crumb::new(b"", false, false, 0),
    ] {
        assert_eq!(Crumb::decode(crumb.encode()), Some(crumb), "{crumb:?}");
    }
}

#[test]
fn the_name_keeps_three_characters_after_the_shared_prefix() {
    let wifi = Crumb::new(b"wellfed_wifi", false, false, 0);
    assert!(wifi.prefixed);
    assert_eq!(&wifi.name, b"_wi");

    // Vendor tasks keep their first characters, folded to lower case; a
    // space ends the name.
    let timer = Crumb::new(b"Tmr Svc", false, false, 0);
    assert!(!timer.prefixed);
    assert_eq!(&timer.name, b"tmr");
    assert_eq!(&Crumb::new(b"A B", false, false, 0).name, b"a\0\0");

    // The diagnostic task is the prefix alone.
    let diagnostic = Crumb::new(b"wellfed", false, false, 0);
    assert!(diagnostic.prefixed);
    assert_eq!(diagnostic.name, [0; 3]);
}

#[test]
fn since_is_kept_in_steps_and_saturates_past_the_longest_expiry() {
    assert_eq!(Crumb::new(b"x", false, false, 127).since_ms, 0);
    assert_eq!(
        Crumb::new(b"x", false, false, 3_999).since_ms,
        31 * SINCE_STEP_MS
    );
    let longest = Crumb::new(b"x", false, false, u32::MAX).since_ms;
    assert_eq!(longest, 63 * SINCE_STEP_MS);
    assert!(longest > 5_300, "past the watchdog's longest expiry");
}

#[test]
fn zero_and_stray_values_are_no_crumb() {
    // A cold boot clears the backup registers.
    assert_eq!(Crumb::decode(0), None);

    let word = Crumb::new(b"wellfed-svc", false, false, 1_000).encode();
    // A wrong tag, or any flipped bit the check covers.
    assert_eq!(Crumb::decode(word & !(1 << 31)), None);
    for bit in 2..29 {
        assert_eq!(Crumb::decode(word ^ (1 << bit)), None, "bit {bit}");
    }
    // An older KM0's park diagnostics: entries, channel 16, an event.
    assert_eq!(Crumb::decode((7 << 16) | (16 << 8) | 2), None);
}

#[test]
fn the_crumb_tells_a_spin_from_masked_interrupts_and_a_deliberate_stop() {
    // The tick ran on long after the feeder last did: a task kept the CPU.
    let spin = Crumb::new(b"wellfed_wifi", false, false, 3_968);
    assert_eq!(spin.stall(), Stall::Spin);
    assert_eq!(
        spin.to_string(),
        "kind=spin task=wellfed_wi since_ms=3968 suspended=0"
    );

    // The tick stopped inside a feed period: interrupts were masked.
    let masked = Crumb::new(b"TCP_IP", false, false, 200);
    assert_eq!(masked.stall(), Stall::Masked);
    assert_eq!(
        masked.to_string(),
        "kind=masked task=tcp since_ms=128 suspended=0"
    );
    assert_eq!(
        Crumb::new(b"x", false, false, SPIN_MS - 1).stall(),
        Stall::Masked
    );
    assert_eq!(Crumb::new(b"x", false, false, SPIN_MS).stall(), Stall::Spin);

    // The feeder stopped on purpose: whatever ran after, the reset is its own.
    let stopped = Crumb::new(b"IDLE", false, true, 3_968);
    assert_eq!(stopped.stall(), Stall::Stopped);
    assert_eq!(
        stopped.to_string(),
        "kind=stopped task=idl since_ms=3968 suspended=0"
    );

    let nameless = Crumb::new(b"", true, false, 0);
    assert_eq!(
        nameless.to_string(),
        "kind=masked task=none since_ms=0 suspended=1"
    );
}

#[test]
fn the_tick_hook_counts_ticks_since_the_feeder_ran() {
    let _guard = start();

    ticks(c"wellfed-wdg", 100);
    crumb::fed();
    ticks(c"wellfed_wifi", 3_000);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!(&crumb.name, b"_wi");
    assert!(crumb.prefixed && !crumb.suspended && !crumb.stopping);
    assert_eq!(crumb.since_ms, 3_000 / SINCE_STEP_MS * SINCE_STEP_MS);
    assert_eq!(crumb.stall(), Stall::Spin);

    // The feeder ran again: the count starts over.
    crumb::fed();
    tick(c"IDLE", true);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!((crumb.since_ms, crumb.suspended), (0, true));
    assert_eq!(crumb.stall(), Stall::Masked);
}

#[test]
fn a_deliberate_stop_marks_every_later_tick() {
    let _guard = start();
    tick(c"wellfed-svc", false);
    assert!(!Crumb::decode(register()).is_some_and(|c| c.stopping));

    crumb::stopping();
    ticks(c"IDLE", 4_000);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert!(crumb.stopping);
    assert_eq!(crumb.stall(), Stall::Stopped);
}

#[test]
fn a_null_name_and_a_long_one_are_read_safely() {
    let _guard = start();
    // SAFETY: null is allowed.
    unsafe { wellfed_tick_hook(core::ptr::null(), 0) };
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!((crumb.prefixed, crumb.name), (false, [0; 3]));

    tick(c"wellfed-a-very-long-task-name", false);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!((crumb.prefixed, &crumb.name), (true, b"-a-"));
}

#[test]
fn the_boot_takes_the_crumb_once() {
    let _guard = start();
    assert_eq!(crumb::take(), None, "a cold boot has none");

    ticks(c"wellfed_wifi", 4_000);
    let left = Crumb::decode(register());
    assert!(left.is_some());
    assert_eq!(crumb::take(), left);
    assert_eq!(register(), 0);
    assert_eq!(crumb::take(), None, "reported once");
}

#[test]
fn the_other_backup_registers_are_left_alone() {
    let _guard = start();
    fake::with(|h| {
        for (index, value) in h.backup.iter_mut().enumerate() {
            *value = 0x1111_1111 * u32::try_from(index).unwrap();
        }
    });
    let before = fake::with(|h| h.backup);

    ticks(c"wellfed_wifi", 10);
    let _ = crumb::take();
    ticks(c"IDLE", 1);

    let after = fake::with(|h| h.backup);
    for index in (0..8).filter(|&i| i != REGISTER as usize) {
        assert_eq!(after[index], before[index], "register {index}");
    }
}
