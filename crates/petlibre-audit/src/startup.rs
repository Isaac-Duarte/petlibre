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

use petlibre_formats::manifest::BuildManifest;
use regex::bytes::{Captures, Match, Regex};
use serde_json::{Value, json};

use crate::capture::{Capture, Time, seconds};
use crate::{Report, Result, digest, require, update, update_except_outcome};

fn re(pattern: &str) -> Regex {
    // Byte-wise ASCII semantics, as Python's bytes patterns.
    Regex::new(&format!("(?-u){pattern}")).expect("audit pattern")
}

macro_rules! pattern {
    ($name:ident, $text:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| re($text));
    };
}

pattern!(
    START,
    r"PLFW1 start build=([0-9a-f]{64}) bank=(0|1|unknown) reset=([0-9a-f]{8}) mmu=([0-9a-f]{8}),([0-9a-f]{8}),([0-9a-f]{8})\r\n"
);
pub const PASS: &[u8] = b"PLFW1 result diagnostic=pass backend=unopened\r\n";
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
const ROUTING_FIELDS: [&str; 7] = [
    "stage",
    "groups",
    "aon",
    "control_a",
    "control_b",
    "share_pad",
    "ext_a",
];

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

const AMACRO: u64 = 1 << 28;
/// PORB, MBIAS and LDO trim/power, per the stock trace.
const SHARE_PAD_HELPER: u64 = 0x3ff;

const SUPERVISION_MARK: &[u8] = b"PLTMR1 open board=";
const FLASH_MARK: &[u8] = b"PLFLS1 hello authority=";
/// FW06 stage C park round trip follows READY.
const PARK_MARK: &[u8] = b"PLPRK1 start mode=";
/// FW06 stage D mediated flash read follows READY.
const READ_MARK: &[u8] = b"PLRDS1 start marker=";

pattern!(
    READ_START,
    r"PLRDS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n"
);
pattern!(
    READ_BATCH,
    r"PLRDS1 batch reads=([0-9]{1,10}) id=([0-9a-f]{6}) mismatches=([0-9]{1,10}) park_max=([0-9]{1,10}) release_max=([0-9]{1,10}) frozen=([0-9]{1,10}) alive=([0-9]{1,10})\r\n"
);
pattern!(
    READ_DONE_RE,
    r"PLRDS1 done reads=([0-9]{1,10}) leases=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10})\r\n"
);
pattern!(READ_SKIP, r"PLRDS1 skip marker=([0-9a-f]{8})\r\n");

const READ_BATCHES: [u64; 4] = [25, 50, 75, 100];
/// JEDEC ID of the research unit's GigaDevice flash (boot log `Flash ID:c8, 40, 17`).
const READ_ID: &str = "c84017";
/// Reads, leases, phase IDLE, no fault.
const READ_DONE: [u64; 4] = [100, 100, 1, 0];
/// 40 ms park/release exchange limit at 32 768 Hz.
const EXCHANGE_TICKS: u64 = 1310;

/// FW06 stage E owner-approved scratch-sector write follows READY.
const WRITE_MARK: &[u8] = b"PLWRS1 start marker=";
/// FW15 flash journal on NOR follows READY.
const JOURNAL_MARK: &[u8] = b"PLJNL1 start mode=";
/// FW15 settings-store power-cut stress follows READY.
const SETTINGS_MARK: &[u8] = b"PLSET1 start marker=";
/// FW14 read-only power-input samples follow the LED probe.
pub const ADC_MARK: &[u8] = b"PLADC1 open outputs=";
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

const DEC: &str = "([0-9]{1,10})";
static WRITE_START: LazyLock<Regex> = LazyLock::new(|| {
    re(r"PLWRS1 start marker=([0-9a-f]{8}) watchdog=(pass|fail) installed=([01])\r\n")
});
static WRITE_PREIMAGE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 preimage id=([0-9a-f]{{6}}) blank={DEC}\r\n"
    ))
});
static WRITE_CYCLE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 cycle n={DEC} erase_ticks={DEC} blank={DEC} program_max={DEC} errors={DEC}\r\n"
    ))
});
static WRITE_RESTORE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 restore erase_ticks={DEC} blank={DEC}\r\n"
    ))
});
static WRITE_DONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"PLWRS1 done cycles={DEC} leases={DEC} phase={DEC} fault={DEC} park_max={DEC} release_max={DEC} frozen={DEC}\r\n"
    ))
});
static WRITE_SKIP: LazyLock<Regex> = LazyLock::new(|| re(r"PLWRS1 skip marker=([0-9a-f]{8})\r\n"));

const WRITE_CYCLES: [u64; 3] = [1, 2, 3];
/// Every sampled word of the 4 KiB scratch sector reads erased.
const WRITE_BLANK: u64 = 1024;
/// Cycles, leases, phase IDLE, no fault; frozen equals leases.
const WRITE_DONE: [u64; 4] = [3, 53, 1, 0];
const TICK_HZ: f64 = 32768.0;
/// 500 ms lease active budget.
const ERASE_TICKS: u64 = 16384;
/// 50 ms.
const PROGRAM_TICKS: u64 = 1638;

pattern!(
    FLASH_HELLO,
    r"PLFLS1 hello authority=([0-9a-f]{16}) count=([0-9]{1,10})\r\n"
);
pattern!(FLASH_FOREIGN, r"PLFLS1 foreign\r\n");
pattern!(
    FLASH_READY_RE,
    r"PLFLS1 ready peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) inactive=([01])\r\n"
);
pattern!(
    FLASH_PEER_RE,
    r"PLFLS1 peer peer=([0-9a-f]{16}) reply=([0-9]{1,10}) phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n"
);
pattern!(
    FLASH_DONE_RE,
    r"PLFLS1 done phase=([0-9]{1,10}) fault=([0-9]{1,10}) blocked=([01])\r\n"
);
pattern!(
    FLASH_ERROR,
    r"PLFLS1 error (?:timeout ms=[0-9]{1,10}|clock)\r\n"
);

// Reply ACCEPTED/FAILED, phase IDLE/FAULT, fault PEER_LOST; the test KM0
// republishes its session ~10 s after READY.
const FLASH_READY: [u64; 4] = [0, 1, 0, 1];
const FLASH_PEER: [u64; 4] = [5, 8, 7, 1];
const FLASH_DONE: [u64; 3] = [8, 7, 1];

pattern!(
    CLOCK,
    r"PLCLK1 function_before=([0-9a-f]{8}) clock_before=([0-9a-f]{8}) function_after=([0-9a-f]{8}) clock_after=([0-9a-f]{8}) status=(pass|fail)\r\n"
);

