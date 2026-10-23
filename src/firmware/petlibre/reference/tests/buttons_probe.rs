//! FW20 read-only button probe against the fake SDK: the pins become
//! inputs, a bouncing press is one record with its bounce, a floating line
//! is bounded, and the motor pins never leave STOP.

use petlibre_fw_reference::probes::buttons::{
    EVENT_CAP_MS, IDLE_MS, MASK, PINS, POLL_MS, Probe, Record, SETTLE_MS,
};
use petlibre_hal::board::MOTOR_MASK;
use petlibre_hal::sdk::fake;
use petlibre_hal::stopped;

const RELEASED: u32 = MASK;

fn start() -> Probe {
    fake::reset_hardware();
    fake::with(|h| {
        h.interrupts = 1;
        h.input[1] |= RELEASED;
        h.direction[1] |= MASK;
    });
    assert!(stopped::initialize().verified());
    Probe::default()
}

fn set_levels(levels: u32) {
    fake::with(|h| h.input[1] = (h.input[1] & !MASK) | levels);
}

/// Steps until a record, advancing the clock one poll each time.
fn next_record(probe: &mut Probe, limit_ms: u32) -> Option<Record> {
    for _ in 0..limit_ms / POLL_MS {
        fake::with(|h| h.uptime_ms += POLL_MS);
        let step = probe.step();
        assert_eq!(step.delay_ms, POLL_MS);
        if step.record.is_some() {
            return step.record;
        }
    }
    None
}

#[test]
fn opens_the_pins_as_inputs_and_reports_the_levels() {
    let mut probe = start();

    let step = probe.step();
    let Some(Record::Open {
        inputs,
        levels,
        stopped,
        ..
    }) = step.record
    else {
        panic!("{step:?}");
    };
    assert!(inputs && stopped);
    assert_eq!(levels, RELEASED);
    assert_eq!(fake::with(|h| h.direction[1]) & MASK, 0);
    assert_eq!(PINS.len(), MASK.count_ones() as usize);
}

#[test]
fn a_bouncing_press_and_release_are_one_record_each() {
    let mut probe = start();
    probe.step();
    let pressed = RELEASED & !(1 << 5);

    for level in [pressed, RELEASED, pressed, RELEASED, pressed] {
        set_levels(level);
        fake::with(|h| h.uptime_ms += POLL_MS);
        assert_eq!(probe.step().record, None);
    }

    let Some(Record::Change {
        n,
        from,
        to,
        edges,
        bounce_ms,
        settled,
        stopped,
        ..
    }) = next_record(&mut probe, SETTLE_MS * 2)
    else {
        panic!("no change record");
    };
    assert_eq!((n, from, to, edges), (0, RELEASED, pressed, 5));
    assert_eq!(bounce_ms, 4 * POLL_MS);
    assert!(settled && stopped);

    set_levels(RELEASED);
    let Some(Record::Change { n, from, to, .. }) = next_record(&mut probe, SETTLE_MS * 2) else {
        panic!("no release record");
    };
    assert_eq!((n, from, to), (1, pressed, RELEASED));
    assert_eq!(fake::with(|h| h.output[0]) & MOTOR_MASK, MOTOR_MASK);
}

#[test]
fn a_line_that_never_settles_is_cut_off() {
    let mut probe = start();
    probe.step();

    let mut record = None;
    for i in 0..EVENT_CAP_MS / POLL_MS + 1 {
        set_levels(if i % 2 == 0 {
            RELEASED & !(1 << 7)
        } else {
            RELEASED
        });
        fake::with(|h| h.uptime_ms += POLL_MS);
        record = probe.step().record;
        if record.is_some() {
            break;
        }
    }

    let Some(Record::Change {
        settled, bounce_ms, ..
    }) = record
    else {
        panic!("{record:?}");
    };
    assert!(!settled);
    assert!(bounce_ms >= EVENT_CAP_MS - POLL_MS);
}

#[test]
fn an_idle_record_follows_a_quiet_period() {
    let mut probe = start();
    probe.step();

    let Some(Record::Idle {
        levels, stopped, ..
    }) = next_record(&mut probe, IDLE_MS + POLL_MS)
    else {
        panic!("no idle record");
    };
    assert_eq!(levels, RELEASED);
    assert!(stopped);
}
