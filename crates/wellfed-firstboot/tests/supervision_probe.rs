use wellfed_amebad::board::MOTOR_MASK;
use wellfed_amebad::sdk::{self, fake};
use wellfed_amebad::watchdog::WatchdogFault;
use wellfed_firstboot::probes::supervision::{
    FEED_MS, MARKER, Probe, RESET_DEADLINE_MS, Record, SAMPLES, STALL_MS, Sample,
};

fn motor_pins() -> u32 {
    sdk::gpio_port_read(0, MOTOR_MASK)
}

/// Both emitters OFF: LOW pad input AND LOW output latch.
fn optics_off() -> bool {
    !sdk::gpio_read_bit(15)
        && !sdk::gpio_read_bit(35)
        && sdk::gpio_output_latch(0) & (1 << 15) == 0
        && sdk::gpio_output_latch(1) & (1 << 3) == 0
}

fn hw<R>(f: impl FnOnce(&mut fake::Hardware) -> R) -> R {
    fake::with(f)
}

/// About 1 ms of the nominal 32 kHz SYSTIMER per delay millisecond.
const TICKS_PER_MS: u32 = 33;

/// Run task iterations, letting each requested delay pass, until the probe
/// ends or the fake watchdog expires (the platform reset).
fn run(probe: &mut Probe) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..10_000 {
        let step = probe.step();
        records.extend(step.record);
        if step.delay_ms == 0 {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(TICKS_PER_MS, true);
            if hw(|h| h.watchdog_expired) {
                return records;
            }
        }
    }
    records
}

fn cold() -> Box<Probe> {
    fake::reset_hardware();
    Box::default()
}

fn samples(records: &[Record]) -> Vec<Sample> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::Sample(s) => Some(*s),
            _ => None,
        })
        .collect()
}

#[test]
fn cold_boot_measures_stalls_and_resets_through_the_watchdog() {
    let mut probe = cold();
    let records = run(&mut probe);
    assert_eq!(
        records[0],
        Record::Open {
            board: true,
            events: true
        }
    );

    let samples = samples(&records);
    assert_eq!(samples.len() as u32, SAMPLES + 1);
    for (i, pair) in samples.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        assert_eq!(b.n, i as u32 + 1);
        let ticks = b.raw.wrapping_sub(a.raw);
        assert!(ticks > 0 && b.ms > a.ms);
        // One accepted interrupt per 328-tick period, within one either way.
        assert!((b.irqs - a.irqs).abs_diff(ticks / 328) <= 1);
    }
    assert!(
        samples.iter().all(|s| s.ready && s.healthy && s.stopped),
        "{samples:?}"
    );

    let stall = records
        .iter()
        .position(|r| matches!(r, Record::StallBegin(_)));
    let (Record::StallBegin(begin), Record::StallEnd(end)) =
        (records[stall.unwrap()], records[stall.unwrap() + 1])
    else {
        panic!("stall records out of order: {records:?}");
    };
    assert!(end.ms - begin.ms >= STALL_MS);
    assert!((end.irqs - begin.irqs).abs_diff(STALL_MS / 10) <= 2);
    assert!(end.ready && end.healthy && end.stopped);

    let arm = records
        .iter()
        .position(|r| matches!(r, Record::Arm { .. }))
        .unwrap();
    assert!(matches!(
        records[arm],
        Record::Arm {
            ok: true,
            marker: MARKER,
            ..
        }
    ));
    let Record::Fed {
        ms,
        checkpoints,
        ok,
    } = records[arm + 1]
    else {
        panic!("expected fed after arm: {records:?}");
    };
    assert!(ok && ms >= FEED_MS && checkpoints >= FEED_MS / 250);
    assert!(matches!(records[arm + 2], Record::WatchdogStall { .. }));

    let stalled: Vec<_> = records[arm + 3..].to_vec();
    assert!(!stalled.is_empty());
    for r in &stalled {
        assert!(matches!(
            r,
            Record::Stalled { fault, enabled: true, .. } if *fault == WatchdogFault::PROGRESS.0
        ));
    }
    // Expired in reset mode while stopped, with the marker set for the next boot.
    assert!(hw(|h| h.watchdog_expired && h.watchdog_reset_requests == 1));
    assert_eq!(sdk::backup_read(sdk::BACKUP_USER), MARKER);
    assert_eq!(motor_pins(), MOTOR_MASK);
    assert!(optics_off());
}

