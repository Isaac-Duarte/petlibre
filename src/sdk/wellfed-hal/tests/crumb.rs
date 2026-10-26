//! The silent-reset breadcrumb: its register encoding, how the next boot
//! reads a hang from it, and the tick hook that writes it.

use std::sync::{Mutex, MutexGuard};

use wellfed_hal::sdk::fake;
use wellfed_hal::supervision::crumb::{
    self, Crumb, Feeder, REGISTER, SINCE_STEP_MS, SPIN_MS, Scheduler, Stall, wellfed_tick_hook,
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
        Crumb::new(b"wellfed_wifi", Scheduler::Running, Feeder::Feeding, 3_968),
        Crumb::new(b"wellfed-wdg", Scheduler::Suspended, Feeder::Stopping, 0),
        Crumb::new(b"wellfed", Scheduler::Running, Feeder::Feeding, 128),
        Crumb::new(b"IDLE", Scheduler::Running, Feeder::Feeding, 5_300),
        Crumb::new(b"Tmr Svc", Scheduler::Suspended, Feeder::Feeding, 250),
        Crumb::new(b"", Scheduler::Running, Feeder::Feeding, 0),
    ] {
        assert_eq!(Crumb::decode(crumb.encode()), Some(crumb), "{crumb:?}");
    }
}

#[test]
fn the_name_keeps_three_characters_after_the_shared_prefix() {
    let wifi = Crumb::new(b"wellfed_wifi", Scheduler::Running, Feeder::Feeding, 0);
    assert!(wifi.prefixed);
    assert_eq!(&wifi.name, b"_wi");

    // Vendor tasks keep their first characters, folded to lower case; a
    // space ends the name.
    let timer = Crumb::new(b"Tmr Svc", Scheduler::Running, Feeder::Feeding, 0);
    assert!(!timer.prefixed);
    assert_eq!(&timer.name, b"tmr");
    assert_eq!(
        &Crumb::new(b"A B", Scheduler::Running, Feeder::Feeding, 0).name,
        b"a\0\0"
    );

    // The diagnostic task is the prefix alone.
    let diagnostic = Crumb::new(b"wellfed", Scheduler::Running, Feeder::Feeding, 0);
    assert!(diagnostic.prefixed);
    assert_eq!(diagnostic.name, [0; 3]);
}

#[test]
fn since_is_kept_in_steps_and_saturates_past_the_longest_expiry() {
    assert_eq!(
        Crumb::new(b"x", Scheduler::Running, Feeder::Feeding, 127).since_ms,
        0
    );
    assert_eq!(
        Crumb::new(b"x", Scheduler::Running, Feeder::Feeding, 3_999).since_ms,
        31 * SINCE_STEP_MS
    );
    let longest = Crumb::new(b"x", Scheduler::Running, Feeder::Feeding, u32::MAX).since_ms;
    assert_eq!(longest, 63 * SINCE_STEP_MS);
    assert!(longest > 5_300, "past the watchdog's longest expiry");
}

#[test]
fn zero_and_stray_values_are_no_crumb() {
    // A cold boot clears the backup registers.
    assert_eq!(Crumb::decode(0), None);

    let word = Crumb::new(b"wellfed-svc", Scheduler::Running, Feeder::Feeding, 1_000).encode();
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
    let spin = Crumb::new(b"wellfed_wifi", Scheduler::Running, Feeder::Feeding, 3_968);
    assert_eq!(spin.stall(), Stall::Spin);
    assert_eq!(
        spin.to_string(),
        "kind=spin task=wellfed_wi since_ms=3968 suspended=0"
    );

    // The tick stopped inside a feed period: interrupts were masked.
    let masked = Crumb::new(b"TCP_IP", Scheduler::Running, Feeder::Feeding, 200);
    assert_eq!(masked.stall(), Stall::Masked);
    assert_eq!(
        masked.to_string(),
        "kind=masked task=tcp since_ms=128 suspended=0"
    );
    assert_eq!(
        Crumb::new(b"x", Scheduler::Running, Feeder::Feeding, SPIN_MS - 1).stall(),
        Stall::Masked
    );
    assert_eq!(
        Crumb::new(b"x", Scheduler::Running, Feeder::Feeding, SPIN_MS).stall(),
        Stall::Spin
    );

    // The feeder stopped on purpose: whatever ran after, the reset is its own.
    let stopped = Crumb::new(b"IDLE", Scheduler::Running, Feeder::Stopping, 3_968);
    assert_eq!(stopped.stall(), Stall::Stopped);
    assert_eq!(
        stopped.to_string(),
        "kind=stopped task=idl since_ms=3968 suspended=0"
    );

    let nameless = Crumb::new(b"", Scheduler::Suspended, Feeder::Feeding, 0);
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
    assert!(crumb.prefixed);
    assert_eq!(
        (crumb.scheduler, crumb.feeder),
        (Scheduler::Running, Feeder::Feeding)
    );
    assert_eq!(crumb.since_ms, 3_000 / SINCE_STEP_MS * SINCE_STEP_MS);
    assert_eq!(crumb.stall(), Stall::Spin);

    // The feeder ran again: the count starts over.
    crumb::fed();
    tick(c"IDLE", true);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!((crumb.since_ms, crumb.scheduler), (0, Scheduler::Suspended));
    assert_eq!(crumb.stall(), Stall::Masked);
}

