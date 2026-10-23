//! A finalized receive-only FW14 power-input capture against its build artifact.
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

use std::path::Path;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};
use serde_json::{Value, json};

use crate::capture::seconds;
use crate::startup::{self, contains, count, find};
use crate::{Report, Result, digest, require, round, update};

const POLICIES: [&str; 2] = ["fw14-adc-v1", "fw14-adc-v2"];
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

fn record(kind: char) -> String {
    match kind {
        'C' => format!(
            r"PLADC1 clock function_before={HEX8} clock_before={HEX8} function_after={HEX8} clock_after={HEX8} status=(pass|fail)\r\n"
        ),
        'O' => format!(
            r"PLADC1 open outputs=([01]) efuse=([0-9a-f]{{8}}|none) offset={DEC} gain={DEC} default_offset=([01]) default_gain=([01]) status=(pass|fail)\r\n"
        ),
        'S' => format!(
            r"PLADC1 sample n={DEC} ticks={DEC} food=([01]) outlet=([01]) dc=([01]) stopped=([01]) current={CHANNEL} battery={CHANNEL}(?: supply={CHANNEL})?\r\n"
        ),
        _ => format!(r"PLADC1 error n={DEC} ticks={DEC} stopped=([01])\r\n"),
    }
}

fn exact(kind: char) -> Regex {
    Regex::new(&format!("(?-u)^(?:{})$", record(kind))).expect("ADC pattern")
}

static ANY: LazyLock<Regex> = LazyLock::new(|| {
    let alternatives: Vec<String> = ['C', 'O', 'S', 'E']
        .iter()
        .map(|k| format!("(?P<{k}>{})", record(*k)))
        .collect();
    Regex::new(&format!("(?-u){}", alternatives.join("|"))).expect("ADC pattern")
});

static CLOCK: LazyLock<Regex> = LazyLock::new(|| exact('C'));
static OPEN: LazyLock<Regex> = LazyLock::new(|| exact('O'));
static SAMPLE: LazyLock<Regex> = LazyLock::new(|| exact('S'));

/// A fixed-format ADC record: its kind, offset and bytes.
pub struct Record<'h> {
    pub kind: char,
    pub start: usize,
    pub bytes: &'h [u8],
}

/// Fixed-format ADC records, and whether every PLADC1 text parsed (other
/// text is reported, not echoed).
pub fn records(rx: &[u8]) -> (Vec<Record<'_>>, bool) {
    let found: Vec<Record> = ANY
        .captures_iter(rx)
        .map(|c| {
            let kind = ['C', 'O', 'S', 'E']
                .into_iter()
                .find(|k| c.name(&k.to_string()).is_some())
                .unwrap_or('E');
            let whole = c.get(0).expect("whole match");
            Record {
                kind,
                start: whole.start(),
                bytes: whole.as_bytes(),
            }
        })
        .collect();

    let intact = found.len() == count(rx, b"PLADC1");
    (found, intact)
}

/// Stock order: floor mean in tenths, minus offset, times 1000, over gain;
/// 0 at or below offset.
pub fn scaled(mean: i64, offset: i64, gain: i64) -> i64 {
    if mean * 10 <= offset {
        0
    } else {
        (mean * 10 - offset) * 1000 / gain
    }
}

