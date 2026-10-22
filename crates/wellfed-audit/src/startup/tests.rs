//! The startup auditor's checks: classifiers, policy binding both ways, and
//! the FW06 handshake, mediated read and scratch-sector write records.

use serde_json::json;

use super::*;
use crate::capture::Time;
use crate::test_support::*;

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn start(identity: &str) -> Vec<u8> {
    format!("PLFW1 start build={identity} bank=1 reset=00100000 mmu=00000003,0c000000,03e7a000\r\n")
        .into_bytes()
}

fn boot(identity: &str) -> Vec<u8> {
    [
        &b"Wellfed local firmware: Rust bringup\r\n"[..],
        &start(identity),
        b"Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n",
        PASS,
    ]
    .concat()
}

const STAGES: [&str; 9] = [
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

const CLOCK_LINE: &[u8] = b"PLCLK1 function_before=00000000 clock_before=00000000 function_after=00100000 clock_after=00100000 status=pass\r\n";

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

const POWER_V3_LINE: &[u8] = b"PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r";

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
    // FW04 accepts the sequence, but only PB22 was input-verified.
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

const OUTPUTS_LINE: &[u8] =
    b"PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n";

#[test]
fn stopped_profile_requires_correct_levels_directions_and_order() {
    let good = [OUTPUTS_LINE, &boot(ID)].concat();
    assert_eq!(outcome(&classify(&good, ID, true)), "pass");
    assert_eq!(classify(&good, ID, true)["stopped_gpio_verified"], true);
    assert_eq!(
        outcome(&classify(&[&good[..], &good].concat(), ID, true)),
        "repeated_startup"
    );
    assert_eq!(outcome(&classify(&boot(ID), ID, true)), "outputs_missing");
    assert_eq!(
        outcome(&classify(&[&boot(ID)[..], OUTPUTS_LINE].concat(), ID, true)),
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
        assert_eq!(outcome(&classify(&rx, ID, true)), "outputs_failed");
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
        assert_eq!(outcome(&classify(&rx, ID, false)), expected);
    }
    assert_eq!(classify(&b, ID, false)["reset_flags"], "00100000");
    let bank0 = replace(&replace(&b, b"bank=1", b"bank=0"), b"03e7a000", b"03ffa000");
    assert_eq!(outcome(&classify(&bank0, ID, false)), "pass");
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
        assert!(f.error(60.0).contains("Shared-pad v3"));
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
    f.set("profile", json!("plaf103-firstboot"));
    assert!(f.error(60.0).contains("not source-bound"));
}

/// A line arrives at the time of the latest flash record up to and including it.
fn run_flash(lines: &[&[u8]], times: [f64; 4], version: u8, end: Option<Time>) -> Report {
    let mut rx = Vec::new();
    let mut spans = Vec::new();
    let mut now = 0.0;
    for line in lines {
        for (kind, seconds) in ["hello", "ready", "peer", "done"].iter().zip(times) {
            if line.starts_with(format!("PLFLS1 {kind} ").as_bytes()) {
                now = seconds;
            }
        }
        spans.push((rx.len(), rx.len() + line.len(), now));
        rx.extend_from_slice(line);
    }
    let arrival = |offset: usize| -> Result<Time> {
        Ok(at(spans
            .iter()
            .find(|(a, b, _)| *a <= offset && offset < *b)
            .unwrap()
            .2))
    };
    classify_flash(&rx, &arrival, version, end).unwrap()
}

#[test]
fn flash_handshake_records_order_fields_and_timing() {
    let led: &[u8] = b"PLLED1 done low=all\r\n";
    let hello: &[u8] = b"PLFLS1 hello authority=000000030000a1b2 count=3\r\n";
    let ready: &[u8] = b"PLFLS1 ready peer=000000070000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n";
    let peer: &[u8] = b"PLFLS1 peer peer=000000080000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n";
    let done: &[u8] = b"PLFLS1 done phase=8 fault=7 blocked=1\r\n";
    let foreign: &[u8] = b"PLFLS1 foreign\r\n";
    let times = [0.0, 1.0, 11.0, 31.0];
    let good = [led, hello, ready, peer, done];
    let report = run_flash(&good, times, 1, None);
    assert_eq!(outcome(&report), "pass");
    assert_eq!(report["flash_handshake_verified"], true);
    assert_eq!(report["flash_authority_session"], "000000030000a1b2");
    assert_eq!(report["flash_boot_count"], 3);
    assert_eq!(report["flash_peer_session"], "000000070000c3d4");
    assert_eq!(report["flash_injected_peer_session"], "000000080000e5f6");
    assert_eq!(
        (
            report["flash_ready_seconds"].as_f64(),
            report["flash_peer_change_seconds"].as_f64(),
            report["flash_done_seconds"].as_f64()
        ),
        (Some(1.0), Some(10.0), Some(30.0))
    );
    assert_eq!(
        outcome(&run_flash(
            &[led, hello, foreign, ready, peer, done],
            times,
            1,
            None
        )),
        "pass"
    );
    assert_eq!(
        outcome(&run_flash(
            &[led, hello, ready, peer, foreign, done],
            times,
            1,
            None
        )),
        "pass"
    );
    let r = |old: &[u8], new: &[u8], line: &[u8]| replace(line, old, new);
    let (h_count, h_zero) = (
        r(b"count=3", b"count=x3", hello),
        r(
            b"count=3",
            b"count=0",
            &r(b"000000030000a1b2", b"0000000000000000", hello),
        ),
    );
    let cases: Vec<(Vec<Vec<u8>>, &str)> = vec![
        (vec![led.into()], "flash_handshake_missing"),
        (
            vec![led.into(), hello.into(), ready.into()],
            "flash_handshake_missing",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                done.into(),
                done.into(),
            ],
            "flash_handshake_malformed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                foreign.into(),
                foreign.into(),
                ready.into(),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_malformed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                done.into(),
                b"PLFLS1 untrusted\r\n".to_vec(),
            ],
            "flash_handshake_malformed",
        ),
        (
            vec![led.into(), h_count, ready.into(), peer.into(), done.into()],
            "flash_handshake_malformed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"000000070000c3d4", b"0000000700c3d4", ready),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_malformed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                b"PLFLS1 error timeout ms=2500\r\n".to_vec(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                done.into(),
                b"PLFLS1 error clock\r\n".to_vec(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                r(b"count=3", b"count=4", hello),
                ready.into(),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![led.into(), h_zero, ready.into(), peer.into(), done.into()],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"000000070000c3d4", b"000000030000a1b2", ready),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"000000070000c3d4", b"000000000000c3d4", ready),
                r(b"0000000800", b"0000000100", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"inactive=1", b"inactive=0", ready),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"reply=0", b"reply=1", ready),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                r(b"phase=1", b"phase=2", ready),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                r(b"000000080000e5f6", b"000000070000c3d4", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                r(b"000000080000e5f6", b"000000090000e5f6", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                r(b"reply=5", b"reply=0", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                r(b"fault=7", b"fault=6", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                r(b"blocked=1", b"blocked=0", peer),
                done.into(),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                r(b"phase=8", b"phase=1", done),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                r(b"blocked=1", b"blocked=0", done),
            ],
            "flash_handshake_failed",
        ),
        (
            vec![
                hello.into(),
                led.into(),
                ready.into(),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_order",
        ),
        (
            vec![hello.into(), ready.into(), peer.into(), done.into()],
            "flash_handshake_order",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                peer.into(),
                ready.into(),
                done.into(),
            ],
            "flash_handshake_order",
        ),
        (
            vec![
                led.into(),
                foreign.into(),
                hello.into(),
                ready.into(),
                peer.into(),
                done.into(),
            ],
            "flash_handshake_order",
        ),
        (
            vec![
                led.into(),
                hello.into(),
                done.into(),
                ready.into(),
                peer.into(),
            ],
            "flash_handshake_order",
        ),
    ];
    for (lines, expected) in cases {
        let refs: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
        let result = run_flash(&refs, times, 1, None);
        assert_eq!(outcome(&result), expected, "{lines:?}");
        assert_eq!(result["flash_handshake_verified"], false);
        assert!(!json!(result).to_string().contains("untrusted"));
    }
    for times in [
        [0.0, 2.6, 12.6, 32.6],
        [0.0, 1.0, 9.9, 31.0],
        [0.0, 1.0, 13.1, 31.0],
        [0.0, 1.0, 11.0, 29.9],
        [0.0, 1.0, 11.0, 33.1],
    ] {
        assert_eq!(
            outcome(&run_flash(&good, times, 1, None)),
            "flash_handshake_timing",
            "{times:?}"
        );
    }
    assert_eq!(
        outcome(&run_flash(&good, [0.0, 2.5, 11.5, 34.5], 1, None)),
        "pass"
    );
}

