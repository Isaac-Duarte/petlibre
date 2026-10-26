//! The startup auditor's checks: classifiers, policy binding both ways, and
//! evidence binding. The flash-coordination records are tested in
//! `flash/tests.rs`, which shares this module's boot and record lines.

use serde_json::{Value, json};

use super::*;
use crate::Error;
use crate::test_support::*;

// The classifiers as the JSON they report, so cases assert on report fields.

fn classify(rx: &[u8], identity: &str, stopped: Stopped) -> Value {
    report(super::classify(rx, identity, stopped))
}

fn classify_clock(rx: &[u8]) -> Value {
    report(super::classify_clock(rx))
}

fn classify_led(rx: &[u8], version: u8) -> Value {
    report(super::classify_led(rx, version))
}

fn classify_power(rx: &[u8], version: u8) -> Value {
    report(super::classify_power(rx, version))
}

fn classify_routing(rx: &[u8], version: u8) -> Value {
    report(super::classify_routing(rx, version))
}

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn start(identity: &str) -> Vec<u8> {
    format!("PLFW1 start build={identity} bank=1 reset=00100000 mmu=00000003,0c000000,03e7a000\r\n")
        .into_bytes()
}

pub(super) fn boot(identity: &str) -> Vec<u8> {
    [
        &b"Wellfed local firmware: Rust bringup\r\n"[..],
        &start(identity),
        b"Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n",
        PASS,
    ]
    .concat()
}

pub(super) const STAGES: [&str; 9] = [
    "startup", "PA0", "PA0", "PA2", "PA2", "PA4", "PA4", "PB22", "PB22",
];

fn routing_v1() -> Vec<Vec<u8>> {
    STAGES
        .iter()
        .map(|s| format!("PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000\r\n").into_bytes())
        .collect()
}

#[test]
fn routing_observations_are_fixed_fields_and_complete_sequence() {
    let records = routing_v1();
    let full = records.concat();
    let report = classify_routing(&full, 1);
    assert_eq!(report["gpio_routing_complete"], true);
    assert_eq!(report["gpio_routing_records"].as_array().unwrap().len(), 9);
    assert_eq!(report["gpio_routing_records"][0]["groups"], "0000ffff");

    let last = records.last().unwrap();
    for bad in [
        full[..full.len() - last.len()].to_vec(),
        [&full[..], last].concat(),
        replace(&full, b"stage=PA0", b"stage=PB22"),
        replace(&full, b"aon=00020003", b"aon=untrusted"),
    ] {
        assert_eq!(classify_routing(&bad, 1)["gpio_routing_complete"], false);
    }

    let untrusted = classify_routing(&replace(&full, b"aon=00020003", b"aon=untrusted"), 1);
    assert!(!json!(untrusted).to_string().contains("untrusted"));
}

#[test]
fn routing_v2_adds_supply_and_raw_port_fields() {
    let records: Vec<u8> = STAGES
        .iter()
        .flat_map(|s| {
            format!("PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n")
                .into_bytes()
        })
        .collect();

    let report = classify_routing(&records, 2);
    assert_eq!(report["gpio_routing_complete"], true);
    assert_eq!(report["gpio_routing_records"][1]["share_pad"], "80000355");
    assert_eq!(report["gpio_routing_records"][1]["ext_a"], "00006001");

    assert_eq!(
        classify_routing(&records, 1)["gpio_routing_complete"],
        false
    );
}

#[test]
fn readback_v2_reports_samples_without_changing_outcome() {
    let detail = b"PLDBG1 pin=PA0 stage=verify want=1 latch=1 input=0 highs=17/64 directions=1 stopped=1 pad=00005c00\r\n";
    let failed = classify_led(
        &[
            &b"PLLED1 begin\r\n"[..],
            detail,
            b"PLLED1 error step=unverified\r\n",
        ]
        .concat(),
        2,
    );

    assert_eq!(outcome(&failed), "led_failed");
    assert_eq!(failed["led_readbacks"][0]["input_high_samples"], 17);
    assert_eq!(failed["led_readbacks"][0]["input_high"], false);

    for bad in [&b"highs=65/64"[..], b"highs=17/32", b"highs=017/64"] {
        assert_eq!(
            classify_led(&replace(detail, b"highs=17/64", bad), 2)["led_readbacks"],
            json!([])
        );
    }

    assert_eq!(classify_led(detail, 1)["led_readbacks"], json!([]));
}