/// (offset, gain, default_offset, default_gain) for the efuse bytes
/// 0x1d0..0x1d3 in read order.
pub fn calibration(efuse: &str) -> Option<[i64; 4]> {
    let raw: Vec<u8> = (0..efuse.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(efuse.get(i..i + 2)?, 16).ok())
        .collect::<Option<_>>()?;
    let [a, b, c, d] = <[u8; 4]>::try_from(raw).ok()?;
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

/// The ADC outcome for one startup's parsed records, filling fixed report fields.
pub fn check_adc(
    rx: &[u8],
    found: &[Record],
    arrival: startup::Arrival,
    result: &mut Report,
    supply: bool,
) -> Result<&'static str> {
    let kinds: String = found.iter().map(|r| r.kind).collect();
    if !(kinds.starts_with("CO") && kinds[2..].chars().all(|k| k == 'S')) {
        return Ok("adc_sequence");
    }

    match find(rx, b"PLLED1 done low=all\r\n") {
        Some(led_done) if found[0].start >= led_done => {}
        _ => return Ok("adc_sequence"),
    }

    let c = CLOCK.captures(found[0].bytes).expect("clock record");
    let hex = |i| u64::from_str_radix(&text(&c, i), 16).unwrap_or(0);
    let (before_f, before_c, after_f, after_c) = (hex(1), hex(2), hex(3), hex(4));
    result.insert(
        "adc_enabled_before".into(),
        json!(before_f & before_c & ADC_BIT != 0),
    );
    result.insert(
        "adc_clock".into(),
        json!({"function_before": format!("{before_f:08x}"), "clock_before": format!("{before_c:08x}"),
               "function_after": format!("{after_f:08x}"), "clock_after": format!("{after_c:08x}")}),
    );

    if text(&c, 5) != "pass"
        || after_f & after_c & ADC_BIT == 0
        || (after_f & !ADC_BIT, after_c & !ADC_BIT) != (before_f & !ADC_BIT, before_c & !ADC_BIT)
    {
        return Ok("adc_clock");
    }

    let o = OPEN.captures(found[1].bytes).expect("open record");
    let int = |i| text(&o, i).parse::<i64>().unwrap_or(0);
    let reported = [int(3), int(4), int(5), int(6)];
    result.insert(
        "adc_calibration".into(),
        json!({"offset": reported[0], "gain": reported[1], "default_offset": reported[2], "default_gain": reported[3]}),
    );

    let efuse = text(&o, 2);
    if text(&o, 1) != "1"
        || text(&o, 7) != "pass"
        || efuse == "none"
        || calibration(&efuse) != Some(reported)
    {
        return Ok("adc_open");
    }

    // Sample fields in capture-group order: 0 n, 1 ticks, 2-4 food/outlet/dc,
    // 5 stopped, then (low, high, mean, scaled) per channel at 6 current,
    // 10 battery and 14 supply; an absent supply group reads as -1.
    let samples: Vec<Vec<i64>> = found[2..]
        .iter()
        .map(|r| {
            let s = SAMPLE.captures(r.bytes).expect("sample record");
            (1..s.len())
                .map(|i| {
                    s.get(i).map_or(-1, |m| {
                        String::from_utf8_lossy(m.as_bytes()).parse().unwrap_or(-1)
                    })
                })
                .collect()
        })
        .collect();

    if samples.iter().any(|s| (s[14] >= 0) != supply) {
        return Ok("adc_sequence");
    }

    result.insert("adc_samples".into(), json!(samples.len()));
    if samples.is_empty() {
        return Ok("adc_samples");
    }

    let ticks_max = samples.iter().map(|s| s[1]).max().unwrap_or(0);
    result.insert("adc_ticks_max".into(), json!(ticks_max));
    result.insert(
        "adc_capture_ms_max".into(),
        json!(round(ticks_max as f64 * 1000.0 / 32768.0, 2)),
    );

    for (name, index) in [("food", 2), ("outlet", 3), ("dc", 4)] {
        let mut levels: Vec<i64> = samples.iter().map(|s| s[index]).collect();
        levels.sort();
        levels.dedup();
        result.insert(format!("adc_{name}_levels"), json!(levels));
    }

    let mut channels = vec![("current", 6), ("battery", 10)];
    if supply {
        channels.push(("supply", 14));
    }

    for (name, base) in &channels {
        let values: Vec<&[i64]> = samples.iter().map(|s| &s[*base..base + 4]).collect();
        let max = |f: &dyn Fn(&[i64]) -> i64| values.iter().map(|v| f(v)).max().unwrap_or(0);
        let min = |f: &dyn Fn(&[i64]) -> i64| values.iter().map(|v| f(v)).min().unwrap_or(0);
        result.insert(
            format!("adc_{name}"),
            json!({"min": min(&|v| v[0]), "max": max(&|v| v[1]), "mean_min": min(&|v| v[2]),
                   "mean_max": max(&|v| v[2]), "spread_max": max(&|v| v[1] - v[0]),
                   "scaled_min": min(&|v| v[3]), "scaled_max": max(&|v| v[3])}),
        );
    }

    for (number, s) in samples.iter().enumerate() {
        if s[0] != number as i64 || s[5] != 1 || !(0 < s[1] && s[1] <= CAPTURE_TICKS) {
            return Ok("adc_samples");
        }
        for (_, base) in &channels {
            let (low, high, mean, value) = (s[*base], s[base + 1], s[base + 2], s[base + 3]);
            if !(low <= mean && mean <= high && high <= MAX_RAW)
                || value != scaled(mean, reported[0], reported[1])
            {
                return Ok("adc_values");
            }
        }
    }

    if supply
        && samples.iter().any(|s| {
            [6, 10]
                .iter()
                .any(|base| (s[16] - s[base + 2]).abs() < DISTINCT_RAW)
        })
    {
        return Ok("adc_channels");
    }

    if samples.len() < MIN_SAMPLES {
        return Ok("adc_samples");
    }

    let first = arrival(found[2].start)?;
    let last = arrival(found[found.len() - 1].start)?;
    let spacing = round(seconds(last, first) / (samples.len() - 1) as f64, 3);
    result.insert("adc_sample_seconds".into(), json!(spacing));
    if !(SAMPLE_SECONDS.0 <= spacing && spacing <= SAMPLE_SECONDS.1) {
        return Ok("adc_timing");
    }

    Ok("pass")
}