const OUTPUT_ONLY: [&str; 3] = ["PA0", "PA2", "PA4"];
const LED_PINS: [&str; 4] = ["PA0", "PA2", "PA4", "PB22"];
/// The KM0 printf format of the before-KM4 (v3) shared-pad record, as linked.
const POWER_V3_FORMAT: &[u8] = b"after_pad=%08x ext_a=%08x,%08x,%08x,%08x";

/// Non-overlapping occurrences, as `bytes.count`.
pub fn count(haystack: &[u8], needle: &[u8]) -> usize {
    let (mut n, mut at) = (0, 0);
    while let Some(i) = find_from(haystack, needle, at) {
        n += 1;
        at = i + needle.len().max(1);
    }
    n
}

pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
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

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// `bytes.find`: the offset, or -1.
fn find_i(haystack: &[u8], needle: &[u8]) -> i64 {
    find(haystack, needle).map_or(-1, |i| i as i64)
}

/// A start record of marker 00000000, watchdog pass, installed 1.
fn started_cleanly(start: &Captures) -> bool {
    group(start, 1) == b"00000000" && group(start, 2) == b"pass" && group(start, 3) == b"1"
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
    c.get(0).expect("whole match")
}

fn all<'h>(pattern: &Regex, rx: &'h [u8]) -> Vec<Captures<'h>> {
    pattern.captures_iter(rx).collect()
}

fn obj(value: Value) -> Report {
    match value {
        Value::Object(map) => map,
        _ => Report::new(),
    }
}

fn with_outcome(outcome: &str, report: &Report) -> Report {
    let mut out = Report::new();
    out.insert("outcome".into(), json!(outcome));
    update(&mut out, report);
    out
}

pub type Arrival<'a> = &'a dyn Fn(usize) -> Result<Time>;

pub fn classify_clock(rx: &[u8]) -> Report {
    let values = all(&CLOCK, rx);
    if values.len() != 1 || count(rx, b"PLCLK1") != 1 {
        let outcome = if values.is_empty() {
            "gpio_clock_missing"
        } else {
            "gpio_clock_repeated"
        };
        return obj(json!({"outcome": outcome, "gpio_clock_verified": false}));
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

    obj(
        json!({"outcome": if ok { "pass" } else { "gpio_clock_failed" }, "gpio_clock_verified": ok,
               "gpio_function_before": text(m, 1), "gpio_clock_before": text(m, 2),
               "gpio_function_after": text(m, 3), "gpio_clock_after": text(m, 4)}),
    )
}

/// One intact KM0 supply record, preserving unrelated bits, before any KM4 report.
///
/// Version 2 also requires the stock CODEC_Init LDO trim (0x2A or 0x68);
/// version 3 adds read-only raw Port A samples after each supply step.
pub fn classify_power(rx: &[u8], version: u8) -> Report {
    let values = all(if version >= 3 { &POWER_V3 } else { &POWER_V1 }, rx);
    if values.len() != 1 || count(rx, b"PLPWR1") != 1 {
        let outcome = if contains(rx, b"PLPWR1") {
            "shared_pad_malformed"
        } else {
            "shared_pad_missing"
        };
        return obj(json!({"outcome": outcome, "shared_pad_verified": false}));
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
        "shared_pad_failed"
    } else if km4.iter().min().is_some_and(|first| *first < end) {
        "shared_pad_order"
    } else {
        "pass"
    };

    let mut out = obj(
        json!({"outcome": outcome, "shared_pad_verified": outcome == "pass",
        "shared_pad_ldo_trim": format!("{:02x}", (pad >> 1) & 0x7f),
        "shared_pad_function_before": text(m, 1), "shared_pad_control_before": text(m, 2),
        "shared_pad_function_after": text(m, 3), "shared_pad_control_after": text(m, 4)}),
    );

    if version >= 3 {
        let samples: Vec<String> = text(m, 5).split(',').map(str::to_string).collect();
        out.insert("shared_pad_port_a_samples".into(), json!(samples));
    }
    out
}

/// Keep fixed numeric observations without inferring pin-group membership.
/// Version 3 has the v2 fields but only the startup stage, since the LED
/// probe that added the per-pin stages is gone (`low-only-v1`).
pub fn classify_routing(rx: &[u8], version: u8) -> Report {
    let pattern = if version >= 2 {
        &ROUTING_V2
    } else {
        &ROUTING_V1
    };
    let records: Vec<Value> = all(pattern, rx)
        .iter()
        .map(|m| {
            Value::Object(
                (1..m.len())
                    .map(|i| (ROUTING_FIELDS[i - 1].to_string(), json!(text(m, i))))
                    .collect(),
            )
        })
        .collect();

    let stages: Vec<&str> = records
        .iter()
        .map(|r| r["stage"].as_str().unwrap_or(""))
        .collect();

    let expected: &[&str] = if version >= 3 {
        &["startup"]
    } else {
        &[
            "startup", "PA0", "PA0", "PA2", "PA2", "PA4", "PA4", "PB22", "PB22",
        ]
    };
    let complete = stages == expected && count(rx, b"PLIO1") == records.len();

    obj(
        json!({"outcome": if complete { "pass" } else { "gpio_routing_incomplete" },
               "gpio_routing_complete": complete, "gpio_routing_records": records}),
    )
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
pub fn classify_led(rx: &[u8], version: u8) -> Report {
    let low_only = version >= 4;
    let output_only: &[&str] = if version == 3 { &OUTPUT_ONLY } else { &[] };
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
    expected.push(b"PLLED1 done low=all\r\n".to_vec());

    let actual: Vec<&[u8]> = LED_RECORD.find_iter(rx).map(|m| m.as_bytes()).collect();
    let prefix = expected.len().min(actual.len());
    let matches_prefix = actual.len() <= expected.len()
        && actual
            .iter()
            .zip(&expected[..prefix])
            .all(|(a, e)| *a == e.as_slice());

    let outcome = if contains(rx, b"PLLED1 error ") || LED_FAIL.is_match(rx) {
        "led_failed"
    } else if actual.is_empty() && !contains(rx, b"PLLED1") {
        "led_missing"
    } else if actual.len() != count(rx, b"PLLED1") || !matches_prefix {
        "led_unexpected_sequence"
    } else if actual.len() != expected.len() {
        "led_incomplete"
    } else {
        "pass"
    };

    let pattern = if version >= 2 {
        &READBACK_V2
    } else {
        &READBACK_V1
    };
    let diagnostics: Vec<Value> = all(pattern, rx)
        .iter()
        .map(|m| {
            let mut d = obj(json!({"pin": text(m, 1), "stage": text(m, 2),
                "want_high": dec(m, 3) != 0, "latch_high": dec(m, 4) != 0, "input_high": dec(m, 5) != 0}));
            if version > 1 {
                d.insert("input_high_samples".into(), json!(dec(m, 6)));
                d.insert("input_samples".into(), json!(64));
            }
            d.insert("directions_ok".into(), json!(dec(m, 7) != 0));
            d.insert("stopped_ok".into(), json!(dec(m, 8) != 0));
            d.insert("pad_control".into(), json!(text(m, 9)));
            Value::Object(d)
        })
        .collect();

    let verified: Vec<&str> = if outcome == "pass" && !low_only {
        LED_PINS
            .iter()
            .copied()
            .filter(|p| !output_only.contains(p))
            .collect()
    } else {
        vec![]
    };

    let mut out = obj(
        json!({"outcome": outcome, "led_probe_verified": outcome == "pass" && output_only.is_empty() && !low_only,
        "led_input_verified_pins": verified, "led_readbacks": diagnostics}),
    );
    if low_only {
        out.insert("led_low_verified".into(), json!(outcome == "pass"));
    }

    if !output_only.is_empty() {
        out.insert("led_output_only_pins".into(), json!(output_only));
    }
    out
}

struct Flash<'h> {
    hello: Vec<Captures<'h>>,
    foreign: Vec<Captures<'h>>,
    ready: Vec<Captures<'h>>,
    peer: Vec<Captures<'h>>,
    done: Vec<Captures<'h>>,
    error: Vec<Captures<'h>>,
}

impl Flash<'_> {
    fn total(&self) -> usize {
        self.hello.len()
            + self.foreign.len()
            + self.ready.len()
            + self.peer.len()
            + self.done.len()
            + self.error.len()
    }
}