#[test]
fn flash_handshake_policy_binding_and_legacy() {
    let mut f = Fixture::new(json!({}));
    let id = f.identity.clone();
    f.set("startup_led_policy", json!("bounded-probe-v1"));
    let sequence = led_full(1).concat();
    let records: [&[u8]; 4] = [
        b"PLFLS1 hello authority=000000010000a1b2 count=1\r\n",
        b"PLFLS1 ready peer=000000010000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n",
        b"PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n",
        b"PLFLS1 done phase=8 fault=7 blocked=1\r\n",
    ];
    let write = |f: &mut Fixture, image: &[u8], chunks: &[Vec<u8>], seconds: &[f64]| {
        f.image(image);
        let timed: Vec<(f64, Vec<u8>)> = seconds
            .iter()
            .copied()
            .zip(chunks.iter().cloned())
            .collect();
        f.capture(&timed, 120.0);
        f.verify(60.0)
    };
    let legacy_image = format!("PLFW1 start build={id} PLLED1 begin").into_bytes();
    let legacy = write(
        &mut f,
        &legacy_image,
        &[[&boot(&id)[..], &sequence].concat()],
        &[0.0],
    )
    .unwrap();
    assert_eq!(outcome(&legacy), "pass");
    assert!(!legacy.keys().any(|k| k.starts_with("flash_")));
    let image = [&legacy_image[..], b" ", FLASH_MARK].concat();
    let chunks = vec![
        [&boot(&id)[..], &sequence, records[0]].concat(),
        records[1].to_vec(),
        records[2].to_vec(),
        records[3].to_vec(),
    ];
    let times = [0.0, 1.0, 11.0, 31.0];
    // Image carries the records but the manifest omits the policy.
    let error = write(&mut f, &image, &chunks, &times)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Flash handshake policy/artifact mismatch"));
    f.set("startup_flash_handshake_policy", json!("fw06-hello-v1"));
    let report = write(&mut f, &image, &chunks, &times).unwrap();
    assert_eq!(outcome(&report), "pass");
    assert_eq!(report["flash_handshake_verified"], true);
    assert_eq!(report["flash_done_seconds"].as_f64(), Some(30.0));
    assert_eq!(
        outcome(&write(&mut f, &image, &chunks, &[0.0, 1.0, 5.0, 31.0]).unwrap()),
        "flash_handshake_timing"
    );
    assert_eq!(
        outcome(&write(&mut f, &image, &chunks[..2], &[0.0, 1.0]).unwrap()),
        "flash_handshake_missing"
    );
    // A failing startup keeps its outcome and still reports the flash fields.
    let failing = vec![
        [&boot(&id)[..], FAIL, &sequence, records[0]].concat(),
        records[1].to_vec(),
        records[2].to_vec(),
        records[3].to_vec(),
    ];
    let failed = write(&mut f, &image, &failing, &times).unwrap();
    assert_eq!(outcome(&failed), "diagnostic_failed");
    assert_eq!(failed["flash_handshake_verified"], true);
    // Manifest declares the policy but the image lacks the records.
    let error = write(&mut f, &legacy_image, &chunks, &times)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Flash handshake policy/artifact mismatch"));
    f.set("startup_flash_handshake_policy", json!("fw06-hello-v2"));
    let error = write(&mut f, &image, &chunks, &times)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Unknown flash handshake policy"));
    f.set("startup_flash_handshake_policy", json!("fw06-hello-v1"));
    f.set("startup_led_policy", Value::Null);
    let bare = [format!("PLFW1 start build={id} ").as_bytes(), FLASH_MARK].concat();
    let error = write(&mut f, &bare, &chunks, &times)
        .unwrap_err()
        .to_string();
    assert!(error.contains("requires the LED probe policy"));
}