pub(super) const CLOCK_LINE: &[u8] = b"PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n";

#[test]
fn shared_pad_record_must_be_intact_single_preserving_and_before_km4() {
    let line: &[u8] = b"PLPWR1 before_func=0201019a before_pad=80000000 after_func=1201019a after_pad=80000355\n\r";
    let clock = CLOCK_LINE;
    let report = classify_power(&[line, clock].concat(), 1);
    assert_eq!(report["shared_pad_verified"], true);
    assert_eq!(report["shared_pad_control_after"], "80000355");

    let cases: Vec<(Vec<u8>, &str)> = vec![
        (clock.to_vec(), "shared_pad_missing"),
        ([clock, line].concat(), "shared_pad_order"),
        ([line, line, clock].concat(), "shared_pad_malformed"),
        (
            [&line[..40], &clock[..20], &line[40..], &clock[20..]].concat(),
            "shared_pad_malformed",
        ),
        (
            [
                &replace(line, b"after_func=1201019a", b"after_func=0201019a")[..],
                clock,
            ]
            .concat(),
            "shared_pad_failed",
        ),
        (
            [
                &replace(line, b"after_func=1201019a", b"after_func=1201019b")[..],
                clock,
            ]
            .concat(),
            "shared_pad_failed",
        ),
        (
            [
                &replace(line, b"after_pad=80000355", b"after_pad=80000155")[..],
                clock,
            ]
            .concat(),
            "shared_pad_failed",
        ),
        (
            [
                &replace(line, b"after_pad=80000355", b"after_pad=c0000355")[..],
                clock,
            ]
            .concat(),
            "shared_pad_failed",
        ),
    ];

    for (rx, expected) in cases {
        assert_eq!(outcome(&classify_power(&rx, 1)), expected);
    }
}

#[test]
fn shared_pad_v2_requires_stock_ldo_trim() {
    let line = |pad: &str| {
        [
            format!("PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad={pad}\n\r").as_bytes(),
            CLOCK_LINE,
        ]
        .concat()
    };

    for (pad, v1, v2) in [
        ("00000301", "pass", "shared_pad_failed"),
        ("00000355", "pass", "pass"),
        ("000003d1", "pass", "pass"),
        ("00000353", "pass", "shared_pad_failed"),
    ] {
        assert_eq!(outcome(&classify_power(&line(pad), 1)), v1, "{pad}");
        assert_eq!(outcome(&classify_power(&line(pad), 2)), v2, "{pad}");
    }

    assert_eq!(
        classify_power(&line("00000355"), 2)["shared_pad_ldo_trim"],
        "2a"
    );
}

pub(super) const POWER_V3_LINE: &[u8] = b"PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r";

#[test]
fn shared_pad_v3_reports_port_a_samples() {
    let rx = [POWER_V3_LINE, CLOCK_LINE].concat();
    let report = classify_power(&rx, 3);
    assert_eq!(outcome(&report), "pass");
    assert_eq!(
        report["shared_pad_port_a_samples"],
        json!(["e1c46e80", "e1c46e00", "e1c46e00", "e1c46e00"])
    );

    assert_eq!(
        outcome(&classify_power(&replace(&rx, b"00000355", b"00000301"), 3)),
        "shared_pad_failed"
    );
    assert_eq!(
        outcome(&classify_power(&replace(&rx, b",e1c46e00\n", b"\n"), 3)),
        "shared_pad_malformed"
    );
    assert_eq!(outcome(&classify_power(&rx, 2)), "shared_pad_malformed");
}