pub fn verify(capture: &Path, manifest: &Path, image: &Path) -> Result<Report> {
    let loaded = startup::load(capture, manifest, image)?;
    let policy = loaded.manifest.startup_adc_policy.as_deref();
    require(
        policy.is_some_and(|p| POLICIES.contains(&p)),
        "ADC policy required",
    )?;

    let c = &loaded.capture;
    let rx = &c.rx;
    let arrival = |offset: usize| c.arrival(offset);
    let mut result: Report = match json!({"outcome": "pass", "startups": count(rx, b"PLFW1 start "),
        "adc_enabled_before": null, "adc_clock": null, "adc_calibration": null, "adc_samples": null,
        "adc_ticks_max": null, "adc_capture_ms_max": null, "adc_sample_seconds": null,
        "adc_food_levels": null, "adc_outlet_levels": null, "adc_dc_levels": null,
        "adc_current": null, "adc_battery": null, "adc_supply": null})
    {
        Value::Object(map) => map,
        _ => unreachable!(),
    };

    let end = crate::capture::timestamp(&c.metadata["end_utc"])?;
    let mut check = startup::startup(
        rx,
        &loaded.manifest,
        &loaded.image,
        &loaded.identity,
        Some(&arrival),
        Some(end),
    )?;
    check.remove("start_offset");
    let startup_outcome = check["outcome"].as_str().unwrap_or("").to_string();
    result.insert("startup".into(), Value::Object(check));

    let (found, intact) = records(rx);
    let outcome = if startup_outcome != "pass" {
        format!("startup_{startup_outcome}")
    } else if !intact
        || FOREIGN.iter().any(|f| contains(rx, f))
        || found.iter().any(|r| r.kind == 'E')
    {
        "adc_sequence".into()
    } else {
        check_adc(
            rx,
            &found,
            &arrival,
            &mut result,
            policy == Some("fw14-adc-v2"),
        )?
        .into()
    };
    result.insert("outcome".into(), json!(outcome));

    let mut tail = Report::new();
    tail.insert("build_id".into(), json!(loaded.identity));
    tail.insert("ota_sha256".into(), json!(digest(&loaded.image)));
    tail.insert("rx_sha256".into(), json!(digest(rx)));
    tail.insert("tx_bytes".into(), json!(c.tx.len()));
    tail.insert("limit".into(), json!(LIMIT));
    update(&mut result, &tail);
    Ok(result)
}

#[cfg(test)]
mod tests;