// FW06 stage D (mediated read) and stage E (scratch-sector write).

const HELLO: &[u8] = b"PLFLS1 hello authority=000000010000a1b2 count=1\r\n";
const READY: &[u8] = b"PLFLS1 ready peer=000000010000c3d4 reply=0 phase=1 fault=0 inactive=1\r\n";
const ROUTING_V2_LINE: &str = "groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001";

/// (host seconds, bytes, label) receive chunks.
type Chunks = Vec<(f64, Vec<u8>, String)>;

fn stage_boot(identity: &str) -> Vec<u8> {
    let routing: Vec<u8> = STAGES
        .iter()
        .flat_map(|s| format!("PLIO1 stage={s} {ROUTING_V2_LINE}\r\n").into_bytes())
        .collect();
    let start = format!(
        "PLFW1 start build={identity} bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000\r\n"
    );
    [
        POWER_V3_LINE,
        CLOCK_LINE,
        &routing,
        OUTPUTS_LINE,
        b"Wellfed local firmware: Rust bringup\r\n",
        start.as_bytes(),
        b"Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n",
        PASS,
    ]
    .concat()
}

fn stage_prefix(identity: &str, ready_delay: f64) -> Chunks {
    let led = led_full(3);
    vec![
        (0.0, stage_boot(identity), "startup".into()),
        (3.0, led[0].clone(), "led".into()),
        (17.0, led[1..].concat(), "led".into()),
        (17.2, HELLO.to_vec(), "hello".into()),
        (17.2 + ready_delay, READY.to_vec(), "ready".into()),
    ]
}

struct Stage {
    f: Fixture,
    base: Vec<u8>,
}

impl Stage {
    fn new(policy: &str, mark: &[u8]) -> Self {
        let mut f = Fixture::new(json!({"source": "f".repeat(64)}));
        for (key, value) in [
            ("startup_output_policy", "stop-only"),
            ("startup_led_policy", "bounded-probe-v3"),
            ("startup_gpio_routing_policy", "read-only-v2"),
            ("startup_shared_pad_policy", "before-km4-v3"),
            ("startup_gpio_clock_policy", "explicit-enable-v1"),
            ("startup_flash_handshake_policy", policy),
        ] {
            f.set(key, json!(value));
        }
        let base = [
            format!(
                "PLFW1 start build={} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= PLIO1 stage= share_pad= PLPWR1 before_func=",
                f.identity
            )
            .as_bytes(),
            POWER_V3_FORMAT,
            b" ",
            FLASH_MARK,
        ]
        .concat();
        f.image(&[&base[..], b" ", mark].concat());
        Self { f, base }
    }

    fn audit(&self, (chunks, end): &(Chunks, f64)) -> Result<Report> {
        let plain: Vec<(f64, Vec<u8>)> = chunks.iter().map(|(t, d, _)| (*t, d.clone())).collect();
        self.f.capture(&plain, *end);
        self.f.verify(1.0)
    }
}

/// Case builders over a good capture, as the Python tests' helpers.
struct Edits {
    good: Chunks,
    end: f64,
}

impl Edits {
    fn at(&self, label: &str) -> f64 {
        self.good.iter().find(|c| c.2 == label).unwrap().0
    }

    fn edit(&self, label: &str, old: &[u8], new: &[u8]) -> (Chunks, f64) {
        let chunks = self
            .good
            .iter()
            .map(|(t, d, k)| {
                (
                    *t,
                    if k == label {
                        replace(d, old, new)
                    } else {
                        d.clone()
                    },
                    k.clone(),
                )
            })
            .collect();
        (chunks, self.end)
    }

    fn drop(&self, labels: &[&str]) -> (Chunks, f64) {
        (
            self.good
                .iter()
                .filter(|c| !labels.contains(&c.2.as_str()))
                .cloned()
                .collect(),
            self.end,
        )
    }

    fn add(&self, label: &str, data: &[u8], delay: f64) -> (Chunks, f64) {
        let mut chunks = self.good.clone();
        chunks.push((self.at(label) + delay, data.to_vec(), "extra".into()));
        (chunks, self.end)
    }

    fn swap(&self, label: &str, data: Vec<u8>) -> (Chunks, f64) {
        let chunks = self
            .good
            .iter()
            .map(|(t, d, k)| {
                (
                    *t,
                    if k == label { data.clone() } else { d.clone() },
                    k.clone(),
                )
            })
            .collect();
        (chunks, self.end)
    }

