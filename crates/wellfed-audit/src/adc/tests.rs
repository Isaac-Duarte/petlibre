//! The FW14 ADC auditor's checks.

use serde_json::json;

use super::*;
use crate::startup::{ADC_MARK, PASS};
use crate::test_support::*;

const POWER: &[u8] = b"PLPWR1 before_func=1201019a before_pad=00000000 after_func=1201019a after_pad=00000355 ext_a=e1c46e80,e1c46e00,e1c46e00,e1c46e00\n\r";
const CLOCK: &[u8] = b"PLCLK1 function_before=001115f7 clock_before=00131557 function_after=001115f7 clock_after=00131557 status=pass\r\n";
const OUTPUTS: &[u8] =
    b"PLFW1 outputs pa=00006000 pb=00000000 dira=0000e000 dirb=00000008 status=pass\r\n";
const ADC_CLOCK: &[u8] = b"PLADC1 clock function_before=000115f7 clock_before=00031457 function_after=000115f7 clock_after=00031557 status=pass\r\n";
const OPEN: &[u8] = b"PLADC1 open outputs=1 efuse=ffffffff offset=2480 gain=12050 default_offset=1 default_gain=1 status=pass\r\n";
const STAGES: [&str; 9] = [
    "startup", "PA0", "PA0", "PA2", "PA2", "PA4", "PA4", "PB22", "PB22",
];

fn boot(identity: &str) -> Vec<u8> {
    let routing: Vec<u8> = STAGES
        .iter()
        .flat_map(|s| {
            format!("PLIO1 stage={s} groups=0000ffff aon=00020003 control_a=00000000 control_b=00000000 share_pad=80000355 ext_a=00006001\r\n")
                .into_bytes()
        })
        .collect();
    let start = format!(
        "PLFW1 start build={identity} bank=1 reset=00000000 mmu=00000003,0c000000,03e7a000\r\n"
    );
    [
        POWER,
        CLOCK,
        &routing,
        OUTPUTS,
        b"Wellfed local firmware: Rust bringup\r\n",
        start.as_bytes(),
        b"Synthetic core diagnostic: PASS\r\nSynthetic diagnostic only; feeder backend stays unopened.\r\n",
        PASS,
    ]
    .concat()
}

#[derive(Clone, Copy)]
struct Sample {
    current: (i64, i64, i64),
    battery: (i64, i64, i64),
    supply: Option<(i64, i64, i64)>,
    ticks: i64,
    dc: i64,
    stopped: i64,
    scaled: Option<i64>,
}

const GOOD: Sample = Sample {
    current: (290, 310, 300),
    battery: (2590, 2610, 2600),
    supply: None,
    ticks: 420,
    dc: 1,
    stopped: 1,
    scaled: None,
};

fn sample(n: i64, s: Sample) -> Vec<u8> {
    let channel = |(low, high, mean): (i64, i64, i64)| {
        format!("{low},{high},{mean},{}", scaled(mean, 2480, 12050))
    };
    let supply = s
        .supply
        .map_or(String::new(), |c| format!(" supply={}", channel(c)));
    let line = format!(
        "PLADC1 sample n={n} ticks={} food=0 outlet=1 dc={} stopped={} current={} battery={}{supply}\r\n",
        s.ticks,
        s.dc,
        s.stopped,
        channel(s.current),
        channel(s.battery)
    )
    .into_bytes();
    match s.scaled {
        None => line,
        Some(value) => replace(
            &line,
            format!(",{} battery", scaled(s.current.2, 2480, 12050)).as_bytes(),
            format!(",{value} battery").as_bytes(),
        ),
    }
}

struct Capture {
    samples: i64,
    period: f64,
    clock: Vec<u8>,
    opened: Vec<u8>,
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            samples: 40,
            period: 1.0,
            clock: ADC_CLOCK.to_vec(),
            opened: OPEN.to_vec(),
        }
    }
}

/// Timed receive chunks and the capture end.
type Received = (Vec<(f64, Vec<u8>)>, f64);

fn capture(identity: &str, c: Capture, line: &dyn Fn(i64) -> Vec<u8>) -> Received {
    let led = led_full(3).concat();
    let mut out = vec![
        (0.0, boot(identity)),
        (5.0, led),
        (5.05, [&c.clock[..], &c.opened].concat()),
    ];
    for n in 0..c.samples {
        out.push((6.05 + n as f64 * c.period, line(n)));
    }
    (out, 6.05 + c.samples as f64 * c.period + 2.0)
}

fn good_line(n: i64) -> Vec<u8> {
    sample(n, GOOD)
}

