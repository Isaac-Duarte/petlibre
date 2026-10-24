//! FW20 bench probe on KM4: the four panel buttons, read-only.
//!
//! Stock configures PB4..PB7 as inputs with no pull and takes their edges by
//! interrupt; which printed button is which pin, and its pressed level, are
//! not known. The probe configures the pins as stock does, records their pad
//! control before and after, then polls the four levels every `POLL_MS`.
//!
//! A change starts an event; the event ends once the levels have held for
//! `SETTLE_MS`, or after `EVENT_CAP_MS` if they never settle (a floating
//! input), so the log is bounded at about one line per event. Each event
//! reports the levels before and after, the edges seen and the time from
//! the first to the last edge, which is the contact bounce. Motor and
//! emitter outputs are never touched; every record re-checks they read
//! back stopped.

use core::fmt;

use petlibre_hal::sdk;
use petlibre_hal::stopped;

/// PB4..PB7 as SDK pin numbers (port B is pins 32..63).
pub const PINS: [u32; 4] = sdk::board::BUTTONS;
const PORT_B: u32 = 1;
/// The four button bits in the port B input word.
pub const MASK: u32 = 0xf0;

pub const POLL_MS: u32 = 2;
pub const SETTLE_MS: u32 = 30;
pub const EVENT_CAP_MS: u32 = 1000;
/// An idle record this often shows the lines are still being read.
pub const IDLE_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// Pins configured: their pad control words before and after, the
    /// direction bits (0 = input) and the first levels.
    Open {
        pads_before: [u32; 4],
        pads_after: [u32; 4],
        inputs: bool,
        levels: u32,
        stopped: bool,
    },

    /// One settled change, or one that hit `EVENT_CAP_MS` unsettled.
    Change {
        n: u32,
        at_ms: u32,
        from: u32,
        to: u32,
        edges: u32,
        bounce_ms: u32,
        settled: bool,
        stopped: bool,
    },

    Idle {
        at_ms: u32,
        levels: u32,
        stopped: bool,
    },
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        describe(*self, f)
    }
}

/// `PLBTN1 …` body; levels are the port B input bits 4..7 in hex.
pub fn describe(record: Record, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let bit = u8::from;
    match record {
        Record::Open {
            pads_before: b,
            pads_after: a,
            inputs,
            levels,
            stopped,
        } => write!(
            f,
            "open pads_before={:08x},{:08x},{:08x},{:08x} pads_after={:08x},{:08x},{:08x},{:08x} inputs={} levels={levels:02x} stopped={}",
            b[0],
            b[1],
            b[2],
            b[3],
            a[0],
            a[1],
            a[2],
            a[3],
            bit(inputs),
            bit(stopped),
        ),

        Record::Change {
            n,
            at_ms,
            from,
            to,
            edges,
            bounce_ms,
            settled,
            stopped,
        } => write!(
            f,
            "change n={n} ms={at_ms} from={from:02x} to={to:02x} edges={edges} bounce_ms={bounce_ms} settled={} stopped={}",
            bit(settled),
            bit(stopped),
        ),

        Record::Idle {
            at_ms,
            levels,
            stopped,
        } => write!(
            f,
            "idle ms={at_ms} levels={levels:02x} stopped={}",
            bit(stopped)
        ),
    }
}

/// `delay_ms` is always `POLL_MS`; the probe runs as long as the build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

/// A change still bouncing.
#[derive(Clone, Copy, Debug, Default)]
struct Event {
    from: u32,
    first_ms: u32,
    last_ms: u32,
    edges: u32,
}

#[derive(Debug, Default)]
pub struct Probe {
    started: bool,
    levels: u32,
    event: Option<Event>,
    changes: u32,
    idle_since_ms: u32,
}

fn levels() -> u32 {
    sdk::gpio_port_read(PORT_B, MASK)
}

impl Probe {
    /// One task iteration in task context.
    pub fn step(&mut self) -> Step {
        let now = sdk::uptime_ms();
        let record = if self.started {
            self.poll(now)
        } else {
            Some(self.open(now))
        };
        Step {
            delay_ms: POLL_MS,
            record,
        }
    }

    fn open(&mut self, now: u32) -> Record {
        self.started = true;

        let pads_before = PINS.map(sdk::pad_control);
        for pin in PINS {
            sdk::gpio_init(pin, false);
        }
        let pads_after = PINS.map(sdk::pad_control);

        self.levels = levels();
        self.idle_since_ms = now;
        Record::Open {
            pads_before,
            pads_after,
            inputs: sdk::gpio_direction(PORT_B) & MASK == 0,
            levels: self.levels,
            stopped: stopped::observe().verified(),
        }
    }

    fn poll(&mut self, now: u32) -> Option<Record> {
        let read = levels();
        if read != self.levels {
            let event = self.event.get_or_insert(Event {
                from: self.levels,
                first_ms: now,
                ..Event::default()
            });
            event.edges += (read ^ self.levels).count_ones();
            event.last_ms = now;
            self.levels = read;
        }

        if let Some(event) = self.event {
            let settled = now.wrapping_sub(event.last_ms) >= SETTLE_MS;
            if settled || now.wrapping_sub(event.first_ms) >= EVENT_CAP_MS {
                self.event = None;
                self.idle_since_ms = now;
                let n = self.changes;
                self.changes += 1;
                return Some(Record::Change {
                    n,
                    at_ms: event.first_ms,
                    from: event.from,
                    to: self.levels,
                    edges: event.edges,
                    bounce_ms: event.last_ms.wrapping_sub(event.first_ms),
                    settled,
                    stopped: stopped::observe().verified(),
                });
            }
            return None;
        }

        if now.wrapping_sub(self.idle_since_ms) >= IDLE_MS {
            self.idle_since_ms = now;
            return Some(Record::Idle {
                at_ms: now,
                levels: self.levels,
                stopped: stopped::observe().verified(),
            });
        }
        None
    }
}
