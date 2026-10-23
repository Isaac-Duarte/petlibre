//! A finalized receive-only diagnostic startup against its build artifact.
//!
//! Port of `audit-firmware-startup.py`: the same policies, record formats,
//! outcomes and report fields. Only fixed public fields are parsed; arbitrary
//! device text is never returned.
//!
//! `load` binds the evidence before anything is classified: the manifest's
//! `startup_identity` is the hash of its profile and inputs, the image is the
//! manifest's OTA and embeds that identity, and the capture is finalized and
//! receive-only. Each `startup_*_policy` the manifest declares must match the
//! record formats linked into the image, mostly both ways, so a build cannot
//! be audited under a different probe's rules. One startup passes when its
//! start record names the build and its live bank mapping, outputs were
//! stopped, the synthetic diagnostic passed, and every declared probe's
//! records classify as pass. Record formats and policies are described in
//! docs/firmware/startup-diagnostics.md.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use wellfed_formats::manifest::BuildManifest;
use regex::bytes::{Captures, Match, Regex};
use serde::Serialize;

use crate::capture::{Capture, Time, seconds};
use crate::{Error, Result, digest, outcomes, require};

mod flash;
mod policy;

pub(crate) use flash::{FlashPolicy, FlashReport, classify_flash};
use policy::Policies;

fn re(pattern: &str) -> Regex {
    // Byte-wise ASCII semantics, as Python's bytes patterns.
    Regex::new(&format!("(?-u){pattern}")).expect("audit pattern")
}

macro_rules! pattern {
    ($name:ident, $text:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| re($text));
    };
}
pub(crate) use pattern;

pattern!(
    START,
    r"PLFW1 start build=([0-9a-f]{64}) bank=(0|1|unknown) reset=([0-9a-f]{8}) mmu=([0-9a-f]{8}),([0-9a-f]{8}),([0-9a-f]{8})\r\n"
);
pub(crate) const PASS: &[u8] = b"PLFW1 result diagnostic=pass backend=unopened\r\n";
const FAIL: &[u8] = b"PLFW1 result diagnostic=fail backend=unopened\r\n";
pattern!(
    OUTPUTS,
    r"PLFW1 outputs pa=([0-9a-f]{8}) pb=([0-9a-f]{8}) dira=([0-9a-f]{8}) dirb=([0-9a-f]{8}) status=(pass|fail)\r\n"
);

pattern!(
    POWER_V1,
    r"PLPWR1 before_func=([0-9a-f]{8}) before_pad=([0-9a-f]{8}) after_func=([0-9a-f]{8}) after_pad=([0-9a-f]{8})()\n\r"
);
pattern!(
    POWER_V3,
    r"PLPWR1 before_func=([0-9a-f]{8}) before_pad=([0-9a-f]{8}) after_func=([0-9a-f]{8}) after_pad=([0-9a-f]{8}) ext_a=((?:[0-9a-f]{8},){3}[0-9a-f]{8})\n\r"
);

pattern!(
    ROUTING_V1,
    r"PLIO1 stage=(startup|PA0|PA2|PA4|PB22) groups=([0-9a-f]{8}) aon=([0-9a-f]{8}) control_a=([0-9a-f]{8}) control_b=([0-9a-f]{8})\r\n"
);
pattern!(
    ROUTING_V2,
    r"PLIO1 stage=(startup|PA0|PA2|PA4|PB22) groups=([0-9a-f]{8}) aon=([0-9a-f]{8}) control_a=([0-9a-f]{8}) control_b=([0-9a-f]{8}) share_pad=([0-9a-f]{8}) ext_a=([0-9a-f]{8})\r\n"
);

pattern!(
    READBACK_V1,
    r"PLDBG1 pin=(PA0|PA2|PA4|PB22) stage=(request|verify|initialize) want=([01]) latch=([01]) input=([01])() directions=([01]) stopped=([01]) pad=([0-9a-f]{8})\r\n"
);
pattern!(
    READBACK_V2,
    r"PLDBG1 pin=(PA0|PA2|PA4|PB22) stage=(request|verify|initialize) want=([01]) latch=([01]) input=([01]) highs=([0-9]|[1-5][0-9]|6[0-4])/64 directions=([01]) stopped=([01]) pad=([0-9a-f]{8})\r\n"
);