struct Adc {
    f: Fixture,
    base: Vec<u8>,
}

impl Adc {
    fn new() -> Self {
        let mut f = Fixture::new(json!({"source": "f".repeat(64)}));
        for (key, value) in [
            ("startup_output_policy", json!("stop-only")),
            ("startup_led_policy", json!("bounded-probe-v3")),
            ("startup_gpio_routing_policy", json!("read-only-v2")),
            ("startup_shared_pad_policy", json!("before-km4-v3")),
            ("startup_gpio_clock_policy", json!("explicit-enable-v1")),
            ("startup_flash_handshake_policy", Value::Null),
            ("startup_adc_policy", json!("fw14-adc-v1")),
        ] {
            f.set(key, value);
        }
        let base = [
            format!(
                "PLFW1 start build={} PLFW1 outputs pa= PLCLK1 function_before= PLLED1 begin output_only= highs= PLIO1 stage= share_pad= PLPWR1 before_func=",
                f.identity
            )
            .as_bytes(),
            b"after_pad=%08x ext_a=%08x,%08x,%08x,%08x",
        ]
        .concat();
        f.image(&[&base[..], b" ", ADC_MARK].concat());
        Self { f, base }
    }

    fn audit(&self, (chunks, end): &Received) -> Result<Report> {
        self.f.capture(chunks, *end);
        let p = self.f.path();
        verify(p, &p.join("manifest.json"), &p.join("image.bin"))
    }
}

#[test]
fn stock_scaling_and_calibration_defaults() {
    assert_eq!(scaled(240, 2480, 12050), 0);
    assert_eq!(scaled(3918, 2480, 12050), (39180 - 2480) * 1000 / 12050);
    assert_eq!(calibration("ffffffff"), Some([2480, 12050, 1, 1]));
    assert_eq!(calibration("a8090f2f"), Some([2472, 12047, 0, 0]));
    assert_eq!(calibration("zz"), None);
}

#[test]
fn read_only_capture_passes_with_fixed_fields() {
    let a = Adc::new();
    let id = a.f.identity.clone();
    let report = a
        .audit(&capture(&id, Capture::default(), &good_line))
        .unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(report["startup"]["outcome"], "pass");
    assert_eq!(report["adc_enabled_before"], false);
    assert_eq!(
        report["adc_calibration"],
        json!({"offset": 2480, "gain": 12050, "default_offset": 1, "default_gain": 1})
    );
    assert_eq!(report["adc_samples"], 40);
    assert_eq!(report["adc_sample_seconds"].as_f64(), Some(1.0));
    assert_eq!(report["adc_ticks_max"], 420);
    assert_eq!(
        (
            &report["adc_food_levels"],
            &report["adc_outlet_levels"],
            &report["adc_dc_levels"]
        ),
        (&json!([0]), &json!([1]), &json!([1]))
    );
    assert_eq!(
        report["adc_current"],
        json!({"min": 290, "max": 310, "mean_min": 300, "mean_max": 300, "spread_max": 20, "scaled_min": 43, "scaled_max": 43})
    );
    assert_eq!(
        report["adc_battery"]["scaled_max"],
        (26000 - 2480) * 1000 / 12050
    );
    assert!(
        report["limit"]
            .as_str()
            .unwrap()
            .contains("not established")
    );
    assert!(!json!(report).to_string().contains("PLADC1"));
    // An ADC already enabled by startup is preserved and still passes.
    let on = replace(
        ADC_CLOCK,
        b"clock_before=00031457",
        b"clock_before=00031557",
    );
    let report = a
        .audit(&capture(
            &id,
            Capture {
                clock: on,
                ..Capture::default()
            },
            &good_line,
        ))
        .unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(report["adc_enabled_before"], true);
    // Real efuse calibration decodes little endian.
    let opened = b"PLADC1 open outputs=1 efuse=a8090f2f offset=2472 gain=12047 default_offset=0 default_gain=0 status=pass\r\n".to_vec();
    let line = |n: i64| {
        let s = replace(
            &good_line(n),
            b",300,43 ",
            format!(",300,{} ", scaled(300, 2472, 12047)).as_bytes(),
        );
        replace(
            &s,
            format!(",2600,{}\r", scaled(2600, 2480, 12050)).as_bytes(),
            format!(",2600,{}\r", scaled(2600, 2472, 12047)).as_bytes(),
        )
    };
    let report = a
        .audit(&capture(
            &id,
            Capture {
                opened,
                ..Capture::default()
            },
            &line,
        ))
        .unwrap();
    assert_eq!(outcome(&report), "pass");
}

