//! The FW44 budget run against the SDK fake: it waits for its start time,
//! runs XX then KK through the session table one operation per step, reports
//! each pattern once, and records the heap at boot, after the run and then
//! once a minute.

use petlibre_core::noise::Pattern;
use petlibre_fw_feeder::budget::{Budget, RUNS, Record, START_MS, Stage, ticks_to_us};
use petlibre_sdk::hal::sdk::fake;

#[test]
fn runs_xx_then_kk_one_operation_per_step() {
    fake::reset_hardware();
    let mut budget = Budget::new();
    let mut records = Vec::new();

    assert!(
        !budget.step(0, |r| records.push(r)),
        "nothing before the start"
    );
    assert!(matches!(
        records.as_slice(),
        [Record::Heap {
            stage: Stage::Boot,
            ..
        }]
    ));
    records.clear();

    let mut now = START_MS;
    let mut operations = 0;
    while budget.step(now, |r| records.push(r)) {
        operations += 1;
        now += 1;
        assert!(operations < 1_000, "the run must end");
    }
    assert!(budget.done());

    // XX writes 3 messages and KK 2, each read once, per run; the first
    // step only sets the run up.
    assert_eq!(operations, 1 + usize::from(RUNS) * (2 * 3 + 2 * 2));
    let patterns: Vec<Pattern> = records
        .iter()
        .filter_map(|r| match r {
            Record::Handshake { pattern, .. } => Some(*pattern),
            _ => None,
        })
        .collect();
    assert_eq!(patterns, [Pattern::Xx, Pattern::Kk]);
    assert!(
        !records.iter().any(|r| matches!(r, Record::Failed { .. })),
        "{records:?}"
    );
}

#[test]
fn the_heap_is_recorded_after_the_run_and_each_minute() {
    fake::reset_hardware();
    let mut budget = Budget::new();
    let mut stages = Vec::new();
    let mut heap = |r: Record| {
        if let Record::Heap { stage, free, min } = r {
            assert_eq!((free, min), (200 * 1024, 180 * 1024));
            stages.push(stage);
        }
    };

    let mut now = START_MS;
    while budget.step(now, &mut heap) {
        now += 1;
    }
    // The step after the last operation records the heap, then a minute
    // passes before the next.
    budget.step(now + 59_999, &mut heap);
    budget.step(now + 60_000, &mut heap);
    assert_eq!(stages, [Stage::Boot, Stage::Steady, Stage::Steady]);
}

#[test]
fn ticks_convert_at_the_nominal_rate() {
    assert_eq!(ticks_to_us(0), 0);
    assert_eq!(ticks_to_us(32_768), 1_000_000);
    assert_eq!(ticks_to_us(33), 1_007);
    assert_eq!(ticks_to_us(u32::MAX), u32::MAX, "saturates");
}
