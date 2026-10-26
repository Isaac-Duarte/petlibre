//! Bench probe on KM4: read-only power inputs.
//!
//! Opens the board's outputs (motor STOP, emitters OFF, no flash authority,
//! so every drive is refused), then its sensors: the explicit LP ADC clock
//! and function enable with readback, the efuse calibration and the ADC.
//! It then samples once a second, without end: PA16/PA17/PA19, and 16
//! conversions each of channel 4 (motor current) and channel 5 (battery),
//! timed against the SYSTIMER, then 16 of internal channel 10 (the chip's own
//! supply), which reads well apart from the others and so shows the channel
//! select takes effect. Every sample re-verifies stopped outputs.
//! A failed capture trips the board and ends the probe.

use core::fmt;

use wellfed_core::adc_scale::{AdcCalibration, SAMPLES};

const SAMPLE_COUNT: u32 = 16;
const _: () = assert!(SAMPLE_COUNT as usize == SAMPLES);
use wellfed_core::feed::sensor_inputs::SensorReading;

use wellfed_hal::board::Board;
use wellfed_hal::plaf103;
use wellfed_hal::stopped;
use wellfed_hal::{sdk, startup};

const DEADTIME_MS: u32 = 50;
const LEASE_MS: u32 = 300;
/// Polls per conversion; the capture's measured duration bounds them in time.
pub const ADC_POLLS: u32 = 10_000;
pub const SAMPLE_MS: u32 = 1000;

/// One channel's 16 conversions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub min: u16,
    pub max: u16,
    /// Floor of the mean, as stock scales it.
    pub mean: u16,
    /// Stock-scaled value; 0 at or below the calibration offset.
    pub scaled: u32,
}

impl Channel {
    fn of(samples: &[u16; SAMPLES], calibration: &AdcCalibration) -> Self {
        let sum: u32 = samples.iter().map(|&s| u32::from(s)).sum();

        Self {
            min: samples.iter().copied().min().unwrap_or_default(),
            max: samples.iter().copied().max().unwrap_or_default(),
            // The mean of u16 samples fits a u16.
            mean: u16::try_from(sum / SAMPLE_COUNT).unwrap_or(u16::MAX),
            scaled: calibration.scale_16(samples).unwrap_or_default(),
        }
    }
}

/// `min,max,mean,scaled`, as the `PLADC1 sample` record prints a channel.
struct Values(Channel);

impl fmt::Display for Values {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = self.0;
        write!(f, "{},{},{},{}", c.min, c.max, c.mean, c.scaled)
    }
}

/// The four efuse calibration bytes in read order as hex, or `none`.
struct Efuse(Option<[u8; 4]>);

impl fmt::Display for Efuse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(b) => write!(f, "{:02x}{:02x}{:02x}{:02x}", b[0], b[1], b[2], b[3]),
            None => f.write_str("none"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// Board outputs and sensors as opened.
    Open {
        outputs: bool,
        clock: sdk::LpClock,
        efuse: Option<[u8; 4]>,
        calibration: AdcCalibration,
        ok: bool,
    },

    /// One pass of the once-a-second loop: the digital inputs, the stopped
    /// readback and 16 conversions of each ADC channel.
    Sample {
        n: u32,
        /// SYSTIMER ticks for the whole capture (48 conversions).
        ticks: u32,
        food: bool,
        outlet: bool,
        dc: bool,
        stopped: bool,
        current: Channel,
        battery: Channel,
        supply: Channel,
    },

    /// A capture failed (ADC timeout) or outputs did not read back stopped.
    Error { n: u32, ticks: u32, stopped: bool },
}

/// Logs a record as its `PLADC1` line or lines. The tag stays inside each
/// format string: the startup audit binds `fw14-adc` to `PLADC1 open
/// outputs=` as linked.
pub fn log(record: Record) {
    let bit = u8::from;
    let pass = |ok: bool| if ok { "pass" } else { "fail" };
    match record {
        Record::Open {
            outputs,
            clock,
            efuse,
            calibration,
            ok,
        } => {
            // Two lines: one record exceeds the report buffer.
            startup::log(format_args!(
                "PLADC1 clock function_before={:08x} clock_before={:08x} function_after={:08x} clock_after={:08x} status={}\r\n",
                clock.function_before,
                clock.clock_before,
                clock.function_after,
                clock.clock_after,
                pass(clock.verified()),
            ));
            startup::log(format_args!(
                "PLADC1 open outputs={} efuse={} offset={} gain={} default_offset={} default_gain={} status={}\r\n",
                bit(outputs),
                Efuse(efuse),
                calibration.offset,
                calibration.gain,
                bit(calibration.default_offset),
                bit(calibration.default_gain),
                pass(ok),
            ));
        }

        Record::Sample {
            n,
            ticks,
            food,
            outlet,
            dc,
            stopped,
            current,
            battery,
            supply,
        } => startup::log(format_args!(
            "PLADC1 sample n={n} ticks={ticks} food={} outlet={} dc={} stopped={} current={} battery={} supply={}\r\n",
            bit(food),
            bit(outlet),
            bit(dc),
            bit(stopped),
            Values(current),
            Values(battery),
            Values(supply),
        )),

        Record::Error { n, ticks, stopped } => startup::log(format_args!(
            "PLADC1 error n={n} ticks={ticks} stopped={}\r\n",
            bit(stopped)
        )),
    }
}

/// `delay_ms == 0` ends the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

/// Keep at a fixed address once started: the board's gate refers to it.
#[derive(Debug, Default)]
pub struct Probe {
    pub board: Board,
    started: bool,
    done: bool,
    samples: u32,
}

fn next(delay_ms: u32, record: Record) -> Step {
    Step {
        delay_ms,
        record: Some(record),
    }
}

impl Probe {
    /// One task iteration in task context.
    pub fn step(&mut self) -> Step {
        if self.done {
            return Step {
                delay_ms: 0,
                record: None,
            };
        }

        if !self.started {
            self.started = true;
            let outputs = self.board.open_outputs(DEADTIME_MS, LEASE_MS, 0);
            let sensors = if outputs {
                self.board.open_sensors(ADC_POLLS)
            } else {
                Default::default()
            };
            let record = Record::Open {
                outputs,
                clock: sensors.clock,
                efuse: sensors.efuse,
                calibration: self.board.view().calibration,
                ok: sensors.ok,
            };
            self.done = !sensors.ok;
            return next(if sensors.ok { SAMPLE_MS } else { 0 }, record);
        }

        let n = self.samples;
        self.samples += 1;

        let mut reading = SensorReading::default();
        let begin = sdk::systimer_ticks();
        let mut supply = [0u16; SAMPLES];
        let ok = self.board.capture(&mut reading).is_ok()
            && self
                .board
                .sample_channel(plaf103::pins::ADC_SUPPLY, &mut supply);
        let ticks = sdk::systimer_ticks().wrapping_sub(begin);
        let stopped = stopped::observe().verified();

        if !ok || !stopped {
            self.board.close();
            self.done = true;
            return next(0, Record::Error { n, ticks, stopped });
        }

        let calibration = self.board.view().calibration;
        next(
            SAMPLE_MS,
            Record::Sample {
                n,
                ticks,
                food: reading.food_high,
                outlet: reading.outlet_high,
                dc: reading.dc_present,
                stopped,
                current: Channel::of(&reading.current, &calibration),
                battery: Channel::of(&reading.battery, &calibration),
                supply: Channel::of(&supply, &calibration),
            },
        )
    }
}