/// FW06 HELLO/READY binding, then one injected peer change and the 30 s watch.
///
/// Version 2 (fw06-park-v1) checks only HELLO/READY: it refuses peer/done
/// records and requires the park round trip's start record after READY instead.
/// Version 3 (fw06-read-v1) checks HELLO/READY the same way, then the mediated
/// flash read records; `end` is the capture end time. Version 4
/// (fw06-write-v1) does the same for the scratch-sector write. Version 5
/// (fw15-journal-v1) checks HELLO/READY as version 2, then requires the
/// journal's start record after READY. Version 6 (fw15-settings-v1 and -v2)
/// does the same with the settings store's start record and also refuses
/// journal records.
pub fn classify_flash(
    rx: &[u8],
    arrival: Arrival,
    version: u8,
    end: Option<Time>,
) -> Result<Report> {
    let found = Flash {
        hello: all(&FLASH_HELLO, rx),
        foreign: all(&FLASH_FOREIGN, rx),
        ready: all(&FLASH_READY_RE, rx),
        peer: all(&FLASH_PEER_RE, rx),
        done: all(&FLASH_DONE_RE, rx),
        error: all(&FLASH_ERROR, rx),
    };

    let kinds = if version == 1 { 4 } else { 2 };
    let mut report = obj(
        json!({"flash_handshake_verified": false, "flash_authority_session": null,
        "flash_boot_count": null, "flash_peer_session": null, "flash_injected_peer_session": null,
        "flash_ready_seconds": null, "flash_peer_change_seconds": null, "flash_done_seconds": null}),
    );

    if version == 3 {
        update(
            &mut report,
            &obj(
                json!({"flash_read_verified": false, "flash_read_id": null, "flash_read_reads": null,
            "flash_read_leases": null, "flash_read_park_max": null, "flash_read_release_max": null,
            "flash_read_frozen": null, "flash_read_alive": null, "flash_read_seconds": null,
            "flash_read_tail_seconds": null}),
            ),
        );
    }

    if version == 4 {
        update(
            &mut report,
            &obj(
                json!({"flash_write_verified": false, "flash_write_id": null,
            "flash_write_preimage_blank": null, "flash_write_cycles": [], "flash_write_restore_erase_ticks": null,
            "flash_write_restore_blank": null, "flash_write_leases": null, "flash_write_park_max": null,
            "flash_write_release_max": null, "flash_write_frozen": null, "flash_write_erase_ms_max": null,
            "flash_write_seconds": null, "flash_write_tail_seconds": null}),
            ),
        );
    }

    let single = |list: &Vec<Captures<'_>>| list.len() == 1;
    if single(&found.hello) {
        report.insert(
            "flash_authority_session".into(),
            json!(text(&found.hello[0], 1)),
        );
        report.insert("flash_boot_count".into(), json!(dec(&found.hello[0], 2)));
    }
    if single(&found.ready) {
        report.insert("flash_peer_session".into(), json!(text(&found.ready[0], 1)));
    }
    if single(&found.peer) {
        report.insert(
            "flash_injected_peer_session".into(),
            json!(text(&found.peer[0], 1)),
        );
    }

    if !contains(rx, b"PLFLS1") {
        return Ok(with_outcome("flash_handshake_missing", &report));
    }

    if count(rx, b"PLFLS1") != found.total()
        || found.foreign.len() > 1
        || [&found.hello, &found.ready, &found.peer, &found.done]
            .iter()
            .any(|l| l.len() > 1)
    {
        return Ok(with_outcome("flash_handshake_malformed", &report));
    }

    if version >= 2 && (!found.peer.is_empty() || !found.done.is_empty()) {
        return Ok(with_outcome("flash_handshake_malformed", &report));
    }

    // Under the read and write policies an error after READY belongs to that
    // stage; earlier ones fail the handshake.
    if !found.error.is_empty()
        && (!matches!(version, 3 | 4)
            || !single(&found.ready)
            || whole(&found.error[0]).start() < whole(&found.ready[0]).end())
    {
        return Ok(with_outcome("flash_handshake_failed", &report));
    }

    let singles = [&found.hello, &found.ready, &found.peer, &found.done]
        .iter()
        .filter(|l| single(l))
        .count();
    if singles != kinds {
        return Ok(with_outcome("flash_handshake_missing", &report));
    }

    // With version >= 2 there are no peer/done records, so the singles are hello and ready.
    let (hello, ready) = (&found.hello[0], &found.ready[0]);
    let led_done = find_i(rx, b"PLLED1 done low=all\r\n");

    match version {
        3 => return classify_flash_read(rx, arrival, end, hello, ready, &found, led_done, report),
        4 => return classify_flash_write(rx, arrival, end, hello, ready, &found, led_done, report),

        5 => {
            if [&b"PLPRK1"[..], b"PLRDS1", b"PLWRS1"]
                .iter()
                .any(|m| contains(rx, m))
            {
                return Ok(with_outcome("flash_handshake_malformed", &report));
            }
            return classify_flash_park(
                rx,
                arrival,
                hello,
                ready,
                &found.foreign,
                led_done,
                report,
                JOURNAL_MARK,
            );
        }

        6 => {
            if [&b"PLPRK1"[..], b"PLRDS1", b"PLWRS1", b"PLJNL1"]
                .iter()
                .any(|m| contains(rx, m))
            {
                return Ok(with_outcome("flash_handshake_malformed", &report));
            }
            return classify_flash_park(
                rx,
                arrival,
                hello,
                ready,
                &found.foreign,
                led_done,
                report,
                SETTINGS_MARK,
            );
        }

        2 => {
            return classify_flash_park(
                rx,
                arrival,
                hello,
                ready,
                &found.foreign,
                led_done,
                report,
                PARK_MARK,
            );
        }

        _ => {}
    }

    let (peer, done) = (&found.peer[0], &found.done[0]);
    let (h, r, p, d) = (whole(hello), whole(ready), whole(peer), whole(done));
    let ordered = 0 <= led_done
        && (led_done as usize) < h.start()
        && h.start() < h.end()
        && h.end() <= r.start()
        && r.start() < r.end()
        && r.end() <= p.start()
        && p.start() < p.end()
        && p.end() <= d.start();

    if !ordered || found.foreign.iter().any(|m| whole(m).start() < h.end()) {
        return Ok(with_outcome("flash_handshake_order", &report));
    }

    let (first, second) = (hex(ready, 1), hex(peer, 1));
    let ok = ready_fields_ok(hello, ready)
        && second != first
        && second >> 32 == (first >> 32) + 1
        && decs(peer, 2) == FLASH_PEER
        && decs(done, 1) == FLASH_DONE;
    if !ok {
        return Ok(with_outcome("flash_handshake_failed", &report));
    }

    let at = |m: &Match| arrival(m.end() - 1);
    let (th, tr, tp, td) = (at(&h)?, at(&r)?, at(&p)?, at(&d)?);
    let (ready_s, peer_s, done_s) = (seconds(tr, th), seconds(tp, tr), seconds(td, tr));
    report.insert("flash_ready_seconds".into(), json!(ready_s));
    report.insert("flash_peer_change_seconds".into(), json!(peer_s));
    report.insert("flash_done_seconds".into(), json!(done_s));

    if !((0.0..=2.5).contains(&ready_s)
        && (9.0..=12.0).contains(&peer_s)
        && (29.0..=32.0).contains(&done_s))
    {
        return Ok(with_outcome("flash_handshake_timing", &report));
    }

    let mut out = with_outcome("pass", &report);
    out.insert("flash_handshake_verified".into(), json!(true));
    Ok(out)
}

