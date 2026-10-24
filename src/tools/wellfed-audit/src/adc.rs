//! A finalized receive-only power-input capture against its build artifact.
//!
//! Port of `audit-firmware-adc.py`. A passing capture is one startup whose LED
//! probe completes, then the read-only ADC probe:
//! - The explicit LP ADC clock and function enable reads back with no other bit changed.
//! - The efuse calibration bytes decode to the reported offset and gain.
//! - At least 30 samples follow, numbered from 0, about a second apart. Each has
//!   stopped outputs, every value in range, and each scaled value matching the
//!   stock formula on its mean.
//!
//! Under fw14-adc-v2 every sample also carries internal channel 10 (the chip's
//! own supply). Its mean must differ from both external channels' means by at
//! least 1000 raw: evidence that the channel select takes effect. The input
//! values themselves are otherwise observations; their electrical meaning
//! needs a measurement.

use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};
use serde::Serialize;

use crate::capture::seconds;
use crate::startup::{
    self, Arrival, Evidence, LED_DONE, Outcome, StartupReport, contains, count, find,
};
use crate::{Error, Result, round};

/// Under fw14-adc-v2, the least gap between the internal supply channel's mean
/// and each external channel's mean that shows the channel select took effect.
const DISTINCT_RAW: i64 = 1000;

/// BIT_LSYS_ADC_FEN / BIT_LSYS_ADC_CKE.
const ADC_BIT: u64 = 1 << 8;

const MIN_SAMPLES: usize = 30;
/// Mean host spacing of samples; the firmware waits 1000 ms after each.
const SAMPLE_SECONDS: (f64, f64) = (0.8, 1.5);
/// 100 ms at 32 768 Hz for 32 conversions.
const CAPTURE_TICKS: i64 = 3277;

const MAX_RAW: i64 = 4095;
const DEFAULT_OFFSET: i64 = 2480;
const DEFAULT_GAIN: i64 = 12050;

/// Other diagnostics' record tags: an ADC build must emit none of them.
const FOREIGN: [&[u8]; 7] = [
    b"PLFLS1", b"PLPRK1", b"PLRDS1", b"PLWRS1", b"PLJNL1", b"PLSET1", b"PLTMR1",
];

const LIMIT: &str = "UART evidence of register readback and raw conversions; host timestamps carry USB latency. Voltages, \
    divider ratios, current units and the external-power polarity are not established by this capture.";

const DEC: &str = r"(\d{1,10})";
const HEX8: &str = "([0-9a-f]{8})";
const CHANNEL: &str = r"(\d{1,4}),(\d{1,4}),(\d{1,4}),(\d{1,10})";

/// The ADC probe's record version, from `startup_adc_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdcPolicy {
    /// fw14-adc-v1: the two external channels.
    V1,
    /// fw14-adc-v2: plus the internal supply channel.
    V2,
}

/// The audit's outcome: a failed startup, an ADC check, or `pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdcOutcome {
    Pass,
    /// The startup before the probe failed with this outcome.
    Startup(Outcome),
    Sequence,
    Clock,
    Open,
    Samples,
    Values,
    Channels,
    Timing,
}

impl fmt::Display for AdcOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pass => "pass",
            Self::Startup(outcome) => return write!(f, "startup_{outcome}"),
            Self::Sequence => "adc_sequence",
            Self::Clock => "adc_clock",
            Self::Open => "adc_open",
            Self::Samples => "adc_samples",
            Self::Values => "adc_values",
            Self::Channels => "adc_channels",
            Self::Timing => "adc_timing",
        })
    }
}

impl Serialize for AdcOutcome {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Clock,
    Open,
    Sample,
    Error,
}

const KINDS: [(Kind, &str); 4] = [
    (Kind::Clock, "C"),
    (Kind::Open, "O"),
    (Kind::Sample, "S"),
    (Kind::Error, "E"),
];

fn record(kind: Kind) -> String {
    match kind {
        Kind::Clock => format!(
            r"PLADC1 clock function_before={HEX8} clock_before={HEX8} function_after={HEX8} clock_after={HEX8} status=(pass|fail)\r\n"
        ),
        Kind::Open => format!(
            r"PLADC1 open outputs=([01]) efuse=([0-9a-f]{{8}}|none) offset={DEC} gain={DEC} default_offset=([01]) default_gain=([01]) status=(pass|fail)\r\n"
        ),
        Kind::Sample => format!(
            r"PLADC1 sample n={DEC} ticks={DEC} food=([01]) outlet=([01]) dc=([01]) stopped=([01]) current={CHANNEL} battery={CHANNEL}(?: supply={CHANNEL})?\r\n"
        ),
        Kind::Error => format!(r"PLADC1 error n={DEC} ticks={DEC} stopped=([01])\r\n"),
    }
}

