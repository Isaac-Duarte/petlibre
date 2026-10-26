//! The SDK's attributes on the host: a `#[task]` keeps what the stack audit
//! and the runtime read, and `#[derive(Record)]` writes the line a
//! hand-written formatter would.

use std::sync::atomic::{AtomicU32, Ordering};

use wellfed_sdk::{LogRecord, Record};

const WORDS: u16 = 768;

static STEPS: AtomicU32 = AtomicU32::new(0);

/// Counts its steps.
#[wellfed_sdk::task(name = "wellfed-test", stack = WORDS, priority = 3)]
fn test_task() -> u32 {
    STEPS.fetch_add(1, Ordering::Relaxed) + 10
}

#[test]
fn a_task_carries_its_name_stack_and_priority_and_steps_its_body() {
    assert_eq!(test_task.name(), c"wellfed-test");
    assert_eq!(test_task.stack_words(), 768);
    assert_eq!(test_task.priority(), 3);

    let first = test_task.step();
    let second = test_task.step();

    assert_eq!((first, second), (10, 11));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Fast,
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Fast => "fast",
    }
}

#[derive(Clone, Copy, Record)]
#[record(tag = "PLTST1")]
enum Line {
    #[record("start")]
    Start,
    #[record("open refused=1")]
    Refused,
    #[record("change")]
    Change {
        #[record(rename = "ms")]
        at_ms: u32,
        #[record(fmt = "02x")]
        levels: u32,
        settled: bool,
        #[record(with = mode_name)]
        mode: Mode,
        #[record(skip)]
        _private: u8,
    },
    /// No text: the fields come first.
    State { state: u8, stopped: bool },
    #[record("stop")]
    Stop { reason: Reason },
    #[record(with = custom)]
    Custom(u8),
    #[record("pair")]
    Pair(u8, #[record(fmt = "04x")] u16),
}

#[derive(Clone, Copy, Record)]
enum Reason {
    #[record("refused")]
    Refused,
    #[record("unverified")]
    Unverified { decision: u8 },
}

fn custom(line: &Line, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match line {
        Line::Custom(n) => write!(f, "custom {n}{{}}"),
        _ => Err(std::fmt::Error),
    }
}

#[derive(Clone, Copy, Record)]
#[record("window")]
struct Window {
    n: u32,
    #[record(rename = "type")]
    kind: u8,
}

#[test]
fn records_write_text_then_keyed_fields() {
    let lines = [
        (Line::Start, "start"),
        (Line::Refused, "open refused=1"),
        (
            Line::Change {
                at_ms: 1500,
                levels: 0xa,
                settled: true,
                mode: Mode::Fast,
                _private: 9,
            },
            "change ms=1500 levels=0a settled=1 mode=fast",
        ),
        (
            Line::State {
                state: 2,
                stopped: false,
            },
            "state=2 stopped=0",
        ),
        (
            Line::Stop {
                reason: Reason::Unverified { decision: 3 },
            },
            "stop reason=unverified decision=3",
        ),
        (Line::Custom(7), "custom 7{}"),
        (Line::Pair(1, 0xbeef), "pair 1 beef"),
    ];

    for (line, text) in lines {
        assert_eq!(line.to_string(), text);
    }
    assert_eq!(Reason::Refused.to_string(), "refused");
    assert_eq!(Window { n: 4, kind: 1 }.to_string(), "window n=4 type=1");
    assert_eq!(Line::TAG, "PLTST1");
}