#[test]
fn clock_bits_and_preservation() {
    let line: &[u8] = b"PLCLK1 function_before=00000041 clock_before=00000082 function_after=00100041 clock_after=00100082 status=pass\r\n";
    assert_eq!(classify_clock(line)["gpio_clock_verified"], true);

    for changed in [
        replace(line, b"00100041", b"00000041"),
        replace(line, b"00100082", b"00000082"),
        replace(line, b"00100082", b"00100083"),
        replace(line, b"status=pass", b"status=fail"),
    ] {
        assert_eq!(outcome(&classify_clock(&changed)), "gpio_clock_failed");
    }

    assert_eq!(outcome(&classify_clock(b"")), "gpio_clock_missing");
    assert_eq!(
        outcome(&classify_clock(&[line, line].concat())),
        "gpio_clock_repeated"
    );
}

#[test]
fn led_sequence_and_terminal_failures() {
    let records = led_full(1);
    let full = records.concat();
    let head = records[..records.len() - 1].concat();

    assert_eq!(classify_led(&full, 1)["led_probe_verified"], true);
    assert_eq!(
        classify_led(&full, 1)["led_input_verified_pins"],
        json!(["PA0", "PA2", "PA4", "PB22"])
    );
    assert_eq!(classify_led(&head, 1)["led_input_verified_pins"], json!([]));

    assert_eq!(
        outcome(&classify_led(
            &replace_n(&full, b"status=pass", b"status=fail", 1),
            1
        )),
        "led_failed"
    );
    assert_eq!(
        outcome(&classify_led(
            &[&full[..], b"PLLED1 error finish=input_mismatch low=all\r\n"].concat(),
            1
        )),
        "led_failed"
    );

    let detail: &[u8] = b"PLDBG1 pin=PA0 stage=verify want=1 latch=1 input=0 directions=1 stopped=1 pad=00000000\r\n";
    let failed = classify_led(
        &[&records[0][..], detail, b"PLLED1 error step=unverified\r\n"].concat(),
        1,
    );
    assert_eq!(outcome(&failed), "led_failed");
    assert_eq!(
        failed["led_readbacks"][0],
        json!({"pin": "PA0", "stage": "verify", "want_high": true, "latch_high": true, "input_high": false,
               "directions_ok": true, "stopped_ok": true, "pad_control": "00000000"})
    );

    let last = records.last().unwrap();
    for (rx, expected) in [
        (Vec::new(), "led_missing"),
        (head.clone(), "led_incomplete"),
        ([&full[..], last].concat(), "led_unexpected_sequence"),
        (
            replace(&full, b"pin=PA2", b"pin=PA3"),
            "led_unexpected_sequence",
        ),
        (
            [&full[..], b"PLLED1 error step=unverified\r\n"].concat(),
            "led_failed",
        ),
        (full[..full.len() - 2].to_vec(), "led_unexpected_sequence"),
    ] {
        assert_eq!(outcome(&classify_led(&rx, 1)), expected);
    }
}

#[test]
fn led_v3_labels_port_a_output_only_and_never_passes_them() {
    let full = led_full(3).concat();
    let report = classify_led(&full, 3);
    assert_eq!(outcome(&report), "pass");
    assert_eq!(report["led_output_only_pins"], json!(["PA0", "PA2", "PA4"]));
    // Accepts the sequence, but only PB22 was input-verified.
    assert_eq!(report["led_probe_verified"], false);
    assert_eq!(report["led_input_verified_pins"], json!(["PB22"]));

    for (bad, expected) in [
        (
            replace(
                &full,
                b"pin=PA0 level=1 status=output",
                b"pin=PA0 level=1 status=pass",
            ),
            "led_unexpected_sequence",
        ),
        (
            replace_n(
                &full,
                b"pin=PB22 level=1 status=pass",
                b"pin=PB22 level=1 status=output",
                1,
            ),
            "led_unexpected_sequence",
        ),
        (
            replace(&full, b" output_only=PA0,PA2,PA4", b""),
            "led_unexpected_sequence",
        ),
        (
            replace_n(
                &full,
                b"pin=PB22 level=1 status=pass",
                b"pin=PB22 level=1 status=fail",
                1,
            ),
            "led_failed",
        ),
    ] {
        assert_eq!(outcome(&classify_led(&bad, 3)), expected);
    }

    assert_ne!(outcome(&classify_led(&full, 2)), "pass");
}

