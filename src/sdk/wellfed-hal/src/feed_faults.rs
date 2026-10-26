//! Jams and sensor faults, for the feeder's `feed-jam` and `sensor-fault`
//! bench variants only (feature `feed-faults`; never in a product image).
//!
//! While a fault is armed, the board's input capture reads what that fault
//! would show, and nothing else changes: the motor still turns, the marks
//! still pass, and every path that handles the fault is the product's own
//! (the controller's jam and feedback checks, the owner's sensor hold). The
//! pin watch samples the pads directly, so a bench capture still shows the
//! real PA18 toggling under a masked sensor, which tells an injected fault
//! from a real one.
//!
//! The faults are read where the product reads its inputs: the GPIO word in
//! `Board::capture_gpio`, the ADC conversions and the current channel's
//! samples in `Board::capture`, and the feedback interrupt in the
//! supervisor. One fault at a time, one static in RAM, disarmed at every
//! boot.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use wellfed_core::adc_scale::SAMPLES;
use wellfed_core::feed::controller::MotorCommand;

use crate::plaf103::pins::{FEEDBACK, OUTLET, bit};

/// The current channel's raw reading under a jam: about 620 scaled with
/// the default calibration (offset 2480, gain 12050), twice the 300 jam
/// limit and well under the 1000 ceiling that stops a drive at once.
pub const JAM_RAW: u16 = 1000;

/// The fault the board's capture shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Injected {
    #[default]
    None = 0,
    /// The current channel reads `JAM_RAW` while the motor drives forward,
    /// until it first drives back: a jam the first reversal frees.
    JamFrees = 1,
    /// The current channel reads `JAM_RAW` whenever the motor drives
    /// forward: a jam no reversal frees.
    JamHolds = 2,
    /// PA18 reads HIGH and interrupts no more: the mark sensor lost.
    MarksMissing = 3,
    /// PA18 reads LOW and interrupts no more: the sensor stuck at a mark.
    MarkStuckLow = 4,
    /// The outlet receiver reads LOW, as a blocked beam does.
    OutletBlocked = 5,
    /// The current channel's conversion never completes: the ADC lost.
    AdcFails = 6,
}

impl Injected {
    /// The code `PLINJ1` records print.
    pub const fn code(self) -> u8 {
        self as u8
    }

    const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::JamFrees,
            2 => Self::JamHolds,
            3 => Self::MarksMissing,
            4 => Self::MarkStuckLow,
            5 => Self::OutletBlocked,
            6 => Self::AdcFails,
            _ => Self::None,
        }
    }
}

static ARMED: AtomicU8 = AtomicU8::new(0);
static READINGS: AtomicU32 = AtomicU32::new(0);
static EDGES: AtomicU32 = AtomicU32::new(0);

/// What the armed fault has changed since it was armed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Captures it changed.
    pub readings: u32,
    /// Feedback interrupts it dropped.
    pub edges: u32,
}

/// Show `fault` from the next capture on, and count afresh.
pub fn arm(fault: Injected) {
    READINGS.store(0, Ordering::Relaxed);
    EDGES.store(0, Ordering::Relaxed);
    ARMED.store(fault.code(), Ordering::Release);
}

/// The fault armed now.
pub fn armed() -> Injected {
    Injected::from_code(ARMED.load(Ordering::Acquire))
}

pub fn counts() -> Counts {
    Counts {
        readings: READINGS.load(Ordering::Relaxed),
        edges: EDGES.load(Ordering::Relaxed),
    }
}

fn changed() {
    READINGS.fetch_add(1, Ordering::Relaxed);
}

/// The sensor port word a capture reads, `pins` as the pads gave it.
pub(crate) fn gpio(pins: u32) -> u32 {
    let shown = match armed() {
        Injected::MarksMissing => pins | bit(FEEDBACK),
        Injected::MarkStuckLow => pins & !bit(FEEDBACK),
        Injected::OutletBlocked => pins & !bit(OUTLET),
        _ => return pins,
    };
    changed();
    shown
}

/// Whether the current channel's conversion fails.
pub(crate) fn adc_fails() -> bool {
    let fails = armed() == Injected::AdcFails;
    if fails {
        changed();
    }
    fails
}

/// The current channel's samples, with the motor at `applied`.
pub(crate) fn current(applied: MotorCommand, samples: &mut [u16; SAMPLES]) {
    match (armed(), applied) {
        (Injected::JamFrees | Injected::JamHolds, MotorCommand::Drive0) => {
            samples.fill(JAM_RAW);
            changed();
        }
        // Backing off freed it.
        (Injected::JamFrees, MotorCommand::Drive1) => ARMED.store(0, Ordering::Release),
        _ => {}
    }
}

/// Whether the feedback interrupt drops this edge, as a lost or stuck
/// sensor would never raise it.
pub(crate) fn drop_edge() -> bool {
    let dropped = matches!(armed(), Injected::MarksMissing | Injected::MarkStuckLow);
    if dropped {
        EDGES.fetch_add(1, Ordering::Relaxed);
    }
    dropped
}