pattern!(LED_RECORD, r"PLLED1[^\r\n]*\r\n");
pattern!(LED_FAIL, r"PLLED1 pin=[^\r\n]* status=fail\r\n");
/// The LED probe's last record; the flash and ADC probes follow it.
pub(crate) const LED_DONE: &[u8] = b"PLLED1 done low=all\r\n";

const AMACRO: u64 = 1 << 28;
/// PORB, MBIAS and LDO trim/power, per the stock trace.
const SHARE_PAD_HELPER: u64 = 0x3ff;

// Each probe's record format, as linked into the image, binds its policy.
const SUPERVISION_MARK: &[u8] = b"PLTMR1 open board=";
const FLASH_MARK: &[u8] = b"PLFLS1 hello authority=";
/// FW06 stage C park round trip follows READY.
const PARK_MARK: &[u8] = b"PLPRK1 start mode=";
/// FW06 stage D mediated flash read follows READY.
const READ_MARK: &[u8] = b"PLRDS1 start marker=";
/// FW06 stage E owner-approved scratch-sector write follows READY.
const WRITE_MARK: &[u8] = b"PLWRS1 start marker=";
/// FW15 flash journal on NOR follows READY.
const JOURNAL_MARK: &[u8] = b"PLJNL1 start mode=";
/// FW15 settings-store power-cut stress follows READY.
const SETTINGS_MARK: &[u8] = b"PLSET1 start marker=";
/// FW14 read-only power-input samples follow the LED probe.
pub(crate) const ADC_MARK: &[u8] = b"PLADC1 open outputs=";
/// fw14-adc-v2 samples add internal channel 10.
const ADC_SUPPLY_MARK: &[u8] = b" supply=";
const WIFI_MARK: &[u8] = b"PLWIF1 start created=";
const WIFI_JOIN_MARK: &[u8] = b"PLWIF2 start created=";
const WIFI_DNS_MARK: &[u8] = b"PLWIF3 start created=";
const TRANSPORT_MARK: &[u8] = b"PLMQT1 start created=";
/// FW11 trial test: the watchdog resets it after startup.
const TRIAL_RESET_MARK: &[u8] = b"PLTRL1 test=reset watchdog=";
const OPTICS_MARK: &[u8] = b"PLOPT1 start created=";
/// fw13-optics-v2: 5 s windows.
const OPTICS_PLAN2_MARK: &[u8] = b" plan=2\r\n";
/// FW16 single supervised pulse follows the LED probe.
const MOTION_MARK: &[u8] = b"PLMOT1 start created=";
/// The KM0 printf format of the before-KM4 (v3) shared-pad record, as linked.
const POWER_V3_FORMAT: &[u8] = b"after_pad=%08x ext_a=%08x,%08x,%08x,%08x";

pattern!(
    CLOCK,
    r"PLCLK1 function_before=([0-9a-f]{8}) clock_before=([0-9a-f]{8}) function_after=([0-9a-f]{8}) clock_after=([0-9a-f]{8}) status=(pass|fail)\r\n"
);

const OUTPUT_ONLY: [&str; 3] = ["PA0", "PA2", "PA4"];
const LED_PINS: [&str; 4] = ["PA0", "PA2", "PA4", "PB22"];

outcomes! {
    /// A startup's outcome: the first failing check among the start record
    /// and each declared probe, or `pass`.
    pub enum Outcome {
        Pass => "pass",
        Timeout => "timeout",
        Fault => "fault",
        RepeatedStartup => "repeated_startup",
        OutputsFailed => "outputs_failed",
        DiagnosticFailed => "diagnostic_failed",
        MalformedReport => "malformed_report",
        WrongBuild => "wrong_build",
        UnknownMapping => "unknown_mapping",
        OutputsMissing => "outputs_missing",
        OutputsAfterStartup => "outputs_after_startup",
        IncompleteReport => "incomplete_report",
        ObservationTooShort => "observation_too_short",
        GpioClockMissing => "gpio_clock_missing",
        GpioClockRepeated => "gpio_clock_repeated",
        GpioClockFailed => "gpio_clock_failed",
        GpioClockOrder => "gpio_clock_order",
        LedFailed => "led_failed",
        LedMissing => "led_missing",
        LedUnexpectedSequence => "led_unexpected_sequence",
        LedIncomplete => "led_incomplete",
        GpioRoutingIncomplete => "gpio_routing_incomplete",
        SharedPadMissing => "shared_pad_missing",
        SharedPadMalformed => "shared_pad_malformed",
        SharedPadFailed => "shared_pad_failed",
        SharedPadOrder => "shared_pad_order",
        FlashHandshakeMissing => "flash_handshake_missing",
        FlashHandshakeMalformed => "flash_handshake_malformed",
        FlashHandshakeFailed => "flash_handshake_failed",
        FlashHandshakeOrder => "flash_handshake_order",
        FlashHandshakeTiming => "flash_handshake_timing",
        FlashReadMalformed => "flash_read_malformed",
        FlashReadFailed => "flash_read_failed",
        FlashReadMissing => "flash_read_missing",
        FlashReadOrder => "flash_read_order",
        FlashReadTiming => "flash_read_timing",
        FlashWriteMalformed => "flash_write_malformed",
        FlashWriteFailed => "flash_write_failed",
        FlashWriteMissing => "flash_write_missing",
        FlashWriteOrder => "flash_write_order",
        FlashWriteTiming => "flash_write_timing",
    }
}