pub(super) const OUTPUTS_LINE: &[u8] =
    b"PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n";

#[test]
fn stopped_profile_requires_correct_levels_directions_and_order() {
    let good = [OUTPUTS_LINE, &boot(ID)].concat();
    assert_eq!(outcome(&classify(&good, ID, Stopped::Required)), "pass");
    assert_eq!(
        classify(&good, ID, Stopped::Required)["stopped_gpio_verified"],
        true
    );

    assert_eq!(
        outcome(&classify(
            &[&good[..], &good].concat(),
            ID,
            Stopped::Required
        )),
        "repeated_startup"
    );
    assert_eq!(
        outcome(&classify(&boot(ID), ID, Stopped::Required)),
        "outputs_missing"
    );
    assert_eq!(
        outcome(&classify(
            &[&boot(ID)[..], OUTPUTS_LINE].concat(),
            ID,
            Stopped::Required
        )),
        "outputs_after_startup"
    );

    for (old, new) in [
        (&b"pa=00006000"[..], &b"pa=00002000"[..]),
        (b"pb=00000000", b"pb=00000008"),
        (b"dira=0000e000", b"dira=00006000"),
        (b"dirb=00000008", b"dirb=00000000"),
        (b"status=pass", b"status=fail"),
    ] {
        let rx = [&replace(OUTPUTS_LINE, old, new)[..], &boot(ID)].concat();
        assert_eq!(
            outcome(&classify(&rx, ID, Stopped::Required)),
            "outputs_failed"
        );
    }
}

#[test]
fn classification() {
    let b = boot(ID);
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (b.clone(), "pass"),
        (vec![], "timeout"),
        (start(ID), "incomplete_report"),
        (boot(&"b".repeat(64)), "wrong_build"),
        ([&b[..], &b].concat(), "repeated_startup"),
        ([&b[..], b"HardFault"].concat(), "fault"),
        ([&b[..], FAIL].concat(), "diagnostic_failed"),
        (replace(&b, b"bank=1", b"bank=unknown"), "unknown_mapping"),
        (replace(&b, b"03e7a000", b"ffffffff"), "unknown_mapping"),
        (replace(&b, b"00000003,", b"00000000,"), "unknown_mapping"),
        (
            replace(&b, b"PLFW1 result ", b"PLFW1 invalid "),
            "malformed_report",
        ),
        ([PASS, &start(ID)].concat(), "incomplete_report"),
        ([&b[..], PASS].concat(), "malformed_report"),
    ];
    for (rx, expected) in cases {
        assert_eq!(outcome(&classify(&rx, ID, Stopped::Optional)), expected);
    }

    assert_eq!(
        classify(&b, ID, Stopped::Optional)["reset_flags"],
        "00100000"
    );
    let bank0 = replace(&replace(&b, b"bank=1", b"bank=0"), b"03e7a000", b"03ffa000");
    assert_eq!(outcome(&classify(&bank0, ID, Stopped::Optional)), "pass");
}