#[test]
fn stuck_loop_never_refreshes() {
    let mut probe = cold();
    let mut refreshes_at_stall = None;
    for _ in 0..10_000 {
        let step = probe.step();
        if matches!(step.record, Some(Record::WatchdogStall { .. })) {
            refreshes_at_stall = Some(hw(|h| h.watchdog_refreshes));
        }
        if step.delay_ms == 0 || hw(|h| h.watchdog_expired) {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(TICKS_PER_MS, true);
        }
    }
    assert!(hw(|h| h.watchdog_expired));
    assert_eq!(hw(|h| h.watchdog_refreshes), refreshes_at_stall.unwrap());
}

#[test]
fn boot_with_marker_skips_and_clears_it() {
    let mut probe = cold();
    hw(|h| h.backup[1] = MARKER);
    assert_eq!(
        run(&mut probe),
        [Record::Skip {
            reset: 0,
            marker: MARKER
        }]
    );
    assert_eq!(sdk::backup_read(sdk::BACKUP_USER), 0);
    assert!(!probe.board.opened && !probe.events.opened);
    assert_eq!(
        hw(|h| (h.timer.en, h.watchdog_vendor, h.direction)),
        (0, 0, [0, 0])
    );
}

#[test]
fn watchdog_boot_reason_skips() {
    let mut probe = cold();
    hw(|h| h.boot_reason = sdk::BOOT_KM4_WATCHDOG);
    assert_eq!(
        run(&mut probe),
        [Record::Skip {
            reset: sdk::BOOT_KM4_WATCHDOG,
            marker: 0
        }]
    );
    assert!(!probe.board.opened);
}

#[test]
fn already_enabled_watchdog_refuses_arm_and_clears_marker() {
    let mut probe = cold();
    let mut records = Vec::new();
    for _ in 0..10_000 {
        let step = probe.step();
        if matches!(step.record, Some(Record::StallEnd(_))) {
            hw(|h| h.watchdog_vendor |= sdk::WDG_BIT_ENABLE);
        }
        records.extend(step.record);
        if step.delay_ms == 0 {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(TICKS_PER_MS, true);
        }
    }
    assert!(matches!(
        records.last(),
        Some(Record::Arm { ok: false, .. })
    ));
    assert_eq!(sdk::backup_read(sdk::BACKUP_USER), 0);
}

#[test]
fn watchdog_that_never_expires_is_reported_and_released() {
    let mut probe = cold();
    let mut records = Vec::new();
    for _ in 0..10_000 {
        let step = probe.step();
        if matches!(step.record, Some(Record::Arm { ok: true, .. })) {
            hw(|h| h.watchdog_period = u32::MAX);
        }
        records.extend(step.record);
        if step.delay_ms == 0 {
            break;
        }
        for _ in 0..step.delay_ms {
            fake::advance(TICKS_PER_MS, true);
        }
    }
    let Some(Record::NoReset { ms }) = records.last() else {
        panic!("expected no_reset: {records:?}");
    };
    assert!(*ms >= RESET_DEADLINE_MS);
    assert!(!probe.watchdog.claimed);
    assert_eq!(hw(|h| h.watchdog_vendor) & sdk::WDG_BIT_ENABLE, 0);
    assert_eq!(sdk::backup_read(sdk::BACKUP_USER), 0);
    assert_eq!(motor_pins(), MOTOR_MASK);
}