    fn retime(&self, label: &str, seconds: f64) -> (Chunks, f64) {
        let chunks = self
            .good
            .iter()
            .map(|(t, d, k)| (if k == label { seconds } else { *t }, d.clone(), k.clone()))
            .collect();
        (chunks, self.end)
    }
}

const READ_START_LINE: &[u8] = b"PLRDS1 start marker=00000000 watchdog=pass installed=1\r\n";
const READ_DONE_LINE: &[u8] = b"PLRDS1 done reads=100 leases=100 phase=1 fault=0\r\n";

fn batch(
    reads: u64,
    ident: &str,
    mismatches: u64,
    park: u64,
    release: u64,
    frozen: Option<u64>,
    alive: Option<u64>,
) -> Vec<u8> {
    format!(
        "PLRDS1 batch reads={reads} id={ident} mismatches={mismatches} park_max={park} release_max={release} frozen={} alive={}\r\n",
        frozen.unwrap_or(reads),
        alive.unwrap_or(reads)
    )
    .into_bytes()
}

fn ok_batch(reads: u64) -> Vec<u8> {
    batch(reads, "c84017", 0, 6, 2, None, None)
}

fn read_capture(identity: &str, read_seconds: f64, tail: f64, ready_delay: f64) -> (Chunks, f64) {
    let mut out = stage_prefix(identity, ready_delay);
    let t = out.last().unwrap().0 + 0.05;
    out.push((t, READ_START_LINE.to_vec(), "start".into()));
    for (n, reads) in READ_BATCHES.iter().enumerate() {
        let n = n as f64 + 1.0;
        let skew = if n == 4.0 { 0.01 } else { 0.0 };
        out.push((
            t + read_seconds * n / 4.0 - skew,
            ok_batch(*reads),
            format!("batch{reads}"),
        ));
    }
    out.push((t + read_seconds, READ_DONE_LINE.to_vec(), "done".into()));
    (out, t + read_seconds + tail)
}

#[test]
fn single_boot_read_passes_with_fixed_fields() {
    let s = Stage::new("fw06-read-v1", READ_MARK);
    let id = s.f.identity.clone();
    let report = s.audit(&read_capture(&id, 8.0, 30.0, 0.3)).unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(report["flash_handshake_verified"], true);
    assert_eq!(report["flash_read_verified"], true);
    assert_eq!(
        (
            &report["flash_read_id"],
            &report["flash_read_reads"],
            &report["flash_read_leases"]
        ),
        (&json!("c84017"), &json!(100), &json!(100))
    );
    assert_eq!(
        (
            &report["flash_read_park_max"],
            &report["flash_read_release_max"]
        ),
        (&json!(6), &json!(2))
    );
    assert_eq!(
        (&report["flash_read_frozen"], &report["flash_read_alive"]),
        (&json!(100), &json!(100))
    );
    assert!((report["flash_read_seconds"].as_f64().unwrap() - 8.0).abs() < 1e-3);
    assert!((report["flash_read_tail_seconds"].as_f64().unwrap() - 30.0).abs() < 1e-3);
    assert_eq!(report["flash_authority_session"], "000000010000a1b2");
    assert_eq!(report["flash_boot_count"], 1);
    assert_eq!(report["flash_peer_session"], "000000010000c3d4");
    assert!(report["flash_injected_peer_session"].is_null());
    assert!(!json!(report).to_string().contains("PLRDS1"));
    // One foreign line, alive one below reads, bounds at the limit and unrelated output after done still pass.
    let (chunks, end) = read_capture(&id, 30.0, 30.0, 0.3);
    let mut chunks: Chunks = chunks
        .into_iter()
        .map(|(t, d, k)| {
            (
                t,
                if k == "batch50" {
                    batch(50, "c84017", 0, 1310, 1310, None, Some(49))
                } else {
                    d
                },
                k,
            )
        })
        .collect();
    chunks.push((17.3, b"PLFLS1 foreign\r\n".to_vec(), "foreign".into()));
    chunks.push((end - 1.0, b"unrelated log\r\n".to_vec(), "tail".into()));
    assert_eq!(outcome(&s.audit(&(chunks, end)).unwrap()), "pass");
}