#[test]
fn clock_policy_binding_and_order() {
    let mut f = Fixture::new(json!({}));
    let id = f.identity.clone();
    f.set("startup_output_policy", json!("stop-only"));
    f.set("startup_gpio_clock_policy", json!("explicit-enable-v1"));
    let image =
        format!("PLFW1 start build={id} PLFW1 outputs pa= PLCLK1 function_before=").into_bytes();
    f.image(&image);

    // One chunk at 10:00:00, ending at 10:02:00.
    let once = |f: &Fixture, rx: Vec<u8>| f.capture(&[(0.0, rx)], 120.0);
    for (rx, expected) in [
        ([CLOCK_LINE, OUTPUTS_LINE, &boot(&id)].concat(), "pass"),
        (
            [OUTPUTS_LINE, CLOCK_LINE, &boot(&id)].concat(),
            "gpio_clock_order",
        ),
        ([OUTPUTS_LINE, &boot(&id)].concat(), "gpio_clock_missing"),
    ] {
        once(&f, rx);
        assert_eq!(outcome(&f.verify(60.0).unwrap()), expected);
    }

    let routed = [&image[..], b"PLIO1 stage="].concat();
    f.set("startup_gpio_routing_policy", json!("read-only-v1"));
    f.image(&routed);
    let records = routing_v1();
    for (snapshots, expected) in [
        (records.concat(), "pass"),
        (records[..8].concat(), "gpio_routing_incomplete"),
    ] {
        once(
            &f,
            [CLOCK_LINE, &snapshots, OUTPUTS_LINE, &boot(&id)].concat(),
        );
        let report = f.verify(60.0).unwrap();
        assert_eq!(outcome(&report), expected);
        assert_eq!(
            report["gpio_routing_records"].as_array().unwrap().len(),
            count(&snapshots, b"PLIO1")
        );
    }

    f.set("startup_gpio_routing_policy", Value::Null);
    assert!(f.verify(60.0).is_err());
    f.set("startup_gpio_routing_policy", json!("read-only-v2"));
    assert!(f.verify(60.0).is_err());
    f.set("startup_gpio_routing_policy", json!("read-only-v1"));
    f.set("startup_shared_pad_policy", json!("before-km4-v1"));
    assert!(f.verify(60.0).is_err());
    f.set("startup_shared_pad_policy", Value::Null);

    let power_image = [
        &routed[..],
        b"PLPWR1 before_func=%08x before_pad=%08x after_func=%08x ",
        POWER_V3_FORMAT,
    ]
    .concat();
    f.image(&power_image);
    once(
        &f,
        [
            POWER_V3_LINE,
            CLOCK_LINE,
            &records.concat(),
            OUTPUTS_LINE,
            &boot(&id),
        ]
        .concat(),
    );
    f.set("startup_shared_pad_policy", json!("before-km4-v3"));
    let report = f.verify(60.0).unwrap();
    assert_eq!(outcome(&report), "pass");
    assert_eq!(report["shared_pad_verified"], true);

    // Downgrades: omitting the policy, or declaring an older one, is refused for a v3 image.
    for policy in [Value::Null, json!("before-km4-v1"), json!("before-km4-v2")] {
        f.set("startup_shared_pad_policy", policy);
        assert!(matches!(
            f.error(60.0),
            Error::Policy(PolicyError::Mismatch {
                declaration: Declaration::SharedPadV3
            })
        ));
    }

    f.set("startup_shared_pad_policy", Value::Null);
    f.image(&routed);
    f.set("startup_gpio_clock_policy", Value::Null);
    assert!(f.verify(60.0).is_err());
}