/// HELLO authority carries the boot count; READY binds a distinct, nonzero
/// peer session while stopped.
fn ready_fields_ok(hello: &Captures, ready: &Captures) -> bool {
    let (authority, count, peer) = (hex(hello, 1), dec(hello, 2), hex(ready, 1));
    authority != 0
        && authority >> 32 == count
        && count >= 1
        && peer != 0
        && peer != authority
        && peer >> 32 >= 1
        && decs(ready, 2) == FLASH_READY
}

fn hello_ready_ok(
    arrival: Arrival,
    hello: &Captures,
    ready: &Captures,
    foreign: &[Captures],
    led_done: i64,
    report: &mut Report,
    until: Option<usize>,
) -> Result<Option<&'static str>> {
    let (h, r) = (whole(hello), whole(ready));
    let ordered = 0 <= led_done
        && (led_done as usize) < h.start()
        && h.start() < h.end()
        && h.end() <= r.start()
        && until.is_none_or(|start| r.start() < r.end() && r.end() <= start);

    if !ordered || foreign.iter().any(|m| whole(m).start() < h.end()) {
        return Ok(Some("flash_handshake_order"));
    }

    if !ready_fields_ok(hello, ready) {
        return Ok(Some("flash_handshake_failed"));
    }

    let ready_s = seconds(arrival(r.end() - 1)?, arrival(h.end() - 1)?);
    report.insert("flash_ready_seconds".into(), json!(ready_s));
    if !(0.0..=2.5).contains(&ready_s) {
        return Ok(Some("flash_handshake_timing"));
    }
    Ok(None)
}

/// fw06-park-v1: HELLO/READY fields and timing, then the first park start
/// record after READY. fw15-journal-v1 and fw15-settings-v1 apply the same
/// checks with their start record as `mark`.
#[allow(clippy::too_many_arguments)]
fn classify_flash_park(
    rx: &[u8],
    arrival: Arrival,
    hello: &Captures,
    ready: &Captures,
    foreign: &[Captures],
    led_done: i64,
    mut report: Report,
    mark: &[u8],
) -> Result<Report> {
    let Some(start) = find(rx, mark) else {
        return Ok(with_outcome("flash_handshake_missing", &report));
    };

    if let Some(outcome) = hello_ready_ok(
        arrival,
        hello,
        ready,
        foreign,
        led_done,
        &mut report,
        Some(start),
    )? {
        return Ok(with_outcome(outcome, &report));
    }

    let mut out = with_outcome("pass", &report);
    out.insert("flash_handshake_verified".into(), json!(true));
    Ok(out)
}