fn exact(kind: Kind) -> Regex {
    Regex::new(&format!("(?-u)^(?:{})$", record(kind))).expect("ADC pattern")
}

static ANY: LazyLock<Regex> = LazyLock::new(|| {
    let alternatives: Vec<String> = KINDS
        .iter()
        .map(|(kind, name)| format!("(?P<{name}>{})", record(*kind)))
        .collect();
    Regex::new(&format!("(?-u){}", alternatives.join("|"))).expect("ADC pattern")
});

static CLOCK: LazyLock<Regex> = LazyLock::new(|| exact(Kind::Clock));
static OPEN: LazyLock<Regex> = LazyLock::new(|| exact(Kind::Open));
static SAMPLE: LazyLock<Regex> = LazyLock::new(|| exact(Kind::Sample));

/// A fixed-format ADC record: its kind, offset and bytes.
struct Record<'h> {
    kind: Kind,
    start: usize,
    bytes: &'h [u8],
}

impl<'h> Record<'h> {
    /// The record's fields under its own kind's pattern, which matches it
    /// exactly because [`ANY`] found it with that alternative.
    fn fields(&self, pattern: &Regex) -> Captures<'h> {
        pattern
            .captures(self.bytes)
            .expect("ANY matched this record with the same pattern")
    }
}

/// Fixed-format ADC records, and whether every PLADC1 text parsed (other
/// text is reported, not echoed).
fn records(rx: &[u8]) -> (Vec<Record<'_>>, bool) {
    let found: Vec<Record> = ANY
        .captures_iter(rx)
        .filter_map(|c| {
            let (kind, _) = KINDS.iter().find(|(_, name)| c.name(name).is_some())?;
            let whole = c.get(0)?;
            Some(Record {
                kind: *kind,
                start: whole.start(),
                bytes: whole.as_bytes(),
            })
        })
        .collect();

    let intact = found.len() == count(rx, b"PLADC1");
    (found, intact)
}

/// Stock order: floor mean in tenths, minus offset, times 1000, over gain;
/// 0 at or below offset.
fn scaled(mean: i64, offset: i64, gain: i64) -> i64 {
    if mean * 10 <= offset {
        0
    } else {
        (mean * 10 - offset) * 1000 / gain
    }
}

/// (offset, gain, default_offset, default_gain) for the efuse bytes
/// 0x1d0..0x1d3 in read order.
fn calibration(efuse: &str) -> Option<[i64; 4]> {
    let [a, b, c, d] = wellfed_formats::hex::decode::<4>(efuse)?;
    let (offset, gain) = (
        i64::from(u16::from_le_bytes([a, b])),
        i64::from(u16::from_le_bytes([c, d])),
    );

    Some([
        if offset == 0xffff {
            DEFAULT_OFFSET
        } else {
            offset
        },
        if gain == 0xffff { DEFAULT_GAIN } else { gain },
        i64::from(offset == 0xffff),
        i64::from(gain == 0xffff),
    ])
}

fn text(c: &Captures, i: usize) -> String {
    c.get(i).map_or(String::new(), |m| {
        String::from_utf8_lossy(m.as_bytes()).into_owned()
    })
}

/// One channel's conversion: the 32 conversions' extremes and mean, and the
/// firmware's scaled value of that mean.
#[derive(Debug, Clone, Copy)]
struct Conversion {
    low: i64,
    high: i64,
    mean: i64,
    scaled: i64,
}

struct Sample {
    n: i64,
    ticks: i64,
    food: i64,
    outlet: i64,
    dc: i64,
    stopped: i64,
    current: Conversion,
    battery: Conversion,
    supply: Option<Conversion>,
}

impl Sample {
    fn parse(record: &Record) -> Self {
        let s = record.fields(&SAMPLE);
        let int = |i: usize| -> Option<i64> {
            String::from_utf8_lossy(s.get(i)?.as_bytes()).parse().ok()
        };
        let number = |i: usize| int(i).unwrap_or(-1);
        let conversion = |at: usize| -> Option<Conversion> {
            Some(Conversion {
                low: int(at)?,
                high: int(at + 1)?,
                mean: int(at + 2)?,
                scaled: int(at + 3)?,
            })
        };
        let absent = Conversion {
            low: -1,
            high: -1,
            mean: -1,
            scaled: -1,
        };

        Self {
            n: number(1),
            ticks: number(2),
            food: number(3),
            outlet: number(4),
            dc: number(5),
            stopped: number(6),
            current: conversion(7).unwrap_or(absent),
            battery: conversion(11).unwrap_or(absent),
            supply: conversion(15),
        }
    }

