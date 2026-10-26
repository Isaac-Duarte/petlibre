//! The QA auditor's grading of `PLQA1` records.

use serde_json::json;

use super::*;
use crate::Error;
use crate::test_support::*;

/// A whole run's records with these verdicts, interleaved with other
/// tasks' lines as the firmware logs them.
fn run(verdicts: [&str; 5]) -> Vec<u8> {
    let mut out = String::from("PLOPT1 start created=1 plan=2\r\n");
    for (check, verdict) in Check::ALL.iter().map(|c| c.as_str()).zip(verdicts) {
        out += &format!("PLQA1 start stage={check}\r\n");
        if check == "buttons" {
            out += "PLQA1 prompt press=reset\r\nPLBTN1 change n=0 ms=10 from=f0 to=e0 edges=1 bounce_ms=0 settled=1 stopped=1\r\n";
            let status = if verdict == "pass" {
                "status=pass"
            } else {
                "status=fail reason=timeout"
            };
            out += &format!("PLQA1 item stage=buttons press=reset {status}\r\n");
        }
        out += &format!("PLQA1 check stage={check} status={verdict}\r\n");
    }
    let status = if verdicts.iter().all(|v| *v == "pass") {
        "pass"
    } else {
        "fail"
    };
    out += &format!(
        "PLQA1 result status={status} optics={} power={} buttons={} indicators={} speaker={}\r\n",
        verdicts[0], verdicts[1], verdicts[2], verdicts[3], verdicts[4]
    );
    out.into_bytes()
}

const ALL_PASS: [&str; 5] = ["pass"; 5];

#[test]
fn a_passing_run_passes() {
    let (outcome, fields) = classify(&run(ALL_PASS));

    assert_eq!(outcome, QaOutcome::Pass);
    assert_eq!(fields.qa_prompts, 1);
    assert!(fields.qa_failed_items.is_empty());
    assert!(
        fields
            .qa_checks
            .iter()
            .all(|(_, v)| *v == Some(Verdict::Pass))
    );
}

#[test]
fn the_consoles_line_framing_is_read() {
    // As captured on the feeder (fw39-qa-02-guided): "\n\r" between lines.
    let wire = replace(&run(ALL_PASS), b"\r\n", b"\r\n\r");

    let (outcome, fields) = classify(&wire);

    assert_eq!(outcome, QaOutcome::Pass, "{:?}", fields.qa_sequence_fault);
}

#[test]
fn a_failed_check_fails_with_its_items() {
    let (outcome, fields) = classify(&run(["fail", "pass", "fail", "pass", "pass"]));

    assert_eq!(outcome, QaOutcome::Fail);
    assert_eq!(
        fields.qa_failed_items,
        vec![FailedItem {
            stage: Check::Buttons,
            item: "press=reset".into(),
            reason: "timeout".into(),
        }]
    );
    assert_eq!(fields.qa_checks[0], (Check::Optics, Some(Verdict::Fail)));
}

#[test]
fn broken_sequences_are_neither_pass_nor_fail() {
    let good = run(ALL_PASS);
    let cases: [(Vec<u8>, &str); 7] = [
        (
            replace(&good, b"PLQA1 result", b"PLQA1 resul"),
            "an unreadable PLQA1 record",
        ),
        (
            good.split_inclusive(|b| *b == b'\n')
                .filter(|l| !l.starts_with(b"PLQA1 result"))
                .flatten()
                .copied()
                .collect(),
            "no result",
        ),
        (
            replace(&good, b"start stage=power", b"start stage=speaker"),
            "checks started out of order",
        ),
        (
            replace(&good, b"PLQA1 check stage=power status=pass\r\n", b""),
            "a check never reported a verdict",
        ),
        (
            replace(&good, b"indicators=pass", b"indicators=fail"),
            "the result disagrees with the verdicts",
        ),
        (
            [&good[..], b"PLQA1 start stage=optics\r\n"].concat(),
            "records after the result",
        ),
        (
            replace(
                &good,
                b"PLQA1 check stage=optics",
                b"xxPLQA1 check stage=optics",
            ),
            "a PLQA1 record does not start its line",
        ),
    ];

    for (rx, fault) in cases {
        let (outcome, fields) = classify(&rx);
        assert_eq!(outcome, QaOutcome::Sequence, "{fault}");
        assert_eq!(fields.qa_sequence_fault, Some(fault));
    }
}

#[test]
fn a_status_that_contradicts_the_verdicts_is_refused() {
    let lying = replace(
        &run(["fail", "pass", "pass", "pass", "pass"]),
        b"result status=fail",
        b"result status=pass",
    );

    let (outcome, fields) = classify(&lying);

    assert_eq!(outcome, QaOutcome::Sequence);
    assert_eq!(
        fields.qa_sequence_fault,
        Some("the result status disagrees with the verdicts")
    );
}

#[test]
fn only_a_qa_build_is_audited() {
    let inputs = json!({"source": "f".repeat(64)});
    let mut f = Fixture::new(inputs.clone());
    // The firmware name is part of the source identity.
    let identity = crate::digest(
        &wellfed_formats::canonical::canonical(
            &json!({"device": "plaf103", "profile": "firstboot", "inputs": inputs, "firmware": "optics"}),
        )
        .unwrap(),
    );
    f.set("firmware", json!("optics"));
    f.set("startup_identity", json!(identity));
    let image = format!("PLFW1 start build={identity} ").into_bytes();
    f.image(&image);
    f.capture(&[(0.0, run(ALL_PASS))], 1.0);

    let p = f.path();
    let error = verify(p, &p.join("manifest.json"), &p.join("image.bin")).unwrap_err();
    assert!(
        matches!(&error, Error::QaBuildRequired { firmware: Some(f) } if f == "optics"),
        "{error:?}"
    );
}

#[test]
fn outcomes_print_as_report_strings() {
    assert_eq!(QaOutcome::Sequence.to_string(), "qa_sequence");
    assert_eq!(QaOutcome::Fail.to_string(), "qa_fail");
    assert_eq!(
        report(QaOutcome::Startup(Outcome::Pass)),
        json!("startup_pass")
    );
}