/// fw06-read-v1: HELLO/READY as in park-v1, then one start, four batches
/// and done of the mediated read.
#[allow(clippy::too_many_arguments)]
fn classify_flash_read(
    rx: &[u8],
    arrival: Arrival,
    end: Option<Time>,
    hello: &Captures,
    ready: &Captures,
    flash: &Flash,
    led_done: i64,
    mut report: Report,
) -> Result<Report> {
    let (start, batches, done, skip) = (
        all(&READ_START, rx),
        all(&READ_BATCH, rx),
        all(&READ_DONE_RE, rx),
        all(&READ_SKIP, rx),
    );

    if let Some(last) = batches.last() {
        let mut ids: Vec<String> = batches.iter().map(|m| text(m, 2)).collect();
        ids.sort();
        ids.dedup();
        report.insert(
            "flash_read_id".into(),
            if ids.len() == 1 {
                json!(ids[0])
            } else {
                Value::Null
            },
        );
        report.insert(
            "flash_read_park_max".into(),
            json!(batches.iter().map(|m| dec(m, 4)).max()),
        );
        report.insert(
            "flash_read_release_max".into(),
            json!(batches.iter().map(|m| dec(m, 5)).max()),
        );
        report.insert("flash_read_frozen".into(), json!(dec(last, 6)));
        report.insert("flash_read_alive".into(), json!(dec(last, 7)));
    }

    if done.len() == 1 {
        report.insert("flash_read_reads".into(), json!(dec(&done[0], 1)));
        report.insert("flash_read_leases".into(), json!(dec(&done[0], 2)));
    }

    let reads: Vec<u64> = batches.iter().map(|m| dec(m, 1)).collect();
    let mut distinct = reads.clone();
    distinct.sort();
    distinct.dedup();

    if count(rx, b"PLRDS1") != start.len() + batches.len() + done.len() + skip.len()
        || contains(rx, b"PLPRK1")
        || [&start, &done, &skip].iter().any(|l| l.len() > 1)
        || distinct.len() != reads.len()
    {
        return Ok(with_outcome("flash_read_malformed", &report));
    }

    if !skip.is_empty() || !flash.error.is_empty() {
        return Ok(with_outcome("flash_read_failed", &report));
    }

    if let Some(outcome) = hello_ready_ok(
        arrival,
        hello,
        ready,
        &flash.foreign,
        led_done,
        &mut report,
        None,
    )? {
        return Ok(with_outcome(outcome, &report));
    }

    if start.is_empty() || done.is_empty() {
        return Ok(with_outcome("flash_read_missing", &report));
    }

    let (s, d, r) = (whole(&start[0]), whole(&done[0]), whole(ready));
    // Batches lie between start and done in ascending order; no PLFLS1 record follows done.
    let mut sorted = reads.clone();
    sorted.sort();
    if !(r.end() <= s.start() && s.start() < s.end() && s.end() <= d.start())
        || reads != sorted
        || batches
            .iter()
            .any(|m| !(s.end() <= whole(m).start() && whole(m).start() < d.start()))
        || flash.foreign.iter().any(|m| whole(m).start() > d.start())
    {
        return Ok(with_outcome("flash_read_order", &report));
    }

    let ok = started_cleanly(&start[0])
        && reads == READ_BATCHES
        && batches.iter().all(|m| {
            text(m, 2) == READ_ID
                && dec(m, 3) == 0
                && dec(m, 6) == dec(m, 1)
                && dec(m, 7) + 1 >= dec(m, 1)
                && 0 < dec(m, 4)
                && dec(m, 4) <= EXCHANGE_TICKS
                && 0 < dec(m, 5)
                && dec(m, 5) <= EXCHANGE_TICKS
        })
        && decs(&done[0], 1) == READ_DONE;
    if !ok {
        return Ok(with_outcome("flash_read_failed", &report));
    }

    let finished = arrival(d.end() - 1)?;
    let read_s = seconds(finished, arrival(s.end() - 1)?);
    let end = end
        .ok_or_else(|| crate::Error::Refused("Flash read requires the capture end time".into()))?;
    let tail_s = seconds(end, finished);
    report.insert("flash_read_seconds".into(), json!(read_s));
    report.insert("flash_read_tail_seconds".into(), json!(tail_s));
    if !((0.0..=30.0).contains(&read_s) && tail_s >= 30.0) {
        return Ok(with_outcome("flash_read_timing", &report));
    }

    let mut out = with_outcome("pass", &report);
    out.insert("flash_handshake_verified".into(), json!(true));
    out.insert("flash_read_verified".into(), json!(true));
    Ok(out)
}

/// fw06-write-v1: HELLO/READY as in park-v1, then start, preimage, three
/// erase/program/verify cycles, restore and done.
#[allow(clippy::too_many_arguments)]
fn classify_flash_write(
    rx: &[u8],
    arrival: Arrival,
    end: Option<Time>,
    hello: &Captures,
    ready: &Captures,
    flash: &Flash,
    led_done: i64,
    mut report: Report,
) -> Result<Report> {
    let start = all(&WRITE_START, rx);
    let preimage = all(&WRITE_PREIMAGE, rx);
    let cycles = all(&WRITE_CYCLE, rx);
    let restore = all(&WRITE_RESTORE, rx);
    let done = all(&WRITE_DONE_RE, rx);
    let skip = all(&WRITE_SKIP, rx);

    let cycle_fields: Vec<Vec<u64>> = cycles.iter().map(|m| decs(m, 1)).collect();
    report.insert(
        "flash_write_cycles".into(),
        json!(
            cycle_fields
                .iter()
                .map(|c| json!({"n": c[0], "erase_ticks": c[1], "blank": c[2],
            "program_max": c[3], "errors": c[4]}))
                .collect::<Vec<_>>()
        ),
    );

    if preimage.len() == 1 {
        report.insert("flash_write_id".into(), json!(text(&preimage[0], 1)));
        report.insert(
            "flash_write_preimage_blank".into(),
            json!(dec(&preimage[0], 2)),
        );
    }
    if restore.len() == 1 {
        report.insert(
            "flash_write_restore_erase_ticks".into(),
            json!(dec(&restore[0], 1)),
        );
        report.insert(
            "flash_write_restore_blank".into(),
            json!(dec(&restore[0], 2)),
        );
    }
    if done.len() == 1 {
        let fields = decs(&done[0], 1);
        report.insert("flash_write_leases".into(), json!(fields[1]));
        report.insert("flash_write_park_max".into(), json!(fields[4]));
        report.insert("flash_write_release_max".into(), json!(fields[5]));
        report.insert("flash_write_frozen".into(), json!(fields[6]));
    }

    let mut erase: Vec<u64> = cycle_fields.iter().map(|c| c[1]).collect();
    if restore.len() == 1 {
        erase.push(dec(&restore[0], 1));
    }
    if let Some(max) = erase.iter().max() {
        report.insert(
            "flash_write_erase_ms_max".into(),
            json!(crate::round(*max as f64 * 1000.0 / TICK_HZ, 1)),
        );
    }

    let numbers: Vec<u64> = cycle_fields.iter().map(|c| c[0]).collect();
    let mut distinct = numbers.clone();
    distinct.sort();
    distinct.dedup();
    let records =
        start.len() + preimage.len() + cycles.len() + restore.len() + done.len() + skip.len();
    if count(rx, b"PLWRS1") != records
        || contains(rx, b"PLPRK1")
        || contains(rx, b"PLRDS1")
        || [&start, &preimage, &restore, &done, &skip]
            .iter()
            .any(|l| l.len() > 1)
        || distinct.len() != numbers.len()
    {
        return Ok(with_outcome("flash_write_malformed", &report));
    }

    if !skip.is_empty() || !flash.error.is_empty() {
        return Ok(with_outcome("flash_write_failed", &report));
    }

    if let Some(outcome) = hello_ready_ok(
        arrival,
        hello,
        ready,
        &flash.foreign,
        led_done,
        &mut report,
        None,
    )? {
        return Ok(with_outcome(outcome, &report));
    }

    if [&start, &preimage, &restore, &done]
        .iter()
        .any(|l| l.len() != 1)
    {
        return Ok(with_outcome("flash_write_missing", &report));
    }

    let (s, p, rs, d, r) = (
        whole(&start[0]),
        whole(&preimage[0]),
        whole(&restore[0]),
        whole(&done[0]),
        whole(ready),
    );

    let mut sorted = numbers.clone();
    sorted.sort();
    // start, preimage, ascending cycles, restore, done; no PLFLS1 record follows done.
    if !(r.end() <= s.start()
        && s.start() < s.end()
        && s.end() <= p.start()
        && p.start() < p.end()
        && p.end() <= rs.start()
        && rs.start() < rs.end()
        && rs.end() <= d.start())
        || numbers != sorted
        || cycles
            .iter()
            .any(|m| !(p.end() <= whole(m).start() && whole(m).start() < rs.start()))
        || flash.foreign.iter().any(|m| whole(m).start() > d.start())
    {
        return Ok(with_outcome("flash_write_order", &report));
    }

    let done_fields = decs(&done[0], 1);
    let ok = started_cleanly(&start[0])
        && text(&preimage[0], 1) == READ_ID
        && dec(&preimage[0], 2) == WRITE_BLANK
        && numbers == WRITE_CYCLES
        && cycle_fields.iter().all(|c| {
            c[2] == WRITE_BLANK
                && c[4] == 0
                && 0 < c[1]
                && c[1] <= ERASE_TICKS
                && 0 < c[3]
                && c[3] <= PROGRAM_TICKS
        })
        && dec(&restore[0], 2) == WRITE_BLANK
        && 0 < dec(&restore[0], 1)
        && dec(&restore[0], 1) <= ERASE_TICKS
        && done_fields[..4] == WRITE_DONE
        && done_fields[6] == WRITE_DONE[1]
        && 0 < done_fields[4]
        && done_fields[4] <= EXCHANGE_TICKS
        && 0 < done_fields[5]
        && done_fields[5] <= EXCHANGE_TICKS;
    if !ok {
        return Ok(with_outcome("flash_write_failed", &report));
    }

    let finished = arrival(d.end() - 1)?;
    let write_s = seconds(finished, arrival(s.end() - 1)?);
    let end = end
        .ok_or_else(|| crate::Error::Refused("Flash write requires the capture end time".into()))?;
    let tail_s = seconds(end, finished);
    report.insert("flash_write_seconds".into(), json!(write_s));
    report.insert("flash_write_tail_seconds".into(), json!(tail_s));
    if !((0.0..=60.0).contains(&write_s) && tail_s >= 30.0) {
        return Ok(with_outcome("flash_write_timing", &report));
    }

    let mut out = with_outcome("pass", &report);
    out.insert("flash_handshake_verified".into(), json!(true));
    out.insert("flash_write_verified".into(), json!(true));
    Ok(out)
}