#[test]
fn read_failures_have_distinct_outcomes() {
    let s = Stage::new("fw06-read-v1", READ_MARK);
    let id = s.f.identity.clone();
    let (good, end) = read_capture(&id, 8.0, 30.0, 0.3);
    let e = Edits { good, end };
    let all_read = ["start", "batch25", "batch50", "batch75", "batch100", "done"];
    let cases: Vec<((Chunks, f64), &str)> = vec![
        (e.drop(&all_read), "flash_read_missing"),
        (e.drop(&["done"]), "flash_read_missing"),
        (e.drop(&["start"]), "flash_read_missing"),
        (
            e.add("batch50", b"PLRDS1 untrusted\r\n", 0.001),
            "flash_read_malformed",
        ),
        (
            e.edit("batch25", b"id=c84017", b"id=C84017"),
            "flash_read_malformed",
        ),
        (
            e.edit("batch25", b"id=c84017", b"id=c8401"),
            "flash_read_malformed",
        ),
        (
            e.add("start", READ_START_LINE, 0.001),
            "flash_read_malformed",
        ),
        (
            e.add("batch100", READ_DONE_LINE, 0.001),
            "flash_read_malformed",
        ),
        (
            e.add("batch25", &ok_batch(25), 0.001),
            "flash_read_malformed",
        ),
        (
            e.add(
                "ready",
                b"PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n",
                0.001,
            ),
            "flash_read_malformed",
        ),
        (
            e.add("done", b"PLPRK1 done leases=100 marker=00000002\r\n", 0.001),
            "flash_read_malformed",
        ),
        (
            e.add("ready", b"PLRDS1 skip marker=00000001\r\n", 0.001),
            "flash_read_failed",
        ),
        (
            e.add("batch50", b"PLFLS1 error timeout ms=2500\r\n", 0.001),
            "flash_read_failed",
        ),
        (
            e.edit("start", b"marker=00000000", b"marker=00000001"),
            "flash_read_failed",
        ),
        (
            e.edit("start", b"watchdog=pass", b"watchdog=fail"),
            "flash_read_failed",
        ),
        (
            e.edit("start", b"installed=1", b"installed=0"),
            "flash_read_failed",
        ),
        (e.drop(&["batch75"]), "flash_read_failed"),
        (
            e.add("batch100", &ok_batch(125), 0.001),
            "flash_read_failed",
        ),
        (
            e.swap("batch50", batch(50, "ef4017", 0, 6, 2, None, None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch50", batch(50, "c84017", 1, 6, 2, None, None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch75", batch(75, "c84017", 0, 6, 2, Some(74), None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch75", batch(75, "c84017", 0, 6, 2, None, Some(73))),
            "flash_read_failed",
        ),
        (
            e.swap("batch25", batch(25, "c84017", 0, 0, 2, None, None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch25", batch(25, "c84017", 0, 1311, 2, None, None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch100", batch(100, "c84017", 0, 6, 0, None, None)),
            "flash_read_failed",
        ),
        (
            e.swap("batch100", batch(100, "c84017", 0, 6, 1311, None, None)),
            "flash_read_failed",
        ),
        (
            e.edit("done", b"reads=100", b"reads=99"),
            "flash_read_failed",
        ),
        (
            e.edit("done", b"leases=100", b"leases=99"),
            "flash_read_failed",
        ),
        (e.edit("done", b"phase=1", b"phase=8"), "flash_read_failed"),
        (e.edit("done", b"fault=0", b"fault=7"), "flash_read_failed"),
        (e.retime("start", e.at("ready") - 0.01), "flash_read_order"),
        (
            e.retime("batch25", e.at("batch75") + 0.01),
            "flash_read_order",
        ),
        (
            e.retime("batch100", e.at("done") + 0.01),
            "flash_read_order",
        ),
        (
            e.retime("batch25", e.at("start") - 0.001),
            "flash_read_order",
        ),
        (
            e.add("done", b"PLFLS1 foreign\r\n", 1.0),
            "flash_read_order",
        ),
        (read_capture(&id, 30.1, 30.0, 0.3), "flash_read_timing"),
        (read_capture(&id, 8.0, 29.9, 0.3), "flash_read_timing"),
        (e.drop(&["ready"]), "flash_handshake_missing"),
        (e.drop(&["hello"]), "flash_handshake_missing"),
        (
            e.add(
                "batch50",
                b"PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n",
                0.001,
            ),
            "flash_handshake_malformed",
        ),
        (
            e.add("done", b"PLFLS1 done phase=8 fault=7 blocked=1\r\n", 0.001),
            "flash_handshake_malformed",
        ),
        (
            e.add("hello", b"PLFLS1 error clock\r\n", 0.001),
            "flash_handshake_failed",
        ),
        (
            e.edit("hello", b"count=1", b"count=2"),
            "flash_handshake_failed",
        ),
        (
            e.edit("ready", b"inactive=1", b"inactive=0"),
            "flash_handshake_failed",
        ),
        (
            e.add("led", b"PLFLS1 foreign\r\n", 0.1),
            "flash_handshake_order",
        ),
        (read_capture(&id, 8.0, 30.0, 2.6), "flash_handshake_timing"),
    ];
    for (received, expected) in cases {
        let report = s.audit(&received).unwrap();
        assert_eq!(outcome(&report), expected);
        assert_eq!(report["flash_read_verified"], false);
        assert!(!json!(report).to_string().contains("PLRDS1"));
    }
}

#[test]
fn a_reset_is_a_repeated_startup() {
    let s = Stage::new("fw06-read-v1", READ_MARK);
    let (mut chunks, end) = read_capture(&s.f.identity.clone(), 8.0, 30.0, 0.3);
    let again: Chunks = chunks
        .iter()
        .map(|(t, d, k)| (t + end, d.clone(), k.clone()))
        .collect();
    chunks.extend(again);
    assert_eq!(
        outcome(&s.audit(&(chunks, end * 2.0)).unwrap()),
        "repeated_startup"
    );
}

#[test]
fn read_policy_binding_both_ways() {
    let mut s = Stage::new("fw06-read-v1", READ_MARK);
    let good = read_capture(&s.f.identity.clone(), 8.0, 30.0, 0.3);
    // A read image cannot be audited under another flash policy, or without one.
    for (policy, message) in [
        (
            json!("fw06-hello-v1"),
            "Flash read policy/artifact mismatch",
        ),
        (json!("fw06-park-v1"), "Flash park policy/artifact mismatch"),
        (Value::Null, "Flash handshake policy/artifact mismatch"),
        (json!("fw06-read-v2"), "Unknown flash handshake policy"),
    ] {
        s.f.set("startup_flash_handshake_policy", policy);
        assert!(
            s.audit(&good).unwrap_err().to_string().contains(message),
            "{message}"
        );
    }
    // A hello-only or park image cannot be audited under the read policy.
    s.f.set("startup_flash_handshake_policy", json!("fw06-read-v1"));
    for image in [s.base.clone(), [&s.base[..], b" ", PARK_MARK].concat()] {
        s.f.image(&image);
        let error = s.audit(&good).unwrap_err().to_string();
        assert!(
            error.contains("Flash read policy/artifact mismatch")
                || error.contains("Flash park policy/artifact mismatch")
        );
    }
    // The read policy still requires the HELLO record format.
    s.f.image(&[&replace(&s.base, FLASH_MARK, b"")[..], READ_MARK].concat());
    assert!(
        s.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("Flash handshake policy/artifact mismatch")
    );
    // The classifier needs the capture end for the clean tail.
    let image = [&s.base[..], b" ", READ_MARK].concat();
    s.f.image(&image);
    let rx: Vec<u8> = good.0.iter().flat_map(|c| c.1.clone()).collect();
    let arrival = |_: usize| Ok(t0());
    let error = startup(
        &rx,
        &s.f.manifest,
        &image,
        &s.f.identity,
        Some(&arrival),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("capture end time"));
}

/// The flash records after the LED probe at the given times (per line), under `version`.
fn run_lines(lines: &[&[u8]], version: u8, times: &[f64], end: Option<Time>) -> Report {
    let led = led_full(3);
    let mut rx = led[1..].concat();
    let mut marks = Vec::new();
    for line in lines {
        marks.push(rx.len());
        rx.extend_from_slice(line);
    }
    let arrival = |offset: usize| -> Result<Time> {
        let (_, t) = marks
            .iter()
            .zip(times)
            .rev()
            .find(|(m, _)| **m <= offset)
            .unwrap();
        Ok(at(*t))
    };
    classify_flash(&rx, &arrival, version, end).unwrap()
}

#[test]
fn earlier_policies_ignore_read_and_write_records() {
    let peer: &[u8] = b"PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n";
    let done: &[u8] = b"PLFLS1 done phase=8 fault=7 blocked=1\r\n";
    let park: &[u8] = b"PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n";
    for start in [READ_START_LINE, WRITE_START_LINE] {
        let hello = run_lines(
            &[HELLO, READY, start, peer, done],
            1,
            &[0.0, 1.0, 1.0, 11.0, 31.0],
            None,
        );
        assert_eq!(outcome(&hello), "pass", "{hello:?}");
        let parked = run_lines(&[HELLO, READY, park, start], 2, &[0.0, 1.0, 1.0, 1.0], None);
        assert_eq!(outcome(&parked), "pass", "{parked:?}");
        for report in [hello, parked] {
            assert!(
                !report
                    .keys()
                    .any(|k| k.starts_with("flash_read_") || k.starts_with("flash_write_"))
            );
        }
    }
    let mut lines: Vec<Vec<u8>> = vec![
        HELLO.into(),
        READY.into(),
        WRITE_START_LINE.into(),
        READ_START_LINE.into(),
    ];
    lines.extend(READ_BATCHES.iter().map(|n| ok_batch(*n)));
    lines.push(READ_DONE_LINE.into());
    let refs: Vec<&[u8]> = lines.iter().map(Vec::as_slice).collect();
    let read = run_lines(
        &refs,
        3,
        &[0.0, 1.0, 1.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        Some(at(40.0)),
    );
    assert_eq!(outcome(&read), "pass", "{read:?}");
    assert!(!read.keys().any(|k| k.starts_with("flash_write_")));
}

const WRITE_START_LINE: &[u8] = b"PLWRS1 start marker=00000000 watchdog=pass installed=1\r\n";

fn preimage(ident: &str, blank: u64) -> Vec<u8> {
    format!("PLWRS1 preimage id={ident} blank={blank}\r\n").into_bytes()
}

fn cycle(n: u64, erase: u64, blank: u64, program: u64, errors: &str) -> Vec<u8> {
    format!("PLWRS1 cycle n={n} erase_ticks={erase} blank={blank} program_max={program} errors={errors}\r\n").into_bytes()
}

fn ok_cycle(n: u64) -> Vec<u8> {
    cycle(n, 1500, 1024, 40, "0")
}

fn restore(erase: u64, blank: u64) -> Vec<u8> {
    format!("PLWRS1 restore erase_ticks={erase} blank={blank}\r\n").into_bytes()
}

#[allow(clippy::too_many_arguments)]
fn write_done(
    cycles: u64,
    leases: u64,
    phase: u64,
    fault: u64,
    park: u64,
    release: u64,
    frozen: u64,
) -> Vec<u8> {
    format!(
        "PLWRS1 done cycles={cycles} leases={leases} phase={phase} fault={fault} park_max={park} release_max={release} frozen={frozen}\r\n"
    )
    .into_bytes()
}

fn ok_done() -> Vec<u8> {
    write_done(3, 53, 1, 0, 6, 2, 53)
}

fn write_capture(identity: &str, write_seconds: f64, tail: f64, ready_delay: f64) -> (Chunks, f64) {
    let mut out = stage_prefix(identity, ready_delay);
    let t = out.last().unwrap().0 + 0.05;
    out.push((t, WRITE_START_LINE.to_vec(), "start".into()));
    out.push((t + 0.1, preimage("c84017", 1024), "preimage".into()));
    for n in WRITE_CYCLES {
        out.push((
            t + write_seconds * n as f64 / 5.0,
            ok_cycle(n),
            format!("cycle{n}"),
        ));
    }
    out.push((
        t + write_seconds * 4.0 / 5.0,
        restore(1600, 1024),
        "restore".into(),
    ));
    out.push((t + write_seconds, ok_done(), "done".into()));
    (out, t + write_seconds + tail)
}

#[test]
fn single_boot_write_passes_with_fixed_fields() {
    let s = Stage::new("fw06-write-v1", WRITE_MARK);
    let id = s.f.identity.clone();
    let report = s.audit(&write_capture(&id, 10.0, 30.0, 0.3)).unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(report["flash_handshake_verified"], true);
    assert_eq!(report["flash_write_verified"], true);
    assert_eq!(
        (
            &report["flash_write_id"],
            &report["flash_write_preimage_blank"]
        ),
        (&json!("c84017"), &json!(1024))
    );
    let cycles: Vec<Value> = (1..=3)
        .map(
            |n| json!({"n": n, "erase_ticks": 1500, "blank": 1024, "program_max": 40, "errors": 0}),
        )
        .collect();
    assert_eq!(report["flash_write_cycles"], json!(cycles));
    assert_eq!(
        (
            &report["flash_write_restore_erase_ticks"],
            &report["flash_write_restore_blank"]
        ),
        (&json!(1600), &json!(1024))
    );
    assert_eq!(
        (&report["flash_write_leases"], &report["flash_write_frozen"]),
        (&json!(53), &json!(53))
    );
    assert_eq!(
        (
            &report["flash_write_park_max"],
            &report["flash_write_release_max"]
        ),
        (&json!(6), &json!(2))
    );
    assert_eq!(report["flash_write_erase_ms_max"].as_f64(), Some(48.8));
    assert!((report["flash_write_seconds"].as_f64().unwrap() - 10.0).abs() < 1e-3);
    assert!((report["flash_write_tail_seconds"].as_f64().unwrap() - 30.0).abs() < 1e-3);
    assert!(!report.keys().any(|k| k.starts_with("flash_read_")));
    assert!(!json!(report).to_string().contains("PLWRS1"));
    // One foreign line, bounds at the limits and unrelated output after done still pass.
    let (chunks, end) = write_capture(&id, 60.0, 30.0, 0.3);
    let mut chunks: Chunks = chunks
        .into_iter()
        .map(|(t, d, k)| {
            let d = match k.as_str() {
                "cycle2" => cycle(2, 16384, 1024, 1638, "0"),
                "restore" => restore(16384, 1024),
                "done" => write_done(3, 53, 1, 0, 1310, 1310, 53),
                _ => d,
            };
            (t, d, k)
        })
        .collect();
    chunks.push((17.3, b"PLFLS1 foreign\r\n".to_vec(), "foreign".into()));
    chunks.push((end - 1.0, b"unrelated log\r\n".to_vec(), "tail".into()));
    let report = s.audit(&(chunks, end)).unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(report["flash_write_erase_ms_max"].as_f64(), Some(500.0));
}

#[test]
fn write_failures_have_distinct_outcomes() {
    let s = Stage::new("fw06-write-v1", WRITE_MARK);
    let id = s.f.identity.clone();
    let (good, end) = write_capture(&id, 10.0, 30.0, 0.3);
    let e = Edits { good, end };
    let moved = |label: &str, before: &str| e.retime(label, e.at(before) - 0.001);
    let cases: Vec<((Chunks, f64), &str)> = vec![
        (
            e.drop(&[
                "start", "preimage", "cycle1", "cycle2", "cycle3", "restore", "done",
            ]),
            "flash_write_missing",
        ),
        (e.drop(&["start"]), "flash_write_missing"),
        (e.drop(&["preimage"]), "flash_write_missing"),
        (e.drop(&["restore"]), "flash_write_missing"),
        (e.drop(&["done"]), "flash_write_missing"),
        (
            e.add("cycle1", b"PLWRS1 untrusted\r\n", 0.001),
            "flash_write_malformed",
        ),
        (
            e.edit("preimage", b"id=c84017", b"id=C84017"),
            "flash_write_malformed",
        ),
        (
            e.edit("cycle2", b"errors=0", b"errors=-1"),
            "flash_write_malformed",
        ),
        (
            e.add("start", WRITE_START_LINE, 0.001),
            "flash_write_malformed",
        ),
        (
            e.add("preimage", &preimage("c84017", 1024), 0.001),
            "flash_write_malformed",
        ),
        (
            e.add("restore", &restore(1600, 1024), 0.001),
            "flash_write_malformed",
        ),
        (e.add("done", &ok_done(), 0.001), "flash_write_malformed"),
        (
            e.add("cycle1", &ok_cycle(1), 0.001),
            "flash_write_malformed",
        ),
        (
            e.add(
                "ready",
                b"PLPRK1 start mode=0 marker=00000000 watchdog=pass\r\n",
                0.001,
            ),
            "flash_write_malformed",
        ),
        (
            e.add("ready", READ_START_LINE, 0.001),
            "flash_write_malformed",
        ),
        (
            e.add("ready", b"PLWRS1 skip marker=00000001\r\n", 0.001),
            "flash_write_failed",
        ),
        (
            e.add("cycle2", b"PLFLS1 error timeout ms=2500\r\n", 0.001),
            "flash_write_failed",
        ),
        (
            e.add("done", b"PLFLS1 error clock\r\n", 1.0),
            "flash_write_failed",
        ),
        (
            e.edit("start", b"marker=00000000", b"marker=00000001"),
            "flash_write_failed",
        ),
        (
            e.edit("start", b"watchdog=pass", b"watchdog=fail"),
            "flash_write_failed",
        ),
        (
            e.edit("start", b"installed=1", b"installed=0"),
            "flash_write_failed",
        ),
        (
            e.swap("preimage", preimage("c84017", 1023)),
            "flash_write_failed",
        ),
        (
            e.swap("preimage", preimage("ef4017", 1024)),
            "flash_write_failed",
        ),
        (e.drop(&["cycle3"]), "flash_write_failed"),
        (e.add("cycle3", &ok_cycle(4), 0.001), "flash_write_failed"),
        (
            e.swap("cycle2", cycle(2, 1500, 1024, 40, "1")),
            "flash_write_failed",
        ),
        (
            e.swap("cycle2", cycle(2, 1500, 1023, 40, "0")),
            "flash_write_failed",
        ),
        (
            e.swap("cycle1", cycle(1, 16385, 1024, 40, "0")),
            "flash_write_failed",
        ),
        (
            e.swap("cycle1", cycle(1, 0, 1024, 40, "0")),
            "flash_write_failed",
        ),
        (
            e.swap("cycle3", cycle(3, 1500, 1024, 1639, "0")),
            "flash_write_failed",
        ),
        (
            e.swap("cycle3", cycle(3, 1500, 1024, 0, "0")),
            "flash_write_failed",
        ),
        (e.swap("restore", restore(1600, 1000)), "flash_write_failed"),
        (
            e.swap("restore", restore(16385, 1024)),
            "flash_write_failed",
        ),
        (e.swap("restore", restore(0, 1024)), "flash_write_failed"),
        (
            e.swap("done", write_done(2, 53, 1, 0, 6, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 52, 1, 0, 6, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 8, 0, 6, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 7, 6, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 0, 6, 2, 52)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 0, 0, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 0, 1311, 2, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 0, 6, 0, 53)),
            "flash_write_failed",
        ),
        (
            e.swap("done", write_done(3, 53, 1, 0, 6, 1311, 53)),
            "flash_write_failed",
        ),
        (moved("start", "ready"), "flash_write_order"),
        (moved("preimage", "start"), "flash_write_order"),
        (moved("cycle1", "preimage"), "flash_write_order"),
        (moved("cycle1", "cycle3"), "flash_write_order"),
        (
            e.retime("cycle3", e.at("restore") + 0.001),
            "flash_write_order",
        ),
        (
            e.retime("restore", e.at("done") + 0.001),
            "flash_write_order",
        ),
        (
            e.add("done", b"PLFLS1 foreign\r\n", 1.0),
            "flash_write_order",
        ),
        (write_capture(&id, 60.1, 30.0, 0.3), "flash_write_timing"),
        (write_capture(&id, 10.0, 29.9, 0.3), "flash_write_timing"),
        (e.drop(&["ready"]), "flash_handshake_missing"),
        (e.drop(&["hello"]), "flash_handshake_missing"),
        (
            e.add(
                "cycle1",
                b"PLFLS1 peer peer=000000020000e5f6 reply=5 phase=8 fault=7 blocked=1\r\n",
                0.001,
            ),
            "flash_handshake_malformed",
        ),
        (
            e.add("done", b"PLFLS1 done phase=8 fault=7 blocked=1\r\n", 0.001),
            "flash_handshake_malformed",
        ),
        (
            e.add("hello", b"PLFLS1 error clock\r\n", 0.001),
            "flash_handshake_failed",
        ),
        (
            e.edit("hello", b"count=1", b"count=2"),
            "flash_handshake_failed",
        ),
        (
            e.edit("ready", b"inactive=1", b"inactive=0"),
            "flash_handshake_failed",
        ),
        (
            e.add("led", b"PLFLS1 foreign\r\n", 0.1),
            "flash_handshake_order",
        ),
        (
            write_capture(&id, 10.0, 30.0, 2.6),
            "flash_handshake_timing",
        ),
    ];
    for (received, expected) in cases {
        let report = s.audit(&received).unwrap();
        assert_eq!(outcome(&report), expected);
        assert_eq!(report["flash_write_verified"], false);
        assert!(!json!(report).to_string().contains("PLWRS1"));
    }
}

#[test]
fn write_policy_binding_both_ways() {
    let mut s = Stage::new("fw06-write-v1", WRITE_MARK);
    let good = write_capture(&s.f.identity.clone(), 10.0, 30.0, 0.3);
    for (policy, message) in [
        (
            json!("fw06-hello-v1"),
            "Flash write policy/artifact mismatch",
        ),
        (json!("fw06-park-v1"), "Flash park policy/artifact mismatch"),
        (json!("fw06-read-v1"), "Flash read policy/artifact mismatch"),
        (Value::Null, "Flash handshake policy/artifact mismatch"),
        (json!("fw06-write-v2"), "Unknown flash handshake policy"),
    ] {
        s.f.set("startup_flash_handshake_policy", policy);
        assert!(
            s.audit(&good).unwrap_err().to_string().contains(message),
            "{message}"
        );
    }
    s.f.set("startup_flash_handshake_policy", json!("fw06-write-v1"));
    for image in [
        s.base.clone(),
        [&s.base[..], b" ", PARK_MARK].concat(),
        [&s.base[..], b" ", READ_MARK].concat(),
    ] {
        s.f.image(&image);
        let error = s.audit(&good).unwrap_err().to_string();
        assert!(
            ["write", "park", "read"]
                .iter()
                .any(|k| error.contains(&format!("Flash {k} policy/artifact mismatch"))),
            "{error}"
        );
    }
    s.f.image(&[&replace(&s.base, FLASH_MARK, b"")[..], WRITE_MARK].concat());
    assert!(
        s.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("Flash handshake policy/artifact mismatch")
    );
    let image = [&s.base[..], b" ", WRITE_MARK].concat();
    s.f.image(&image);
    let rx: Vec<u8> = good.0.iter().flat_map(|c| c.1.clone()).collect();
    let arrival = |_: usize| Ok(t0());
    let error = startup(
        &rx,
        &s.f.manifest,
        &image,
        &s.f.identity,
        Some(&arrival),
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("capture end time"));
}