/// One check's outcome and the report fields it fills.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Checked<T> {
    pub(crate) outcome: Outcome,
    #[serde(flatten)]
    pub(crate) fields: T,
}

/// Non-overlapping occurrences, as `bytes.count`.
pub(crate) fn count(haystack: &[u8], needle: &[u8]) -> usize {
    let (mut n, mut at) = (0, 0);
    while let Some(i) = find_from(haystack, needle, at) {
        n += 1;
        at = i + needle.len().max(1);
    }
    n
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    find_from(haystack, needle, 0)
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= haystack.len()).then_some(from);
    }
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

fn group<'h>(c: &Captures<'h>, i: usize) -> &'h [u8] {
    c.get(i).map_or(&[][..], |m| m.as_bytes())
}

fn text(c: &Captures, i: usize) -> String {
    String::from_utf8_lossy(group(c, i)).into_owned()
}

fn hex(c: &Captures, i: usize) -> u64 {
    u64::from_str_radix(&text(c, i), 16).unwrap_or(0)
}

fn dec(c: &Captures, i: usize) -> u64 {
    text(c, i).parse().unwrap_or(0)
}

fn decs(c: &Captures, from: usize) -> Vec<u64> {
    (from..c.len()).map(|i| dec(c, i)).collect()
}

fn whole<'h>(c: &Captures<'h>) -> Match<'h> {
    c.get(0).expect("group 0 is the whole match")
}

fn all<'h>(pattern: &Regex, rx: &'h [u8]) -> Vec<Captures<'h>> {
    pattern.captures_iter(rx).collect()
}