    fn channels(&self) -> impl Iterator<Item = Conversion> {
        [Some(self.current), Some(self.battery), self.supply]
            .into_iter()
            .flatten()
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdcClock {
    function_before: String,
    clock_before: String,
    function_after: String,
    clock_after: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Calibration {
    offset: i64,
    gain: i64,
    default_offset: i64,
    default_gain: i64,
}

/// One channel across every sample.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChannelSummary {
    min: i64,
    max: i64,
    mean_min: i64,
    mean_max: i64,
    spread_max: i64,
    scaled_min: i64,
    scaled_max: i64,
}

impl ChannelSummary {
    fn of(values: &[Conversion]) -> Self {
        let max = |f: fn(&Conversion) -> i64| values.iter().map(f).max().unwrap_or(0);
        let min = |f: fn(&Conversion) -> i64| values.iter().map(f).min().unwrap_or(0);
        Self {
            min: min(|v| v.low),
            max: max(|v| v.high),
            mean_min: min(|v| v.mean),
            mean_max: max(|v| v.mean),
            spread_max: max(|v| v.high - v.low),
            scaled_min: min(|v| v.scaled),
            scaled_max: max(|v| v.scaled),
        }
    }
}

/// The probe's fixed fields, each written as `null` until its check runs.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct AdcFields {
    adc_enabled_before: Option<bool>,
    adc_clock: Option<AdcClock>,
    adc_calibration: Option<Calibration>,
    adc_samples: Option<usize>,
    adc_ticks_max: Option<i64>,
    adc_capture_ms_max: Option<f64>,
    adc_sample_seconds: Option<f64>,
    adc_food_levels: Option<Vec<i64>>,
    adc_outlet_levels: Option<Vec<i64>>,
    adc_dc_levels: Option<Vec<i64>>,
    adc_current: Option<ChannelSummary>,
    adc_battery: Option<ChannelSummary>,
    adc_supply: Option<ChannelSummary>,
}

fn levels(samples: &[Sample], level: fn(&Sample) -> i64) -> Vec<i64> {
    let mut levels: Vec<i64> = samples.iter().map(level).collect();
    levels.sort();
    levels.dedup();
    levels
}

/// The clock record: the ADC bit set in both registers, nothing else changed.
fn check_clock(record: &Record, result: &mut AdcFields) -> bool {
    let c = record.fields(&CLOCK);
    let hex = |i| u64::from_str_radix(&text(&c, i), 16).unwrap_or(0);
    let (before_f, before_c, after_f, after_c) = (hex(1), hex(2), hex(3), hex(4));
    result.adc_enabled_before = Some(before_f & before_c & ADC_BIT != 0);
    result.adc_clock = Some(AdcClock {
        function_before: format!("{before_f:08x}"),
        clock_before: format!("{before_c:08x}"),
        function_after: format!("{after_f:08x}"),
        clock_after: format!("{after_c:08x}"),
    });

    text(&c, 5) == "pass"
        && after_f & after_c & ADC_BIT != 0
        && (after_f & !ADC_BIT, after_c & !ADC_BIT) == (before_f & !ADC_BIT, before_c & !ADC_BIT)
}

/// The open record: outputs stopped, and the reported calibration is what
/// the efuse bytes decode to.
fn check_open(record: &Record, result: &mut AdcFields) -> Option<[i64; 4]> {
    let o = record.fields(&OPEN);
    let int = |i| text(&o, i).parse::<i64>().unwrap_or(0);
    let reported = [int(3), int(4), int(5), int(6)];
    result.adc_calibration = Some(Calibration {
        offset: reported[0],
        gain: reported[1],
        default_offset: reported[2],
        default_gain: reported[3],
    });

    let efuse = text(&o, 2);
    (text(&o, 1) == "1"
        && text(&o, 7) == "pass"
        && efuse != "none"
        && calibration(&efuse) == Some(reported))
    .then_some(reported)
}

/// The ADC outcome for one startup's parsed records, filling fixed report fields.
fn check_adc(
    rx: &[u8],
    found: &[Record],
    arrival: Arrival,
    result: &mut AdcFields,
    policy: AdcPolicy,
) -> Result<AdcOutcome> {
    let kinds: Vec<Kind> = found.iter().map(|r| r.kind).collect();
    let [Kind::Clock, Kind::Open, rest @ ..] = &kinds[..] else {
        return Ok(AdcOutcome::Sequence);
    };
    if rest.iter().any(|k| *k != Kind::Sample) {
        return Ok(AdcOutcome::Sequence);
    }

    match find(rx, LED_DONE) {
        Some(led_done) if found[0].start >= led_done => {}
        _ => return Ok(AdcOutcome::Sequence),
    }

    if !check_clock(&found[0], result) {
        return Ok(AdcOutcome::Clock);
    }
    let Some([offset, gain, ..]) = check_open(&found[1], result) else {
        return Ok(AdcOutcome::Open);
    };

    let samples: Vec<Sample> = found[2..].iter().map(Sample::parse).collect();
    let supply = policy == AdcPolicy::V2;
    if samples.iter().any(|s| s.supply.is_some() != supply) {
        return Ok(AdcOutcome::Sequence);
    }

    result.adc_samples = Some(samples.len());
    if samples.is_empty() {
        return Ok(AdcOutcome::Samples);
    }
    summarize(&samples, supply, result);

    for (number, s) in samples.iter().enumerate() {
        if usize::try_from(s.n) != Ok(number)
            || s.stopped != 1
            || !(0 < s.ticks && s.ticks <= CAPTURE_TICKS)
        {
            return Ok(AdcOutcome::Samples);
        }
        for c in s.channels() {
            if !(c.low <= c.mean && c.mean <= c.high && c.high <= MAX_RAW)
                || c.scaled != scaled(c.mean, offset, gain)
            {
                return Ok(AdcOutcome::Values);
            }
        }
    }

    let distinct = |s: &Sample| {
        s.supply.is_none_or(|supply| {
            [s.current, s.battery]
                .iter()
                .all(|c| (supply.mean - c.mean).abs() >= DISTINCT_RAW)
        })
    };
    if !samples.iter().all(distinct) {
        return Ok(AdcOutcome::Channels);
    }

    if samples.len() < MIN_SAMPLES {
        return Ok(AdcOutcome::Samples);
    }

    let first = arrival(found[2].start)?;
    let last = arrival(found[found.len() - 1].start)?;
    let spacing = round(seconds(last, first) / (samples.len() - 1) as f64, 3);
    result.adc_sample_seconds = Some(spacing);
    if !(SAMPLE_SECONDS.0 <= spacing && spacing <= SAMPLE_SECONDS.1) {
        return Ok(AdcOutcome::Timing);
    }

    Ok(AdcOutcome::Pass)
}

/// The samples' ranges, reported whether or not each sample passes.
fn summarize(samples: &[Sample], supply: bool, result: &mut AdcFields) {
    let ticks_max = samples.iter().map(|s| s.ticks).max().unwrap_or(0);
    result.adc_ticks_max = Some(ticks_max);
    result.adc_capture_ms_max = Some(round(ticks_max as f64 * 1000.0 / 32768.0, 2));

    result.adc_food_levels = Some(levels(samples, |s| s.food));
    result.adc_outlet_levels = Some(levels(samples, |s| s.outlet));
    result.adc_dc_levels = Some(levels(samples, |s| s.dc));

    let summary = |channel: fn(&Sample) -> Option<Conversion>| {
        let values: Vec<Conversion> = samples.iter().filter_map(channel).collect();
        ChannelSummary::of(&values)
    };
    result.adc_current = Some(summary(|s| Some(s.current)));
    result.adc_battery = Some(summary(|s| Some(s.battery)));
    if supply {
        result.adc_supply = Some(summary(|s| s.supply));
    }
}

/// The ADC audit's report.
#[derive(Debug, Clone, Serialize)]
pub struct AdcReport {
    outcome: AdcOutcome,
    startups: usize,
    #[serde(flatten)]
    adc: AdcFields,
    startup: StartupReport,
    #[serde(flatten)]
    evidence: Evidence,
    limit: &'static str,
}

impl AdcReport {
    pub fn passed(&self) -> bool {
        self.outcome == AdcOutcome::Pass
    }
}

pub fn verify(capture: &Path, manifest: &Path, image: &Path) -> Result<AdcReport> {
    let loaded = startup::load(capture, manifest, image)?;
    let policy = match loaded.manifest.startup_adc_policy.as_deref() {
        Some("fw14-adc-v1") => AdcPolicy::V1,
        Some("fw14-adc-v2") => AdcPolicy::V2,
        _ => return Err(Error::Refused("ADC policy required".into())),
    };

    let c = &loaded.capture;
    let rx = &c.rx;
    let arrival = |offset: usize| c.arrival(offset);
    let end = c.end()?;
    let check = startup::startup(
        rx,
        &loaded.manifest,
        &loaded.image,
        &loaded.identity,
        Some(&arrival),
        Some(end),
    )?;

    let mut adc = AdcFields::default();
    let (found, intact) = records(rx);
    let outcome = if check.outcome != Outcome::Pass {
        AdcOutcome::Startup(check.outcome)
    } else if !intact
        || FOREIGN.iter().any(|f| contains(rx, f))
        || found.iter().any(|r| r.kind == Kind::Error)
    {
        AdcOutcome::Sequence
    } else {
        check_adc(rx, &found, &arrival, &mut adc, policy)?
    };

    Ok(AdcReport {
        outcome,
        startups: count(rx, b"PLFW1 start "),
        adc,
        startup: check,
        evidence: loaded.evidence(),
        limit: LIMIT,
    })
}

#[cfg(test)]
mod tests;
