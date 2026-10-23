//! FW14 power inputs for health: the battery pack on ADC channel 5 and the
//! adapter on PA19, sampled every `SAMPLE_MS` by the runtime's power task.
//!
//! Health reports a sample only while it is fresh (`STALE_MS`), so a stalled
//! task, a failed conversion or an ADC the monitor never opened all read as
//! unknown rather than as the last value. The pack voltage is the pin's
//! stock-scaled millivolts through the board divider, measured at 2.058 and
//! 2.068 against a meter on three D cells (physical, October 23 2026).

use core::sync::atomic::{AtomicU32, Ordering};

use petlibre_core::adc_scale::{AdcCalibration, SAMPLES};
use petlibre_core::transport::PowerHealth;

use crate::{adc, sdk};

pub const SAMPLE_MS: u32 = 5_000;
/// Three missed samples.
pub const STALE_MS: u32 = 3 * SAMPLE_MS;
/// Pack millivolts per thousand pin millivolts.
pub const DIVIDER_PER_MILLE: u32 = 2_063;
/// Below this at the pin no pack is fitted: with none, the channel read
/// 16 mV, and three D cells read 2306–2357 mV.
pub const NO_PACK_PIN_MV: u32 = 500;
const BATTERY_CHANNEL: u32 = 5;
const ADAPTER_PIN: u32 = 19;
/// Polls per conversion; a conversion takes well under 1 ms.
const ADC_POLLS: u32 = 10_000;

/// Bit 31: a sample is held; bit 30: the adapter; bits 0..16: pin millivolts.
static SAMPLE: AtomicU32 = AtomicU32::new(0);
static SAMPLED_AT_MS: AtomicU32 = AtomicU32::new(0);
const HELD: u32 = 1 << 31;
const ADAPTER: u32 = 1 << 30;
const MV_MASK: u32 = 0xffff;

/// What a monitor step reports on the UART: the open, the first sample
/// and each adapter change, or the failure that stopped it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Opened { ok: bool },
    Sample { pin_mv: u32, adapter: bool },
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Zero once the monitor has given up.
    pub delay_ms: u32,
    pub record: Option<Record>,
}

/// `PLPWR1 …` body.
pub fn describe(record: Record, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    match record {
        Record::Opened { ok } => write!(f, "open ok={}", u8::from(ok)),
        Record::Sample { pin_mv, adapter } => {
            write!(f, "sample pin_mv={pin_mv} adapter={}", u8::from(adapter))
        }
        Record::Failed => f.write_str("error adc"),
    }
}

/// The power task's state; the runtime keeps one and steps it.
#[derive(Debug, Default)]
pub struct Monitor {
    calibration: Option<AdcCalibration>,
    /// The adapter level last reported, `None` before the first sample.
    reported: Option<bool>,
    /// The ADC failed to open or convert; the monitor stops for this boot.
    failed: bool,
}

impl Monitor {
    /// Opens the ADC on the first call, then samples.
    pub fn step(&mut self) -> Step {
        if self.failed {
            return Step {
                delay_ms: 0,
                record: None,
            };
        }

        let Some(calibration) = self.calibration else {
            sdk::gpio_init(ADAPTER_PIN, false);
            let calibration = adc::open().calibration;
            self.calibration = calibration;
            if calibration.is_none() {
                return self.fail(Record::Opened { ok: false });
            }
            return Step {
                delay_ms: 1,
                record: Some(Record::Opened { ok: true }),
            };
        };

        let mut samples = [0u16; SAMPLES];
        let begin = sdk::uptime_ms();
        if !adc::convert(BATTERY_CHANNEL, ADC_POLLS, &mut samples) {
            return self.fail(Record::Failed);
        }
        let Some(pin_mv) = calibration.scale_16(&samples) else {
            return self.fail(Record::Failed);
        };

        let adapter = sdk::gpio_read_bit(ADAPTER_PIN);
        publish(pin_mv, adapter, begin);
        let changed = self.reported != Some(adapter);
        self.reported = Some(adapter);
        Step {
            delay_ms: SAMPLE_MS,
            record: changed.then_some(Record::Sample { pin_mv, adapter }),
        }
    }

    fn fail(&mut self, record: Record) -> Step {
        self.failed = true;
        SAMPLE.store(0, Ordering::Release);
        adc::close();
        Step {
            delay_ms: 0,
            record: Some(record),
        }
    }
}

/// The time is the start of the conversion, so its duration counts toward
/// the age. The sample is withdrawn while its time changes, so a reader
/// never pairs one sample's value with another's time.
fn publish(pin_mv: u32, adapter: bool, at_ms: u32) {
    let packed = HELD | if adapter { ADAPTER } else { 0 } | pin_mv.min(MV_MASK);
    SAMPLE.store(0, Ordering::Release);
    SAMPLED_AT_MS.store(at_ms, Ordering::Release);
    SAMPLE.store(packed, Ordering::Release);
}

/// The latest sample, if fresh at `now_ms`. A publish between the two reads
/// of the value reads as unknown for that one health.
pub fn health(now_ms: u32) -> Option<PowerHealth> {
    let packed = SAMPLE.load(Ordering::Acquire);
    let at = SAMPLED_AT_MS.load(Ordering::Acquire);
    if SAMPLE.load(Ordering::Acquire) != packed || packed & HELD == 0 {
        return None;
    }

    let age_ms = now_ms.wrapping_sub(at);
    if age_ms > STALE_MS {
        return None;
    }
    let pin_mv = packed & MV_MASK;
    Some(PowerHealth {
        adapter: packed & ADAPTER != 0,
        battery_mv: (pin_mv >= NO_PACK_PIN_MV).then(|| pin_mv * DIVIDER_PER_MILLE / 1000),
        age_ms,
    })
}

/// Tests only: forget the published sample.
#[doc(hidden)]
pub fn reset() {
    SAMPLE.store(0, Ordering::Release);
    SAMPLED_AT_MS.store(0, Ordering::Release);
}
