//! FW20: the button service. The firmware's button task polls PB4..PB7 every
//! `POLL_MS` and turns the levels into gestures (`wellfed_core::buttons`).
//! Each gesture goes into a small queue for the one firmware task that reads
//! them, and to the UART as a `PLKEY1` record.
//!
//! The pins are configured as stock leaves them: inputs with no pad pull,
//! since the board has its own pull-ups (FW20, physical). Polling rather than
//! edge interrupts keeps the service out of interrupt context; a 5 ms poll
//! is well inside the 30 ms debounce and the 0.1 s shortest press seen.
//! Nothing here touches the motor, the emitters or flash.

use core::sync::atomic::{AtomicU32, Ordering};

use wellfed_core::buttons::{Buttons, Config, Engine, Gesture};

use crate::sdk;

/// PB4..PB7 as SDK pin numbers (port B is pins 32..63).
pub const PINS: [u32; 4] = [36, 37, 38, 39];
const PORT_B: u32 = 1;
const MASK: u32 = 0xf0;
pub const POLL_MS: u32 = 5;

/// Gestures waiting for the reader. A person makes a few a second at most,
/// so a full queue means nobody is reading; newer gestures are then dropped
/// and counted, never written over unread ones.
const QUEUE: usize = 8;
static SLOTS: [AtomicU32; QUEUE] = [const { AtomicU32::new(0) }; QUEUE];
/// Gestures written and read so far, wrapping; only the service task moves
/// `WRITTEN` and only the reader moves `READ`.
static WRITTEN: AtomicU32 = AtomicU32::new(0);
static READ: AtomicU32 = AtomicU32::new(0);
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// What the service reports on the UART.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The pins are open: whether all four read as inputs, and who is
    /// already pressed.
    Open { inputs: bool, pressed: Buttons },
    /// `dropped` counts gestures lost to a full queue so far.
    Gesture { gesture: Gesture, dropped: u32 },
    /// The configuration was refused; the service does not run.
    Refused,
}

/// `PLKEY1 …` body.
pub fn describe(record: Record, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    match record {
        Record::Open { inputs, pressed } => {
            write!(f, "open inputs={} pressed={pressed}", u8::from(inputs))
        }
        Record::Gesture { gesture, dropped } => write!(f, "{gesture} dropped={dropped}"),
        Record::Refused => f.write_str("refused config"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Zero once the service has stopped.
    pub delay_ms: u32,
    pub record: Option<Record>,
}

/// The button task's state; the SDK keeps one and steps it.
#[derive(Debug)]
pub struct Service {
    config: Config,
    engine: Option<Engine>,
}

impl Service {
    pub const fn new(config: Config) -> Self {
        Self {
            config,
            engine: None,
        }
    }

    /// Opens the pins on the first call, then polls.
    pub fn step(&mut self) -> Step {
        let now = sdk::uptime_ms();
        let Some(engine) = self.engine.as_mut() else {
            return self.open();
        };

        let pressed = Buttons::from_port_b(sdk::gpio_port_read(PORT_B, MASK));
        let record = engine.update(now, pressed).map(|gesture| Record::Gesture {
            gesture,
            dropped: publish(gesture),
        });
        Step {
            delay_ms: POLL_MS,
            record,
        }
    }

    fn open(&mut self) -> Step {
        if !self.config.valid() {
            return Step {
                delay_ms: 0,
                record: Some(Record::Refused),
            };
        }

        for pin in PINS {
            sdk::gpio_init(pin, false);
        }
        self.engine = Some(Engine::new(self.config));
        Step {
            delay_ms: POLL_MS,
            record: Some(Record::Open {
                inputs: sdk::gpio_direction(PORT_B) & MASK == 0,
                pressed: Buttons::from_port_b(sdk::gpio_port_read(PORT_B, MASK)),
            }),
        }
    }
}

/// Queues `gesture` for the reader; returns the dropped count so far.
fn publish(gesture: Gesture) -> u32 {
    let written = WRITTEN.load(Ordering::Relaxed);
    let read = READ.load(Ordering::Acquire);
    if written.wrapping_sub(read) as usize >= QUEUE {
        return DROPPED.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    }

    SLOTS[written as usize % QUEUE].store(gesture.pack(), Ordering::Relaxed);
    WRITTEN.store(written.wrapping_add(1), Ordering::Release);
    DROPPED.load(Ordering::Relaxed)
}

/// The oldest unread gesture. One firmware task reads gestures: the one that
/// owns what the buttons do, so a Dispense press reaches feeding through the
/// same owner as a network request.
pub fn next() -> Option<Gesture> {
    let read = READ.load(Ordering::Relaxed);
    if read == WRITTEN.load(Ordering::Acquire) {
        return None;
    }

    let word = SLOTS[read as usize % QUEUE].load(Ordering::Relaxed);
    READ.store(read.wrapping_add(1), Ordering::Release);
    Gesture::unpack(word)
}

/// Tests only: empty the queue.
#[doc(hidden)]
pub fn reset() {
    READ.store(WRITTEN.load(Ordering::Acquire), Ordering::Release);
    DROPPED.store(0, Ordering::Relaxed);
}