#[test]
fn evidence_artifact_binding_and_window() {
    let mut f = Fixture::new(json!({"source": "f".repeat(64)}));
    let id = f.identity.clone();
    let image = [
        &b"container "[..],
        format!("PLFW1 start build={id} ").as_bytes(),
    ]
    .concat();
    f.image(&image);

    let rx = boot(&id);
    f.capture(&[(0.0, rx.clone())], 120.0);
    assert_eq!(outcome(&f.verify(60.0).unwrap()), "pass");
    assert_eq!(outcome(&f.verify(180.0).unwrap()), "observation_too_short");

    for (key, value) in [
        ("startup_gpio_routing_policy", "read-only-v1"),
        ("startup_output_policy", "stop-only"),
        ("startup_led_policy", "bounded-probe-v1"),
    ] {
        f.set(key, json!(value));
        assert!(f.verify(60.0).is_err(), "{key}");
        f.set(key, Value::Null);
    }

    let sequence = led_full(1).concat();
    let led_image = [&image[..], b"PLLED1 begin"].concat();
    f.set("startup_led_policy", json!("bounded-probe-v1"));
    f.image(&led_image);
    for (received, expected) in [
        ([&rx[..], &sequence].concat(), "pass"),
        ([&sequence[..], &rx].concat(), "led_unexpected_sequence"),
        (
            [
                &rx[..],
                &sequence[..sequence.len() - b"PLLED1 done low=all\r\n".len()],
            ]
            .concat(),
            "led_incomplete",
        ),
    ] {
        f.capture(&[(0.0, received)], 120.0);
        assert_eq!(outcome(&f.verify(60.0).unwrap()), expected);
    }

    f.set("startup_led_policy", Value::Null);
    f.image(&image);
    f.capture(&[(0.0, rx.clone())], 120.0);

    // A gap in the event offsets, a changed artifact, and a changed source are refused.
    let events = fs::read_to_string(f.path().join("serial-events.jsonl")).unwrap();
    fs::write(
        f.path().join("serial-events.jsonl"),
        events.replace("\"offset\":0", "\"offset\":1"),
    )
    .unwrap();
    assert!(f.verify(60.0).is_err());

    fs::write(f.path().join("serial-events.jsonl"), events).unwrap();
    fs::write(
        f.path().join("image.bin"),
        [&image[..], b"corrupt"].concat(),
    )
    .unwrap();
    assert!(f.verify(60.0).is_err());

    fs::write(f.path().join("image.bin"), &image).unwrap();
    f.manifest["inputs"]["source"] = json!("e".repeat(64));
    f.set("profile", json!("firstboot"));
    assert!(matches!(
        f.error(60.0),
        Error::Startup(StartupError::NotSourceBound)
    ));
}

/// Builds signed with the update key, and variant builds, embed an identity
/// that binds the key and variant too.
#[test]
fn keyed_variant_builds_are_source_bound() {
    let mut f = Fixture::new(json!({"source": "f".repeat(64)}));
    f.manifest["variant"] = json!("speaker");
    f.manifest["ota_public_key"] = json!("ab".repeat(32));
    let id = f.typed().source_identity().unwrap();
    assert_ne!(id, f.identity);
    f.manifest["startup_identity"] = json!(id);

    let image = [
        &b"container "[..],
        format!("PLFW1 start build={id} ").as_bytes(),
    ]
    .concat();
    f.image(&image);
    f.capture(&[(0.0, boot(&id))], 120.0);
    assert_eq!(outcome(&f.verify(60.0).unwrap()), "pass");

    // The same sources without the variant are another build.
    f.set("variant", Value::Null);
    assert!(matches!(
        f.error(60.0),
        Error::Startup(StartupError::NotSourceBound)
    ));
}

#[test]
fn low_only_led_policy_expects_no_pulses() {
    let low = b"PLLED1 begin low_only\r\nPLLED1 done low=all\r\n";
    let result = classify_led(low, 4);
    assert_eq!(result["outcome"], "pass");
    assert_eq!(result["led_low_verified"], true);
    assert_eq!(result["led_probe_verified"], false);
    assert_eq!(result["led_input_verified_pins"], json!([]));

    // A pulse record is not part of this policy.
    let pulsed =
        b"PLLED1 begin low_only\r\nPLLED1 pin=PA0 level=1 status=output\r\nPLLED1 done low=all\r\n";
    assert_eq!(
        classify_led(pulsed, 4)["outcome"],
        "led_unexpected_sequence"
    );
    assert_eq!(
        classify_led(b"PLLED1 begin low_only\r\n", 4)["outcome"],
        "led_incomplete"
    );
}

#[test]
fn routing_v3_has_only_the_startup_stage() {
    let v2_startup: Vec<u8> = b"PLIO1 stage=startup groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=00000355 ext_a=e9c46e00\r\n".to_vec();
    assert_eq!(classify_routing(&v2_startup, 3)["outcome"], "pass");
    assert_eq!(
        classify_routing(&v2_startup, 2)["outcome"],
        "gpio_routing_incomplete"
    );
}
