use std::collections::{BTreeMap, VecDeque};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use wellfed_formats::bench::banks::{Check, Content, Entry, Ledger, Source, Witness};
use wellfed_formats::bench::live::{
    Activity, ActivityKind, BenchState, CancelState, ClockView, FeederStatus, FeederView, JobView,
    Journal, JournalSource, Phase, ServerView, Session, Uart,
};
use wellfed_formats::bench::notes::{CardEvidence, Expect, Operator, Timer, TimerState, Verdict};
use wellfed_formats::bench::progress::{
    BenchCommand, Current, Outcome, Progress, RelayState, Step, StepState, Unit,
};

use super::{Screen, draw};

const NOW: f64 = 1_793_280_000.0;

fn step(label: &str, state: StepState) -> Step {
    Step {
        label: label.into(),
        state,
        started: None,
        finished: None,
    }
}

/// A bench mid-write: a flash-execute 7 sectors in, a session paused for
/// it, a ledger, and the operator's step, timers and evidence.
fn writing() -> BenchState {
    let record = Current {
        pid: 4242,
        command: BenchCommand::FlashExecute,
        name: Some("unit-07".into()),
        started: NOW - 30.0,
        finished: None,
        outcome: Outcome::Running,
        error: None,
        steps: vec![
            step("Power the feeder off", StepState::Done),
            step("Write the reviewed plan", StepState::Running),
            step("Verify the marker", StepState::Pending),
            step("Boot and read the banner", StepState::Pending),
        ],
        progress: Some(Progress {
            done: 7,
            total: 19,
            unit: Unit::Sectors,
            since: NOW - 20.0,
            since_done: 0,
            at: NOW - 1.0,
        }),
        capture: None,
        transfer: None,
        relay: BTreeMap::new(),
        lines: Vec::new(),
        next: None,
        flash_tool: None,
    };
    let mut job = JobView {
        id: 12,
        label: "flash-execute unit-07".into(),
        log: "private/bench-status/jobs/12-unit-07.log".into(),
        accepted: NOW - 31.0,
        record: None,
        upcoming: Vec::new(),
        cancel: CancelState::None,
        end: None,
    };
    job.update(&record);

    let mut banks = Ledger::default();
    banks.banks.insert(
        wellfed_formats::bench::banks::Bank::One,
        Entry {
            content: Content::Build {
                id: "4d37c21a00ff9988".into(),
            },
            source: Source::FlashExecute,
            session: Some("unit-06".into()),
            at: NOW - 3600.0,
            check: Check::Confirmed {
                by: Witness::Banner,
                at: NOW - 3500.0,
            },
        },
    );
    let relays = [("1", true), ("2", false), ("3", true), ("4", true)]
        .into_iter()
        .map(|(channel, on)| (channel.to_string(), RelayState { on, at: NOW - 5.0 }))
        .collect();

    BenchState {
        now: NOW,
        server: ServerView {
            pid: 777,
            started: NOW - 7200.0,
            phase: Phase::Serving,
            uart_port: true,
        },
        job: Some(job),
        activity: vec![Activity {
            kind: ActivityKind::Flash,
            pid: 4300,
            label: "update execute".into(),
            started: NOW - 20.0,
        }],
        uart: Uart::Writing {
            done: 7,
            total: 19,
            counted: true,
        },
        relays,
        feeder: FeederView {
            session: Session::Paused {
                pause: wellfed_formats::bench::live::Pause::UartJob {
                    job: 12,
                    label: "flash-execute unit-07".into(),
                },
            },
            status: Some(FeederStatus {
                build: "4d37c21a00ff9988".into(),
                bank: 1,
                uptime_ms: 1000,
                boot_count: 31,
                rssi: -51,
                clock_ms: Some(1),
                locked: Some(false),
                last_event: Some(88),
                sessions: 1,
            }),
            status_at: Some(NOW - 60.0),
            clock: ClockView {
                skew_ms: Some(-140),
                synced_at: None,
            },
            journal: Some(Journal {
                seq: 88,
                epoch: Some(3),
                from: JournalSource::Event,
            }),
            hotspot: Some(true),
            log: vec!["sched: next meal 18:00".into(), "wifi: rssi -51".into()],
            other_sessions: 0,
        },
        capture: Default::default(),
        banks,
        operator: Operator {
            step: Some("Install the candidate over UART".into()),
            next: vec![
                "Boot and watch the banner".into(),
                "Run the smoke tier".into(),
            ],
            timers: vec![
                Timer {
                    label: "fire".into(),
                    started: NOW - 100.0,
                    due: NOW + 250.0,
                    until: NOW + 370.0,
                    expect: Some(Expect::Line {
                        text: "PLSCH1 fire".into(),
                    }),
                    state: TimerState::Waiting,
                },
                Timer {
                    label: "boot".into(),
                    started: NOW - 400.0,
                    due: NOW - 300.0,
                    until: NOW - 180.0,
                    expect: Some(Expect::Event {
                        kind: "boot".into(),
                    }),
                    state: TimerState::Missed { at: NOW - 179.0 },
                },
            ],
            evidence: vec![(
                "schedule".into(),
                vec![CardEvidence {
                    at: NOW - 50.0,
                    verdict: Verdict::Pass,
                    text: "banner seen".into(),
                    path: Some("private/captures/boot-01".into()),
                }],
            )],
        },
    }
}