/// Parse only fixed public fields; never return arbitrary device text.
pub fn classify(rx: &[u8], identity: &str, require_stopped: bool) -> Report {
    let starts = all(&START, rx);
    let outputs = all(&OUTPUTS, rx);
    // The stopped-output state: motor PA13/PA14 latched HIGH/HIGH, infrared
    // PA15/PB3 LOW, and all four driven as outputs.
    let stopped = outputs.len() == 1
        && [
            hex(&outputs[0], 1),
            hex(&outputs[0], 2),
            hex(&outputs[0], 3),
            hex(&outputs[0], 4),
        ] == [0x6000, 0, 0xe000, 8]
        && group(&outputs[0], 5) == b"pass";

    let pass_count = count(rx, PASS);
    let mut outcome = "timeout";
    if [&b"Hard Fault"[..], b"HardFault", b"Fault on thread"]
        .iter()
        .any(|x| contains(rx, x))
    {
        outcome = "fault";
    } else if count(rx, b"PLFW1 start ") > 1
        || count(rx, b"Petlibre local firmware: Rust bringup") > 1
    {
        outcome = "repeated_startup";
    } else if (!outputs.is_empty() && !stopped) || contains(rx, b"PLFW1 error outputs=") {
        outcome = "outputs_failed";
    } else if contains(rx, b"PLFW1 error ")
        || contains(rx, FAIL)
        || contains(rx, b"Synthetic core diagnostic: FAIL")
    {
        outcome = "diagnostic_failed";
    } else if count(rx, b"PLFW1 ") != starts.len() + outputs.len() + pass_count || pass_count > 1 {
        outcome = "malformed_report";
    } else if starts.len() == 1 {
        let m = &starts[0];
        let bank = match group(m, 2) {
            b"unknown" => None,
            value => String::from_utf8_lossy(value).parse::<i64>().ok(),
        };

        let (reason, ctrl, start, offset) = (
            hex(m, 3) as i64,
            hex(m, 4) as i64,
            hex(m, 5) as i64,
            hex(m, 6) as i64,
        );

        let physical = if ctrl & 2 != 0 {
            start - offset
        } else {
            start + offset
        };

        // RSIP mapping entry 0 names the bank: virtual 0x0c000000 at physical
        // 0x08006000 or 0x08186000. Any other mapping is unknown, and the bank the
        // firmware reports must agree with it.
        let live_bank =
            if ctrl & 1 != 0 && start == 0x0c000000 && (0..=0xffffffff).contains(&physical) {
                match physical {
                    0x08006000 => Some(0),
                    0x08186000 => Some(1),
                    _ => None,
                }
            } else {
                None
            };

        let at = whole(m);
        outcome = if text(m, 1) != identity {
            "wrong_build"
        } else if bank.is_none() || live_bank != bank {
            "unknown_mapping"
        } else if require_stopped && !stopped {
            "outputs_missing"
        } else if stopped && whole(&outputs[0]).end() > at.start() {
            "outputs_after_startup"
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
            "pass"
        } else {
            "incomplete_report"
        };

        return obj(
            json!({"outcome": outcome, "bank": bank, "reset_flags": format!("{reason:08x}"),
                          "start_offset": at.start(), "stopped_gpio_verified": stopped}),
        );
    }

    obj(json!({"outcome": outcome}))
}

/// A loaded, validated startup capture and the build it claims to run.
pub struct Loaded {
    pub manifest: BuildManifest,
    pub identity: String,
    pub image: Vec<u8>,
    pub capture: Capture,
}

/// Bind the image to its source-bound manifest and validate a receive-only capture.
pub fn load(capture: &Path, manifest_path: &Path, image_path: &Path) -> Result<Loaded> {
    let manifest: BuildManifest = petlibre_formats::load(manifest_path)?;
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

/// A manifest policy: absent (None) or one of `allowed`.
fn policy<'m>(
    declared: Option<&'m str>,
    allowed: &[&str],
    message: &str,
) -> Result<Option<&'m str>> {
    match declared {
        None => Ok(None),
        Some(value) if allowed.contains(&value) => Ok(Some(value)),
        Some(_) => Err(crate::Error::Refused(message.into())),
    }
}

