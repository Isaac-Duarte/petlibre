//! The scheduling-gap probe: gaps and lateness from the task's own wakes on
//! the 32 kHz timer, the per-window counts and percentile, windows that
//! close on the loop's clock with the Wi-Fi counters, and lines that fit the
//! record buffer whatever their values.

use wellfed_fw_feeder::gaps::{
    Net, OVER_MS, PERIOD_MS, Probe, Record, Task, WINDOW_MS, Window, Windows, describe,
};
use wellfed_sdk::hal::net_status::Status;
use wellfed_sdk::hal::startup::Report;

/// System timer ticks for `ms`, rounded up so they read back as `ms`.
fn ticks(ms: u32) -> u32 {
    u32::try_from((u64::from(ms) * 32_768).div_ceil(1000)).unwrap()
}

/// Wakes `probe` once, then after each of `gaps_ms`. Each gap is rounded
/// to ticks on its own, so it reads back exactly.
fn wake_after(probe: &Probe, gaps_ms: &[u32]) {
    let mut at = 0u32;
    probe.wake(at);
    for &ms in gaps_ms {
        at = at.wrapping_add(ticks(ms));
        probe.wake(at);
    }
}

fn status(generation: u32) -> Status {
    Status {
        rssi: -50,
        attempt: 2,
        link_losses: 1,
        dns_losses: 0,
        last_loss: 1,
        dns_ok: true,
        generation,
    }
}

fn text(record: Record) -> String {
    let mut out = String::new();
    describe(record, &mut out).unwrap();
    out
}

#[test]
fn the_first_wake_only_starts_the_clock() {
    let probe = Probe::new(PERIOD_MS);
    wake_after(&probe, &[]);

    assert_eq!(probe.take(), Window::default());
}

#[test]
fn a_window_holds_the_longest_gap_its_lateness_and_the_counts_past_each_limit() {
    let probe = Probe::new(PERIOD_MS);
    // On time, then held 700 ms, then 60 ms and 1.5 s late.
    wake_after(&probe, &[10, 10, 710, 10, 70, 1_510, 10]);

    let window = probe.take();
    assert_eq!(window.wakes, 7);
    assert_eq!(window.max_gap_ms, 1_510);
    assert_eq!(window.max_late_ms, 1_500);
    assert_eq!(OVER_MS, [50, 200, 1000]);
    assert_eq!(window.over, [3, 2, 1]);
    // Three of seven late: the percentile is the worst bucket's top.
    assert_eq!(window.p99_late_ms, 2_047);

    // Taking it cleared the counts, but the clock runs on.
    probe.wake(ticks(10_000));
    let next = probe.take();
    assert_eq!(next.wakes, 1);
    assert!(next.max_gap_ms > 7_000);
    assert_eq!(next.over, [1, 1, 1]);
}

#[test]
fn the_percentile_ignores_the_last_hundredth() {
    let probe = Probe::new(PERIOD_MS);
    let mut gaps = vec![PERIOD_MS + 3; 990];
    gaps.extend([PERIOD_MS + 700; 10]);
    wake_after(&probe, &gaps);

    let window = probe.take();
    assert_eq!(window.wakes, 1_000);
    assert_eq!(window.p99_late_ms, 3);
    assert_eq!(window.max_late_ms, 700);
    assert_eq!(window.over, [10, 10, 0]);
}

#[test]
fn lateness_is_against_the_delay_the_task_last_asked_for() {
    let probe = Probe::new(0);
    probe.wake(0);
    probe.asked(1_000);
    probe.wake(ticks(1_040));
    probe.asked(5);
    probe.wake(ticks(1_040) + ticks(10));

    let window = probe.take();
    assert_eq!(window.max_gap_ms, 1_040);
    assert_eq!(window.max_late_ms, 40);
    assert_eq!(window.over, [0, 0, 0]);
}

#[test]
fn a_gap_across_the_timer_wrap_reads_as_the_gap() {
    let probe = Probe::new(PERIOD_MS);
    probe.wake(u32::MAX - ticks(5) + 1);
    probe.wake(ticks(5));

    assert_eq!(probe.take().max_gap_ms, 10);
}

#[test]
fn windows_close_on_the_loop_clock_with_both_tasks_and_the_wifi_counters() {
    let looped = Probe::new(0);
    let high = Probe::new(PERIOD_MS);
    let mut windows = Windows::new();
    let mut records = Vec::new();

    windows.step(1_000, &looped, &high, status(1), |r| records.push(r));
    wake_after(&high, &[10, 610]);
    windows.step(1_000 + WINDOW_MS - 1, &looped, &high, status(1), |r| {
        records.push(r);
    });
    assert!(records.is_empty(), "no window has run yet");

    windows.step(1_000 + WINDOW_MS + 5, &looped, &high, status(3), |r| {
        records.push(r);
    });
    let lines: Vec<String> = records.iter().copied().map(text).collect();
    assert_eq!(
        lines,
        [
            "p4 t=11005 w=10005 n=2 max=610 late=600 p99=1023 over=1/1/0 worst=610",
            "loop t=11005 w=10005 n=0 max=0 late=0 p99=0 over=0/0/0 worst=0",
            "net t=11005 gen=3 att=2 lost=1 dns=0 rejoin=1",
        ]
    );

    // The next window keeps the worst since boot and sees no rejoin.
    records.clear();
    high.wake(ticks(10) + ticks(610) + ticks(10));
    windows.step(1_000 + 2 * WINDOW_MS + 5, &looped, &high, status(3), |r| {
        records.push(r);
    });
    assert_eq!(
        records.first(),
        Some(&Record::Task {
            task: Task::High,
            now_ms: 21_005,
            span_ms: WINDOW_MS,
            window: Window {
                wakes: 1,
                max_gap_ms: 10,
                ..Window::default()
            },
            worst_ms: 610,
        })
    );
    assert!(text(records[2]).ends_with("rejoin=0"));
}

#[test]
fn the_longest_lines_fit_the_record_buffer() {
    let full = Window {
        wakes: u32::MAX,
        max_gap_ms: u32::MAX,
        max_late_ms: u32::MAX,
        p99_late_ms: u32::MAX,
        over: [u32::MAX; 3],
    };
    let records = [
        Record::Task {
            task: Task::Loop,
            now_ms: u32::MAX,
            span_ms: u32::MAX,
            window: full,
            worst_ms: u32::MAX,
        },
        Record::Net {
            now_ms: u32::MAX,
            net: Net {
                generation: u32::MAX,
                attempt: u32::MAX,
                link_losses: u32::MAX,
                dns_losses: u32::MAX,
                changed: true,
            },
        },
    ];

    for record in records {
        let mut report = Report::new();
        let line = report.record(format_args!("PLGAP1 {}\r\n", Line(record)));
        assert!(line.is_ok(), "{}", text(record));
    }
}

struct Line(Record);

impl std::fmt::Display for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        describe(self.0, f)
    }
}