#[test]
fn a_deliberate_stop_marks_every_later_tick() {
    let _guard = start();
    tick(c"wellfed-svc", false);
    assert!(!Crumb::decode(register()).is_some_and(|c| c.feeder == Feeder::Stopping));

    crumb::stopping();
    ticks(c"IDLE", 4_000);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!(crumb.feeder, Feeder::Stopping);
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

#[test]
fn a_supervision_stop_marks_the_crumb_and_leaves_the_network() {
    let _guard = start();
    wellfed_hal::link::reset();
    assert_eq!(wellfed_hal::link::wellfed_wifi_restarting(), 0);

    wellfed_hal::supervision::stopping();

    assert_eq!(wellfed_hal::link::wellfed_wifi_restarting(), 1);
    tick(c"IDLE", false);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!(crumb.feeder, Feeder::Stopping);
    wellfed_hal::link::reset();
}

#[test]
fn a_stop_is_certain_once_it_outlasts_the_progress_gap() {
    use wellfed_hal::supervision::stop_certain;

    assert!(!stop_certain(10_000, 10_000, 1_000));
    assert!(!stop_certain(10_000, 10_999, 1_000));
    assert!(stop_certain(10_000, 11_000, 1_000));
    // Across the uptime wrap.
    assert!(stop_certain(u32::MAX - 500, 500, 1_000));
    assert!(!stop_certain(u32::MAX - 500, 400, 1_000));
}

#[test]
fn a_transient_supervision_stop_keeps_the_network_and_the_crumb() {
    use wellfed_hal::link::{self, wellfed_wifi_restarting};
    use wellfed_hal::supervision::{self, config};

    let _guard = start();
    link::reset();
    supervision::reset();
    let gap = config::WATCHDOG.max_progress_gap_ms;

    // Feeder passes every 250 ms: false for three of them, then feeding again.
    supervision::supervised(true, 0);
    for now in [250, 500, 750] {
        supervision::supervised(false, now);
    }
    assert!(750 - 250 < gap);
    supervision::supervised(true, 1_000);
    // A later false starts its own count, not the old one's.
    supervision::supervised(false, 1_250);
    supervision::supervised(false, 1_250 + gap - 1);

    assert_eq!(wellfed_wifi_restarting(), 0);
    tick(c"IDLE", false);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!(crumb.feeder, Feeder::Feeding);
}

#[test]
fn a_supervision_stop_past_the_gap_leaves_and_marks_the_crumb() {
    use wellfed_hal::link::{self, wellfed_wifi_restarting};
    use wellfed_hal::supervision::{self, config};

    let _guard = start();
    link::reset();
    supervision::reset();
    let gap = config::WATCHDOG.max_progress_gap_ms;

    supervision::supervised(false, 5_000);
    supervision::supervised(false, 5_000 + gap - 250);
    assert_eq!(wellfed_wifi_restarting(), 0);
    supervision::supervised(false, 5_000 + gap);

    assert_eq!(wellfed_wifi_restarting(), 1);
    tick(c"IDLE", false);
    let crumb = Crumb::decode(register()).expect("the hook leaves a crumb");
    assert_eq!(crumb.feeder, Feeder::Stopping);
    link::reset();
}