#[test]
fn failures_have_distinct_outcomes() {
    let a = Adc::new();
    let id = a.f.identity.clone();
    let with_clock = |old: &[u8], new: &[u8]| Capture {
        clock: replace(ADC_CLOCK, old, new),
        ..Capture::default()
    };
    let with_open = |old: &[u8], new: &[u8]| Capture {
        opened: replace(OPEN, old, new),
        ..Capture::default()
    };
    let at7 = |s: Sample| move |n: i64| if n == 7 { sample(n, s) } else { good_line(n) };
    let cases: Vec<(Received, &str)> = vec![
        (
            capture(&id, with_clock(b"status=pass", b"status=fail"), &good_line),
            "adc_clock",
        ),
        (
            capture(
                &id,
                with_clock(b"clock_after=00031557", b"clock_after=00031457"),
                &good_line,
            ),
            "adc_clock",
        ),
        (
            capture(
                &id,
                with_clock(b"function_after=000115f7", b"function_after=000117f7"),
                &good_line,
            ),
            "adc_clock",
        ),
        (
            capture(&id, with_open(b"outputs=1", b"outputs=0"), &good_line),
            "adc_open",
        ),
        (
            capture(&id, with_open(b"status=pass", b"status=fail"), &good_line),
            "adc_open",
        ),
        (
            capture(&id, with_open(b"efuse=ffffffff", b"efuse=none"), &good_line),
            "adc_open",
        ),
        (
            capture(&id, with_open(b"gain=12050", b"gain=12051"), &good_line),
            "adc_open",
        ),
        (
            capture(
                &id,
                with_open(b"default_gain=1", b"default_gain=0"),
                &good_line,
            ),
            "adc_open",
        ),
        (
            capture(
                &id,
                Capture {
                    samples: 29,
                    ..Capture::default()
                },
                &good_line,
            ),
            "adc_samples",
        ),
        (
            capture(
                &id,
                Capture {
                    samples: 0,
                    ..Capture::default()
                },
                &good_line,
            ),
            "adc_samples",
        ),
        (
            capture(&id, Capture::default(), &|n| {
                good_line(n + i64::from(n > 5))
            }),
            "adc_samples",
        ),
        (
            capture(&id, Capture::default(), &at7(Sample { stopped: 0, ..GOOD })),
            "adc_samples",
        ),
        (
            capture(&id, Capture::default(), &at7(Sample { ticks: 0, ..GOOD })),
            "adc_samples",
        ),
        (
            capture(
                &id,
                Capture::default(),
                &at7(Sample {
                    ticks: 3278,
                    ..GOOD
                }),
            ),
            "adc_samples",
        ),
        (
            capture(
                &id,
                Capture::default(),
                &at7(Sample {
                    scaled: Some(44),
                    ..GOOD
                }),
            ),
            "adc_values",
        ),
        (
            capture(
                &id,
                Capture::default(),
                &at7(Sample {
                    current: (290, 299, 300),
                    ..GOOD
                }),
            ),
            "adc_values",
        ),
        (
            capture(
                &id,
                Capture::default(),
                &at7(Sample {
                    battery: (2590, 4096, 2600),
                    ..GOOD
                }),
            ),
            "adc_values",
        ),
        (
            capture(
                &id,
                Capture {
                    period: 0.5,
                    ..Capture::default()
                },
                &good_line,
            ),
            "adc_timing",
        ),
        (
            capture(
                &id,
                Capture {
                    period: 2.0,
                    ..Capture::default()
                },
                &good_line,
            ),
            "adc_timing",
        ),
        (
            capture(&id, Capture::default(), &|n| {
                if n == 7 {
                    b"PLADC1 error n=7 ticks=3277 stopped=1\r\n".to_vec()
                } else {
                    good_line(n)
                }
            }),
            "adc_sequence",
        ),
        (
            capture(&id, Capture::default(), &|n| {
                let s = good_line(n);
                if n == 3 {
                    [
                        &s[..],
                        b"PLSET1 start marker=00000000 watchdog=pass installed=1\r\n",
                    ]
                    .concat()
                } else {
                    s
                }
            }),
            "adc_sequence",
        ),
        (
            capture(&id, Capture::default(), &|n| {
                if n == 3 {
                    replace(&good_line(n), b"food=0", b"food=2")
                } else {
                    good_line(n)
                }
            }),
            "adc_sequence",
        ),
        (
            capture(
                &id,
                Capture {
                    opened: vec![],
                    ..Capture::default()
                },
                &good_line,
            ),
            "adc_sequence",
        ),
        (
            capture(&id, Capture::default(), &|n| {
                if n == 3 {
                    [&good_line(n)[..], OPEN].concat()
                } else {
                    good_line(n)
                }
            }),
            "adc_sequence",
        ),
    ];
    for (index, (received, expected)) in cases.into_iter().enumerate() {
        let report = a.audit(&received).unwrap();
        assert_eq!(outcome(&report), expected, "case {index}");
        assert!(!json!(report).to_string().contains("PLADC1"));
    }
    // Records before the LED probe completes, or a second startup.
    let (chunks, end) = capture(&id, Capture::default(), &good_line);
    let early: Vec<(f64, Vec<u8>)> = chunks
        .iter()
        .map(|(t, d)| {
            (
                if find(d, b"PLADC1 clock").is_some() {
                    4.0
                } else {
                    *t
                },
                d.clone(),
            )
        })
        .collect();
    assert_eq!(outcome(&a.audit(&(early, end)).unwrap()), "adc_sequence");
    let mut again = chunks;
    again.push((end - 1.0, boot(&id)));
    assert_eq!(
        outcome(&a.audit(&(again, end)).unwrap()),
        "startup_repeated_startup"
    );
}