pub(crate) type Arrival<'a> = &'a dyn Fn(usize) -> Result<Time>;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ClockReport {
    pub(crate) gpio_clock_verified: bool,
    #[serde(flatten)]
    pub(crate) registers: Option<ClockRegisters>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ClockRegisters {
    pub(crate) gpio_function_before: String,
    pub(crate) gpio_clock_before: String,
    pub(crate) gpio_function_after: String,
    pub(crate) gpio_clock_after: String,
}

pub(crate) fn classify_clock(rx: &[u8]) -> Checked<ClockReport> {
    let values = all(&CLOCK, rx);
    if values.len() != 1 || count(rx, b"PLCLK1") != 1 {
        let outcome = if values.is_empty() {
            Outcome::GpioClockMissing
        } else {
            Outcome::GpioClockRepeated
        };
        return Checked {
            outcome,
            fields: ClockReport {
                gpio_clock_verified: false,
                registers: None,
            },
        };
    }

    let m = &values[0];
    let (before_function, before_clock, function, clock) =
        (hex(m, 1), hex(m, 2), hex(m, 3), hex(m, 4));
    let bit = 1u64 << 20;
    let ok = function & bit != 0
        && clock & bit != 0
        && function & !bit == before_function & !bit
        && clock & !bit == before_clock & !bit
        && group(m, 5) == b"pass";

    Checked {
        outcome: if ok {
            Outcome::Pass
        } else {
            Outcome::GpioClockFailed
        },
        fields: ClockReport {
            gpio_clock_verified: ok,
            registers: Some(ClockRegisters {
                gpio_function_before: text(m, 1),
                gpio_clock_before: text(m, 2),
                gpio_function_after: text(m, 3),
                gpio_clock_after: text(m, 4),
            }),
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PowerReport {
    pub(crate) shared_pad_verified: bool,
    #[serde(flatten)]
    pub(crate) registers: Option<PowerRegisters>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PowerRegisters {
    pub(crate) shared_pad_ldo_trim: String,
    pub(crate) shared_pad_function_before: String,
    pub(crate) shared_pad_control_before: String,
    pub(crate) shared_pad_function_after: String,
    pub(crate) shared_pad_control_after: String,
    /// Version 3 only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shared_pad_port_a_samples: Option<Vec<String>>,
}

/// One intact KM0 supply record, preserving unrelated bits, before any KM4 report.
///
/// Version 2 also requires the stock CODEC_Init LDO trim (0x2A or 0x68);
/// version 3 adds read-only raw Port A samples after each supply step.
pub(crate) fn classify_power(rx: &[u8], version: u8) -> Checked<PowerReport> {
    let values = all(if version >= 3 { &POWER_V3 } else { &POWER_V1 }, rx);
    if values.len() != 1 || count(rx, b"PLPWR1") != 1 {
        let outcome = if contains(rx, b"PLPWR1") {
            Outcome::SharedPadMalformed
        } else {
            Outcome::SharedPadMissing
        };
        return Checked {
            outcome,
            fields: PowerReport {
                shared_pad_verified: false,
                registers: None,
            },
        };
    }

    let m = &values[0];
    let (before_func, before_pad, func, pad) = (hex(m, 1), hex(m, 2), hex(m, 3), hex(m, 4));
    let km4: Vec<usize> = [&b"PLCLK1"[..], b"PLIO1", b"PLFW1"]
        .iter()
        .filter_map(|marker| find(rx, marker))
        .collect();

    let ok = func == before_func | AMACRO
        && pad & 0x301 == 0x301
        && pad & !SHARE_PAD_HELPER == before_pad & !SHARE_PAD_HELPER
        && (version < 2 || [0x2A, 0x68].contains(&((pad >> 1) & 0x7f)));

    let end = whole(m).end();
    let outcome = if !ok {
        Outcome::SharedPadFailed
    } else if km4.iter().min().is_some_and(|first| *first < end) {
        Outcome::SharedPadOrder
    } else {
        Outcome::Pass
    };

    let samples = (version >= 3).then(|| text(m, 5).split(',').map(str::to_string).collect());
    Checked {
        outcome,
        fields: PowerReport {
            shared_pad_verified: outcome == Outcome::Pass,
            registers: Some(PowerRegisters {
                shared_pad_ldo_trim: format!("{:02x}", (pad >> 1) & 0x7f),
                shared_pad_function_before: text(m, 1),
                shared_pad_control_before: text(m, 2),
                shared_pad_function_after: text(m, 3),
                shared_pad_control_after: text(m, 4),
                shared_pad_port_a_samples: samples,
            }),
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RoutingReport {
    pub(crate) gpio_routing_complete: bool,
    pub(crate) gpio_routing_records: Vec<RoutingRecord>,
}

/// One PLIO1 record's fixed fields, kept as the hex the firmware printed.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RoutingRecord {
    pub(crate) stage: String,
    pub(crate) groups: String,
    pub(crate) aon: String,
    pub(crate) control_a: String,
    pub(crate) control_b: String,
    /// Version 2 and later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) share_pad: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ext_a: Option<String>,
}

/// Keep fixed numeric observations without inferring pin-group membership.
/// Version 3 has the v2 fields but only the startup stage, since the LED
/// probe that added the per-pin stages is gone (`low-only-v1`).
pub(crate) fn classify_routing(rx: &[u8], version: u8) -> Checked<RoutingReport> {
    let pattern = if version >= 2 {
        &ROUTING_V2
    } else {
        &ROUTING_V1
    };
    let records: Vec<RoutingRecord> = all(pattern, rx)
        .iter()
        .map(|m| RoutingRecord {
            stage: text(m, 1),
            groups: text(m, 2),
            aon: text(m, 3),
            control_a: text(m, 4),
            control_b: text(m, 5),
            share_pad: m.get(6).map(|_| text(m, 6)),
            ext_a: m.get(7).map(|_| text(m, 7)),
        })
        .collect();

    let stages: Vec<&str> = records.iter().map(|r| r.stage.as_str()).collect();
    let expected: &[&str] = if version >= 3 {
        &["startup"]
    } else {
        &[
            "startup", "PA0", "PA0", "PA2", "PA2", "PA4", "PA4", "PB22", "PB22",
        ]
    };
    let complete = stages == expected && count(rx, b"PLIO1") == records.len();

    Checked {
        outcome: if complete {
            Outcome::Pass
        } else {
            Outcome::GpioRoutingIncomplete
        },
        fields: RoutingReport {
            gpio_routing_complete: complete,
            gpio_routing_records: records,
        },
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LedReport {
    pub(crate) led_probe_verified: bool,
    pub(crate) led_input_verified_pins: Vec<&'static str>,
    pub(crate) led_readbacks: Vec<Readback>,
    /// `low-only-v1` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) led_low_verified: Option<bool>,
    /// `bounded-probe-v3` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) led_output_only_pins: Option<&'static [&'static str]>,
    /// The LED probe's own outcome, kept when an earlier failure decided the
    /// startup's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) led_outcome: Option<Outcome>,
}

/// One PLDBG1 readback's fixed fields.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Readback {
    pub(crate) pin: String,
    pub(crate) stage: String,
    pub(crate) want_high: bool,
    pub(crate) latch_high: bool,
    pub(crate) input_high: bool,
    /// Version 2 and later: highs out of `input_samples` reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_high_samples: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_samples: Option<u64>,
    pub(crate) directions_ok: bool,
    pub(crate) stopped_ok: bool,
    pub(crate) pad_control: String,
}

/// The LED probe's records exactly as the firmware prints them for `version`.
fn expected_led(version: u8, output_only: &[&str]) -> Vec<Vec<u8>> {
    let low_only = version >= 4;
    let mut expected: Vec<Vec<u8>> = vec![if low_only {
        b"PLLED1 begin low_only\r\n".to_vec()
    } else if output_only.is_empty() {
        b"PLLED1 begin\r\n".to_vec()
    } else {
        format!("PLLED1 begin output_only={}\r\n", OUTPUT_ONLY.join(",")).into_bytes()
    }];

    for pin in LED_PINS.into_iter().filter(|_| !low_only) {
        let status = if output_only.contains(&pin) {
            "output"
        } else {
            "pass"
        };
        for level in ["1", "0", "1", "0"] {
            expected
                .push(format!("PLLED1 pin={pin} level={level} status={status}\r\n").into_bytes());
        }
    }

    expected.push(LED_DONE.to_vec());
    expected
}

fn readbacks(rx: &[u8], version: u8) -> Vec<Readback> {
    let pattern = if version >= 2 {
        &READBACK_V2
    } else {
        &READBACK_V1
    };
    all(pattern, rx)
        .iter()
        .map(|m| Readback {
            pin: text(m, 1),
            stage: text(m, 2),
            want_high: dec(m, 3) != 0,
            latch_high: dec(m, 4) != 0,
            input_high: dec(m, 5) != 0,
            input_high_samples: (version > 1).then(|| dec(m, 6)),
            input_samples: (version > 1).then_some(64),
            directions_ok: dec(m, 7) != 0,
            stopped_ok: dec(m, 8) != 0,
            pad_control: text(m, 9),
        })
        .collect()
}

/// Independent expected bounded GPIO sequence; no installer or firmware imports.
///
/// Version 3 labels PA0/PA2/PA4 output-only (no input readback); they report
/// status=output, never pass. The sequence outcome can still be 'pass', but
/// `led_probe_verified` means every pin was input-verified, so it stays false
/// whenever output-only pins are present; `led_input_verified_pins` lists the
/// pins whose levels were read back.
///
/// Version 4 (`low-only-v1`) has no probe: the indicators are only driven
/// LOW and verified, so the sequence is `begin low_only` then `done`, and no
/// pin counts as probe-verified.
pub(crate) fn classify_led(rx: &[u8], version: u8) -> Checked<LedReport> {
    let low_only = version >= 4;
    let output_only: &'static [&'static str] = if version == 3 { &OUTPUT_ONLY } else { &[] };
    let expected = expected_led(version, output_only);

    let actual: Vec<&[u8]> = LED_RECORD.find_iter(rx).map(|m| m.as_bytes()).collect();
    let prefix = expected.len().min(actual.len());
    let matches_prefix = actual.len() <= expected.len()
        && actual
            .iter()
            .zip(&expected[..prefix])
            .all(|(a, e)| *a == e.as_slice());

    let outcome = if contains(rx, b"PLLED1 error ") || LED_FAIL.is_match(rx) {
        Outcome::LedFailed
    } else if actual.is_empty() && !contains(rx, b"PLLED1") {
        Outcome::LedMissing
    } else if actual.len() != count(rx, b"PLLED1") || !matches_prefix {
        Outcome::LedUnexpectedSequence
    } else if actual.len() != expected.len() {
        Outcome::LedIncomplete
    } else {
        Outcome::Pass
    };
    let passed = outcome == Outcome::Pass;

    let verified: Vec<&str> = if passed && !low_only {
        LED_PINS
            .iter()
            .copied()
            .filter(|p| !output_only.contains(p))
            .collect()
    } else {
        vec![]
    };

    Checked {
        outcome,
        fields: LedReport {
            led_probe_verified: passed && output_only.is_empty() && !low_only,
            led_input_verified_pins: verified,
            led_readbacks: readbacks(rx, version),
            led_low_verified: low_only.then_some(passed),
            led_output_only_pins: (!output_only.is_empty()).then_some(output_only),
            led_outcome: None,
        },
    }
}

/// Whether a startup must report its stopped outputs: the build declared
/// the `stop-only` output policy and links the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopped {
    Required,
    Optional,
}

/// The start record's fixed fields.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StartRecord {
    pub(crate) bank: Option<u8>,
    pub(crate) reset_flags: String,
    pub(crate) stopped_gpio_verified: bool,
    /// Where the record starts in the capture, for the observation time; not
    /// reported.
    #[serde(skip)]
    pub(crate) start_offset: usize,
}

/// RSIP mapping entry 0 names the bank: virtual 0x0c000000 at physical
/// 0x08006000 or 0x08186000. Any other mapping is unknown.
fn live_bank(ctrl: u64, start: u64, offset: u64) -> Option<u8> {
    let physical = if ctrl & 2 != 0 {
        start.checked_sub(offset)?
    } else {
        start + offset
    };

    if ctrl & 1 == 0 || start != 0x0c000000 {
        return None;
    }
    match physical {
        0x08006000 => Some(0),
        0x08186000 => Some(1),
        _ => None,
    }
}

/// Parse only fixed public fields; never return arbitrary device text.
pub(crate) fn classify(
    rx: &[u8],
    identity: &str,
    stopped: Stopped,
) -> Checked<Option<StartRecord>> {
    let starts = all(&START, rx);
    let outputs = all(&OUTPUTS, rx);
    // The stopped-output state: motor PA13/PA14 latched HIGH/HIGH, infrared
    // PA15/PB3 LOW, and all four driven as outputs.
    let outputs_stopped = outputs.len() == 1
        && [
            hex(&outputs[0], 1),
            hex(&outputs[0], 2),
            hex(&outputs[0], 3),
            hex(&outputs[0], 4),
        ] == [0x6000, 0, 0xe000, 8]
        && group(&outputs[0], 5) == b"pass";

    let pass_count = count(rx, PASS);
    let unstarted = |outcome| Checked {
        outcome,
        fields: None,
    };
    if [&b"Hard Fault"[..], b"HardFault", b"Fault on thread"]
        .iter()
        .any(|x| contains(rx, x))
    {
        return unstarted(Outcome::Fault);
    } else if count(rx, b"PLFW1 start ") > 1
        || count(rx, b"Wellfed local firmware: Rust bringup") > 1
    {
        return unstarted(Outcome::RepeatedStartup);
    } else if (!outputs.is_empty() && !outputs_stopped) || contains(rx, b"PLFW1 error outputs=") {
        return unstarted(Outcome::OutputsFailed);
    } else if contains(rx, b"PLFW1 error ")
        || contains(rx, FAIL)
        || contains(rx, b"Synthetic core diagnostic: FAIL")
    {
        return unstarted(Outcome::DiagnosticFailed);
    } else if count(rx, b"PLFW1 ") != starts.len() + outputs.len() + pass_count || pass_count > 1 {
        return unstarted(Outcome::MalformedReport);
    }
    let [m] = &starts[..] else {
        return unstarted(Outcome::Timeout);
    };

    let bank = match group(m, 2) {
        b"0" => Some(0),
        b"1" => Some(1),
        _ => None,
    };
    let at = whole(m);
    let outcome = if text(m, 1) != identity {
        Outcome::WrongBuild
    } else if bank.is_none() || live_bank(hex(m, 4), hex(m, 5), hex(m, 6)) != bank {
        // The bank the firmware reports must agree with its live mapping.
        Outcome::UnknownMapping
    } else if stopped == Stopped::Required && !outputs_stopped {
        Outcome::OutputsMissing
    } else if outputs_stopped && whole(&outputs[0]).end() > at.start() {
        Outcome::OutputsAfterStartup
    } else if pass_count == 1
        && find(rx, PASS).is_some_and(|i| i >= at.end())
        && count(rx, b"Synthetic core diagnostic: PASS") == 1
        && count(
            rx,
            b"Synthetic diagnostic only; feeder backend stays unopened.",
        ) == 1
        && !contains(rx, b"Flash Download Start")
        && !contains(rx, b"app_main:version:")
    {
        Outcome::Pass
    } else {
        Outcome::IncompleteReport
    };

    Checked {
        outcome,
        fields: Some(StartRecord {
            bank,
            reset_flags: format!("{:08x}", hex(m, 3)),
            stopped_gpio_verified: outputs_stopped,
            start_offset: at.start(),
        }),
    }
}

/// One startup classified under every policy its manifest declares. Each
/// declared probe adds its fields; the first failure decides the outcome.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StartupReport {
    pub(crate) outcome: Outcome,
    #[serde(flatten)]
    pub(crate) start: Option<StartRecord>,
    #[serde(flatten)]
    pub(crate) clock: Option<ClockReport>,
    #[serde(flatten)]
    pub(crate) led: Option<LedReport>,
    #[serde(flatten)]
    pub(crate) routing: Option<RoutingReport>,
    #[serde(flatten)]
    pub(crate) power: Option<PowerReport>,
    #[serde(flatten)]
    pub(crate) flash: Option<FlashReport>,
}

impl StartupReport {
    fn passed(&self) -> bool {
        self.outcome == Outcome::Pass
    }

    /// A later probe's fields; its outcome only counts while nothing failed.
    fn merge<T>(&mut self, checked: Checked<T>) -> Option<T> {
        if self.passed() {
            self.outcome = checked.outcome;
        }
        Some(checked.fields)
    }
}

/// The receive-only evidence a report was computed from.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Evidence {
    pub(crate) build_id: String,
    pub(crate) ota_sha256: String,
    pub(crate) rx_sha256: String,
    pub(crate) tx_bytes: usize,
}

/// A loaded, validated startup capture and the build it claims to run.
pub(crate) struct Loaded {
    pub(crate) manifest: BuildManifest,
    pub(crate) identity: String,
    pub(crate) image: Vec<u8>,
    pub(crate) capture: Capture,
}

impl Loaded {
    pub(crate) fn evidence(&self) -> Evidence {
        Evidence {
            build_id: self.identity.clone(),
            ota_sha256: digest(&self.image),
            rx_sha256: digest(&self.capture.rx),
            tx_bytes: self.capture.tx.len(),
        }
    }
}

/// Bind the image to its source-bound manifest and validate a receive-only capture.
pub(crate) fn load(capture: &Path, manifest_path: &Path, image_path: &Path) -> Result<Loaded> {
    let manifest: BuildManifest = wellfed_formats::load(manifest_path)?;
    let identity = manifest.source_identity()?;
    require(
        manifest.startup_identity.as_deref() == Some(identity.as_str()),
        "Build identity is not source-bound",
    )?;

    let image = fs::read(image_path)?;
    require(
        manifest
            .artifact("OTA_All.bin")
            .is_some_and(|ota| ota.sha256 == digest(&image) && ota.bytes == image.len() as u64),
        "Artifact/manifest mismatch",
    )?;
    require(
        contains(&image, format!("PLFW1 start build={identity} ").as_bytes()),
        "Identity absent from artifact",
    )?;

    let capture = Capture::load(capture)?;
    capture.validate()?;
    require(
        capture.tx.is_empty() && capture.metadata["mode"] == "observe",
        "Receive-only observation required",
    )?;

    Ok(Loaded {
        manifest,
        identity,
        image,
        capture,
    })
}

/// The clock record must precede the stopped-output record it enables.
fn clock_after_outputs(rx: &[u8]) -> bool {
    let clock_end = CLOCK.find(rx).map(|m| m.end());
    let outputs_start = OUTPUTS.find(rx).map(|m| m.start());
    matches!((clock_end, outputs_start), (Some(end), Some(start)) if end > start)
}

/// The LED probe runs only after the synthetic diagnostic passed.
fn led_before_pass(rx: &[u8]) -> bool {
    let begin = find(rx, b"PLLED1 begin").unwrap_or(0);
    let pass_end = find(rx, PASS).map_or(0, |i| i + PASS.len());
    begin < pass_end
}

/// Classify one startup segment under every policy the manifest declares.
///
/// `arrival(offset)` gives host receive times for timing-bound policies;
/// `end` is the capture end time, needed for the flash read's and write's
/// clean tail.
pub(crate) fn startup(
    rx: &[u8],
    manifest: &BuildManifest,
    image: &[u8],
    identity: &str,
    arrival: Option<Arrival>,
    end: Option<Time>,
) -> Result<StartupReport> {
    let policies = Policies::bind(manifest, image)?;
    let flash = policies
        .flash
        .map(|policy| -> Result<_> {
            require(
                policies.led.is_some(),
                "Flash handshake requires the LED probe policy",
            )?;
            let arrival = arrival.ok_or_else(|| {
                Error::Refused("Flash handshake requires host receive times".into())
            })?;
            require(
                policy != FlashPolicy::Read || end.is_some(),
                "Flash read requires the capture end time",
            )?;
            require(
                policy != FlashPolicy::Write || end.is_some(),
                "Flash write requires the capture end time",
            )?;
            Ok((policy, arrival))
        })
        .transpose()?;

    let start = classify(rx, identity, policies.stopped);
    let mut report = StartupReport {
        outcome: start.outcome,
        start: start.fields,
        clock: None,
        led: None,
        routing: None,
        power: None,
        flash: None,
    };

    if policies.clock {
        let mut clock = classify_clock(rx);
        if report.passed() && clock.outcome == Outcome::Pass && clock_after_outputs(rx) {
            clock.outcome = Outcome::GpioClockOrder;
            clock.fields.gpio_clock_verified = false;
        }
        report.clock = report.merge(clock);
    }

    if let Some(version) = policies.led {
        let mut led = classify_led(rx, version);
        if report.passed() {
            if led.outcome == Outcome::Pass && led_before_pass(rx) {
                led.outcome = Outcome::LedUnexpectedSequence;
                led.fields.led_probe_verified = false;
                led.fields.led_input_verified_pins.clear();
            }
            report.led = report.merge(led);
        } else {
            report.led = Some(LedReport {
                led_probe_verified: false,
                led_input_verified_pins: vec![],
                led_readbacks: led.fields.led_readbacks,
                led_low_verified: None,
                led_output_only_pins: None,
                led_outcome: Some(led.outcome),
            });
        }
    }

    if let Some(version) = policies.routing {
        report.routing = report.merge(classify_routing(rx, version));
    }
    if let Some(version) = policies.power {
        report.power = report.merge(classify_power(rx, version));
    }
    if let Some((policy, arrival)) = flash {
        report.flash = report.merge(classify_flash(rx, arrival, policy, end)?);
    }

    Ok(report)
}

const STARTUP_LIMIT: &str =
    "UART startup evidence; electrical outputs and physical reset cause are not established.";

/// The startup audit's report.
#[derive(Debug, Clone, Serialize)]
pub struct StartupVerdict {
    #[serde(flatten)]
    pub(crate) startup: StartupReport,
    /// Only for a passing startup: how long the capture ran after it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) seconds_after_startup: Option<f64>,
    #[serde(flatten)]
    pub(crate) evidence: Evidence,
    pub(crate) minimum_seconds: f64,
    pub(crate) limit: &'static str,
}

impl StartupVerdict {
    pub fn passed(&self) -> bool {
        self.startup.passed()
    }
}

/// The startup audit of one capture, as `verify` in the Python auditor.
pub fn verify(
    capture: &Path,
    manifest: &Path,
    image: &Path,
    minimum_seconds: f64,
) -> Result<StartupVerdict> {
    let loaded = load(capture, manifest, image)?;
    let c = &loaded.capture;
    let arrival = |offset: usize| c.arrival(offset);
    let end = c.end()?;

    let mut startup = startup(
        &c.rx,
        &loaded.manifest,
        &loaded.image,
        &loaded.identity,
        Some(&arrival),
        Some(end),
    )?;

    let mut seconds_after_startup = None;
    if let (Outcome::Pass, Some(start)) = (startup.outcome, &startup.start) {
        let after = seconds(end, c.arrival(start.start_offset)?);
        if after < minimum_seconds {
            startup.outcome = Outcome::ObservationTooShort;
        }
        seconds_after_startup = Some(after);
    }

    Ok(StartupVerdict {
        startup,
        seconds_after_startup,
        evidence: loaded.evidence(),
        minimum_seconds,
        limit: STARTUP_LIMIT,
    })
}

#[cfg(test)]
mod tests;