/// Classify one startup segment under every policy the manifest declares.
///
/// `arrival(offset)` gives host receive times for timing-bound policies;
/// `end` is the capture end time, needed for the flash read's and write's
/// clean tail.
pub fn startup(
    rx: &[u8],
    manifest: &BuildManifest,
    image: &[u8],
    identity: &str,
    arrival: Option<Arrival>,
    end: Option<Time>,
) -> Result<Report> {
    let has = |mark: &[u8]| contains(image, mark);

    // No current build declares the FW05 probe, so its records mean the image
    // is not the build the manifest describes.
    require(
        !has(SUPERVISION_MARK),
        "Supervision policy/artifact mismatch",
    )?;

    let output = policy(
        manifest.startup_output_policy.as_deref(),
        &["stop-only"],
        "Unknown startup output policy",
    )?;
    require(
        output.is_some() == has(b"PLFW1 outputs pa="),
        "Output policy/artifact mismatch",
    )?;

    let mut result = classify(rx, identity, output.is_some());
    // The first failure decides the outcome: once `result` has failed, later
    // probes add their fields but not their outcome.
    let passed = |r: &Report| r["outcome"] == "pass";

    let clock_policy = policy(
        manifest.startup_gpio_clock_policy.as_deref(),
        &["explicit-enable-v1"],
        "Unknown GPIO clock policy",
    )?;
    require(
        clock_policy.is_none() || output.is_some(),
        "Clock profile requires stopped-output reporting",
    )?;
    require(
        clock_policy.is_some() == has(b"PLCLK1 function_before="),
        "GPIO clock policy/artifact mismatch",
    )?;

    if clock_policy.is_some() {
        let mut clock = classify_clock(rx);
        if passed(&result) {
            if passed(&clock) {
                let clock_end = CLOCK.find(rx).map(|m| m.end());
                let outputs_start = OUTPUTS.find(rx).map(|m| m.start());
                if let (Some(clock_end), Some(outputs_start)) = (clock_end, outputs_start)
                    && clock_end > outputs_start
                {
                    clock.insert("outcome".into(), json!("gpio_clock_order"));
                    clock.insert("gpio_clock_verified".into(), json!(false));
                }
            }
            update(&mut result, &clock);
        } else {
            update_except_outcome(&mut result, &clock);
        }
    }

    let led_policy = policy(
        manifest.startup_led_policy.as_deref(),
        &[
            "bounded-probe-v1",
            "bounded-probe-v2",
            "bounded-probe-v3",
            "low-only-v1",
        ],
        "Unknown startup LED policy",
    )?;
    require(
        led_policy.is_some() == has(b"PLLED1 begin"),
        "LED policy/artifact mismatch",
    )?;
    require(
        matches!(
            led_policy,
            Some("bounded-probe-v2" | "bounded-probe-v3" | "low-only-v1")
        ) == has(b" highs="),
        "LED readback format/artifact mismatch",
    )?;
    require(
        (led_policy == Some("bounded-probe-v3")) == has(b"PLLED1 begin output_only="),
        "LED output-only policy/artifact mismatch",
    )?;

    if let Some(led) = led_policy {
        let version = match led {
            "bounded-probe-v2" => 2,
            "bounded-probe-v3" => 3,
            "low-only-v1" => 4,
            _ => 1,
        };

        let mut led_result = classify_led(rx, version);
        if passed(&result) {
            if passed(&led_result) {
                let begin = find(rx, b"PLLED1 begin").unwrap_or(0);
                let pass_end = find(rx, PASS).map_or(0, |i| i + PASS.len());
                if begin < pass_end {
                    led_result.insert("outcome".into(), json!("led_unexpected_sequence"));
                    led_result.insert("led_probe_verified".into(), json!(false));
                    led_result.insert("led_input_verified_pins".into(), json!([]));
                }
            }
            update(&mut result, &led_result);
        } else {
            result.insert("led_readbacks".into(), led_result["led_readbacks"].clone());
            result.insert("led_probe_verified".into(), json!(false));
            result.insert("led_input_verified_pins".into(), json!([]));
            result.insert("led_outcome".into(), led_result["outcome"].clone());
        }
    }

    let routing_policy = policy(
        manifest.startup_gpio_routing_policy.as_deref(),
        &["read-only-v1", "read-only-v2", "read-only-v3"],
        "Unknown GPIO routing policy",
    )?;
    require(
        routing_policy.is_some() == has(b"PLIO1 stage="),
        "GPIO routing policy/artifact mismatch",
    )?;
    require(
        matches!(routing_policy, Some("read-only-v2" | "read-only-v3")) == has(b" share_pad="),
        "GPIO routing format/artifact mismatch",
    )?;

    if let Some(routing_policy) = routing_policy {
        let routing = classify_routing(
            rx,
            match routing_policy {
                "read-only-v3" => 3,
                "read-only-v2" => 2,
                _ => 1,
            },
        );
        if passed(&result) {
            update(&mut result, &routing);
        } else {
            update_except_outcome(&mut result, &routing);
        }
    }

    let power_policy = policy(
        manifest.startup_shared_pad_policy.as_deref(),
        &["before-km4-v1", "before-km4-v2", "before-km4-v3"],
        "Unknown shared-pad policy",
    )?;
    // Review 06 and earlier print PLPWR1 after KM4 boot without declaring this policy.
    require(
        power_policy.is_none() || has(b"PLPWR1 before_func="),
        "Shared-pad policy/artifact mismatch",
    )?;
    // Checked both ways: a before-KM4 v3 image cannot be audited without its policy.
    require(
        (power_policy == Some("before-km4-v3"))
            == (has(b"PLPWR1 before_func=") && has(POWER_V3_FORMAT)),
        "Shared-pad v3 policy/artifact mismatch",
    )?;

    if let Some(power_policy) = power_policy {
        let version = match power_policy {
            "before-km4-v2" => 2,
            "before-km4-v3" => 3,
            _ => 1,
        };
        let power = classify_power(rx, version);
        if passed(&result) {
            update(&mut result, &power);
        } else {
            update_except_outcome(&mut result, &power);
        }
    }

    // The probes below are only bound to the image here; this audit does not
    // classify their records.
    let wifi = policy(
        manifest.startup_wifi_policy.as_deref(),
        &["fw07-scan-v1", "fw07-join-v1", "fw07-dns-v1"],
        "Unknown Wi-Fi policy",
    )?;
    require(
        (wifi == Some("fw07-scan-v1")) == has(WIFI_MARK),
        "Wi-Fi policy/artifact mismatch",
    )?;
    require(
        (wifi == Some("fw07-join-v1")) == has(WIFI_JOIN_MARK),
        "Wi-Fi join policy/artifact mismatch",
    )?;
    require(
        (wifi == Some("fw07-dns-v1")) == has(WIFI_DNS_MARK),
        "Wi-Fi DNS policy/artifact mismatch",
    )?;
    require(
        wifi.is_none() || led_policy.is_some(),
        "Wi-Fi probe requires the LED probe policy",
    )?;

    let transport = policy(
        manifest.startup_transport_policy.as_deref(),
        &["fw08-transport-v1"],
        "Unknown transport policy",
    )?;
    require(
        transport.is_some() == has(TRANSPORT_MARK),
        "Transport policy/artifact mismatch",
    )?;
    require(
        transport.is_none() || wifi == Some("fw07-dns-v1"),
        "Transport requires the FW07 DNS policy",
    )?;

    let optics = policy(
        manifest.startup_optics_policy.as_deref(),
        &["fw13-optics-v1", "fw13-optics-v2"],
        "Unknown optics policy",
    )?;
    require(
        optics.is_some() == has(OPTICS_MARK),
        "Optics policy/artifact mismatch",
    )?;
    require(
        (optics == Some("fw13-optics-v2")) == has(&[OPTICS_MARK, b"1", OPTICS_PLAN2_MARK].concat()),
        "Optics plan/artifact mismatch",
    )?;
    require(
        optics.is_none() || led_policy.is_some(),
        "Optics probe requires the LED probe policy",
    )?;

    let motion = policy(
        manifest.startup_motion_policy.as_deref(),
        &["fw16-pulse-v1"],
        "Unknown motion policy",
    )?;
    require(
        motion.is_some() == has(MOTION_MARK),
        "Motion policy/artifact mismatch",
    )?;
    require(
        !(motion.is_some() && optics.is_some()),
        "Motion and optics probes are exclusive",
    )?;
    require(
        motion.is_none() || led_policy.is_some(),
        "Motion probe requires the LED probe policy",
    )?;

    let trial = policy(
        manifest.startup_trial_policy.as_deref(),
        &["fw11-reset-v1"],
        "Unknown trial policy",
    )?;
    require(
        trial.is_some() == has(TRIAL_RESET_MARK),
        "Trial policy/artifact mismatch",
    )?;
    require(
        !(trial.is_some() && (optics.is_some() || motion.is_some())),
        "The trial test runs alone",
    )?;

    let adc = policy(
        manifest.startup_adc_policy.as_deref(),
        &["fw14-adc-v1", "fw14-adc-v2"],
        "Unknown ADC policy",
    )?;
    require(
        adc.is_some() == has(ADC_MARK),
        "ADC policy/artifact mismatch",
    )?;
    require(
        (adc == Some("fw14-adc-v2")) == has(ADC_SUPPLY_MARK),
        "ADC supply policy/artifact mismatch",
    )?;
    require(
        adc.is_none() || led_policy.is_some(),
        "ADC probe requires the LED probe policy",
    )?;

    let flash_policy = policy(
        manifest.startup_flash_handshake_policy.as_deref(),
        &[
            "fw06-hello-v1",
            "fw06-park-v1",
            "fw06-read-v1",
            "fw06-write-v1",
            "fw15-journal-v1",
            "fw15-settings-v1",
            "fw15-settings-v2",
        ],
        "Unknown flash handshake policy",
    )?;
    require(
        flash_policy.is_some() == has(FLASH_MARK),
        "Flash handshake policy/artifact mismatch",
    )?;

    // Checked both ways: a park image cannot be audited as a hello-only build, nor the reverse.
    require(
        (flash_policy == Some("fw06-park-v1")) == has(PARK_MARK),
        "Flash park policy/artifact mismatch",
    )?;
    require(
        (flash_policy == Some("fw06-read-v1")) == has(READ_MARK),
        "Flash read policy/artifact mismatch",
    )?;
    require(
        (flash_policy == Some("fw06-write-v1")) == has(WRITE_MARK),
        "Flash write policy/artifact mismatch",
    )?;
    require(
        (flash_policy == Some("fw15-journal-v1")) == has(JOURNAL_MARK),
        "Flash journal policy/artifact mismatch",
    )?;
    require(
        matches!(flash_policy, Some("fw15-settings-v1" | "fw15-settings-v2")) == has(SETTINGS_MARK),
        "Flash settings policy/artifact mismatch",
    )?;

    if let Some(flash_policy) = flash_policy {
        require(
            led_policy.is_some(),
            "Flash handshake requires the LED probe policy",
        )?;

        let arrival = arrival.ok_or_else(|| {
            crate::Error::Refused("Flash handshake requires host receive times".into())
        })?;
        require(
            flash_policy != "fw06-read-v1" || end.is_some(),
            "Flash read requires the capture end time",
        )?;
        require(
            flash_policy != "fw06-write-v1" || end.is_some(),
            "Flash write requires the capture end time",
        )?;

        let version = match flash_policy {
            "fw06-park-v1" => 2,
            "fw06-read-v1" => 3,
            "fw06-write-v1" => 4,
            "fw15-journal-v1" => 5,
            "fw15-settings-v1" | "fw15-settings-v2" => 6,
            _ => 1,
        };

        let flash = classify_flash(rx, arrival, version, end)?;
        if passed(&result) {
            update(&mut result, &flash);
        } else {
            update_except_outcome(&mut result, &flash);
        }
    }

    Ok(result)
}