/// The screen as text, a row per line.
fn render(state: &BenchState, log: &VecDeque<String>, scroll: usize) -> String {
    render_with(state, log, scroll, None)
}

fn render_with(
    state: &BenchState,
    log: &VecDeque<String>,
    scroll: usize,
    notice: Option<&str>,
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(130, 46)).unwrap();
    terminal
        .draw(|frame| {
            draw(
                frame,
                &Screen {
                    state,
                    log,
                    scroll,
                    notice,
                },
            );
        })
        .unwrap();
    terminal.backend().to_string()
}

#[test]
fn a_first_q_during_a_write_puts_the_confirmation_in_the_footer() {
    let prompt = wellfed_bench_server::stopping::CONFIRM_PROMPT;

    let asked = render_with(&writing(), &log(1), 0, Some(prompt));
    let plain = render(&writing(), &log(1), 0);

    assert!(asked.contains(prompt), "{asked}");
    assert!(!asked.contains("q or Ctrl+C stops the server"), "{asked}");
    assert!(plain.contains("q or Ctrl+C stops the server"), "{plain}");
}

fn log(lines: usize) -> VecDeque<String> {
    (1..=lines).map(|n| format!("job line {n}")).collect()
}

#[test]
fn a_write_shows_its_banner_job_step_and_sectors() {
    let screen = render(&writing(), &log(3), 0);
    if let Some(path) = std::env::var_os("TUI_SNAPSHOT") {
        std::fs::write(path, &screen).unwrap();
    }

    for text in [
        "UART WRITE IN PROGRESS: don't touch the bench",
        "7 of 19 sectors verified",
        "flash-execute unit-07",
        "Step: Write the reviewed plan",
        "Next: Verify the marker → Boot and read the banner",
        "Flash pid 4300: update execute",
        "Lease: this server, pid 777",
        "1 Feeder power",
        "UART adapter: present",
        "Bank 1: build 4d37c21a00ff",
        "confirmed by the banner",
        "Session: paused: job 12 (flash-execute unit-07) holds",
        "Clock: -140 ms",
        "Journal high water: event 88 in epoch 3",
        "Hotspot: up",
        "wifi: rssi -51",
        "Now: Install the candidate over UART",
        "1. Boot and watch the banner",
        "fire in 4m 10s",
        "MISSED",
        "schedule: 1 pass, 0 fail; last: banner seen",
        "job line 3",
    ] {
        assert!(screen.contains(text), "{text:?} missing from\n{screen}");
    }
}

#[test]
fn an_idle_bench_has_no_banner_and_a_scrolled_log_holds_its_place() {
    let mut state = writing();
    state.uart = Uart::Idle;
    state.job = None;
    state.activity.clear();
    state.operator = Operator::default();

    let screen = render(&state, &log(500), 100);

    assert!(!screen.contains("UART WRITE"), "{screen}");
    assert!(screen.contains("No job yet"), "{screen}");
    assert!(screen.contains("No step noted"), "{screen}");
    assert!(
        screen.contains("Log (100 lines up; End follows)"),
        "{screen}"
    );
    assert!(screen.contains("job line 400"), "{screen}");
    assert!(!screen.contains("job line 401"), "{screen}");
}

#[test]
fn a_seen_timer_and_a_write_with_no_count_say_so() {
    let mut state = writing();
    state.uart = Uart::Writing {
        done: 0,
        total: 0,
        counted: false,
    };
    state.operator.timers[0].state = TimerState::Seen {
        at: NOW - 1.0,
        line: "PLSCH1 fire slot=0".into(),
    };
    state.server.phase = Phase::Stopping;

    let screen = render(&state, &log(1), 0);

    assert!(
        screen.contains("This write reports no sector count"),
        "{screen}"
    );
    assert!(screen.contains("SEEN"), "{screen}");
    assert!(screen.contains("PLSCH1 fire slot=0"), "{screen}");
    assert!(screen.contains("stopping: cancelling the job"), "{screen}");
}