#[test]
fn supply_channel_policy_requires_distinct_channels() {
    let mut a = Adc::new();
    let id = a.f.identity.clone();
    a.f.set("startup_adc_policy", json!("fw14-adc-v2"));
    a.f.image(&[&a.base[..], b" ", ADC_MARK, b" supply="].concat());
    let battery = (230, 250, 240);
    let run = |a: &Adc, supply: (i64, i64, i64), battery: (i64, i64, i64)| {
        let line = move |i: i64| {
            sample(
                i,
                Sample {
                    current: battery,
                    battery,
                    supply: Some(supply),
                    ..GOOD
                },
            )
        };
        a.audit(&capture(&id, Capture::default(), &line))
    };
    let report = run(&a, (3480, 3500, 3490), battery).unwrap();
    assert_eq!(outcome(&report), "pass", "{report:?}");
    assert_eq!(
        (
            &report["adc_supply"]["mean_min"],
            &report["adc_supply"]["scaled_max"]
        ),
        (&json!(3490), &json!(scaled(3490, 2480, 12050)))
    );
    assert_eq!(
        outcome(&run(&a, (1130, 1150, 1139), battery).unwrap()),
        "adc_channels"
    );
    assert_eq!(
        outcome(&run(&a, (3480, 3500, 3490), (2580, 2600, 2591)).unwrap()),
        "adc_channels"
    );
    assert_eq!(
        outcome(&run(&a, (3480, 3500, 4000), battery).unwrap()),
        "adc_values"
    );
    // A sample without the supply channel, under v2, or with it, under v1.
    assert_eq!(
        outcome(
            &a.audit(&capture(&id, Capture::default(), &good_line))
                .unwrap()
        ),
        "adc_sequence"
    );
    a.f.set("startup_adc_policy", json!("fw14-adc-v1"));
    let error = run(&a, (3480, 3500, 3490), battery)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ADC supply policy/artifact mismatch"));
    a.f.image(&[&a.base[..], b" ", ADC_MARK].concat());
    assert_eq!(
        outcome(&run(&a, (3480, 3500, 3490), battery).unwrap()),
        "adc_sequence"
    );
}

#[test]
fn policy_binding_both_ways() {
    let mut a = Adc::new();
    let good = capture(&a.f.identity.clone(), Capture::default(), &good_line);
    a.f.set("startup_adc_policy", Value::Null);
    assert!(
        a.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("ADC policy required")
    );
    a.f.set("startup_adc_policy", json!("fw14-adc-v1"));
    let base = a.base.clone();
    a.f.image(&base);
    assert!(
        a.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("ADC policy/artifact mismatch")
    );
    a.f.set("startup_adc_policy", json!("fw14-adc-v3"));
    assert!(
        a.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("ADC policy required")
    );
    a.f.set("startup_adc_policy", json!("fw14-adc-v1"));
    a.f.set("startup_led_policy", Value::Null);
    a.f.image(
        &[
            &replace(&base, b"PLLED1 begin output_only= highs= ", b"")[..],
            b" ",
            ADC_MARK,
        ]
        .concat(),
    );
    assert!(
        a.audit(&good)
            .unwrap_err()
            .to_string()
            .contains("ADC probe requires the LED probe policy")
    );
}

#[test]
fn records_classify_and_flag_unparsed_text() {
    let rx = b"PLADC1 error n=1 ticks=2 stopped=1\r\nPLADC1 junk\r\n";
    let (found, intact) = records(rx);
    assert_eq!(found.iter().map(|r| r.kind).collect::<String>(), "E");
    assert!(!intact);
}