/// The startup audit of one capture, as `verify` in the Python auditor.
pub fn verify(
    capture: &Path,
    manifest: &Path,
    image: &Path,
    minimum_seconds: f64,
) -> Result<Report> {
    let loaded = load(capture, manifest, image)?;
    let c = &loaded.capture;
    let arrival = |offset: usize| c.arrival(offset);
    let end = crate::capture::timestamp(&c.metadata["end_utc"])?;

    let mut result = startup(
        &c.rx,
        &loaded.manifest,
        &loaded.image,
        &loaded.identity,
        Some(&arrival),
        Some(end),
    )?;

    if result["outcome"] == "pass" {
        let at = result
            .remove("start_offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let seconds_after = seconds(end, c.arrival(at)?);
        result.insert("seconds_after_startup".into(), json!(seconds_after));
        if seconds_after < minimum_seconds {
            result.insert("outcome".into(), json!("observation_too_short"));
        }
    }

    result.remove("start_offset");
    update(
        &mut result,
        &obj(
            json!({"build_id": loaded.identity, "ota_sha256": digest(&loaded.image),
        "rx_sha256": digest(&c.rx), "tx_bytes": c.tx.len(), "minimum_seconds": minimum_seconds,
        "limit": "UART startup evidence; electrical outputs and physical reset cause are not established."}),
        ),
    );

    Ok(result)
}

#[cfg(test)]
mod tests;
