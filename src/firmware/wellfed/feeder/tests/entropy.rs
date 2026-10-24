//! The FW45 noise capture against the SDK fake: each source starts, streams
//! numbered records of `LINE` samples and ends with its health result, then
//! the ADC follows the jitter source; a source that stops giving samples is
//! given up, and the records format as the host parses them.

use wellfed_fw_feeder::entropy::{Capture, LINE, LINES, Record, SAMPLES, Source, describe};
use wellfed_sdk::hal::sdk::fake;

fn run(capture: &mut Capture) -> Vec<Record> {
    let mut records = Vec::new();
    while capture.step(|r| records.push(r)).is_some() {}
    records
}

fn text(record: Record) -> String {
    let mut out = String::new();
    describe(record, &mut out).unwrap();
    out
}

#[test]
fn both_sources_stream_in_order_and_report_health() {
    fake::reset_hardware();
    // A constant cycle count: a stuck source the health tests must catch.
    fake::with(|h| {
        h.ticks_per_read = 1;
        h.systick_reload = 9_999;
        h.systick_step = 1_234;
        h.supply_adc = 0x0123;
    });
    let records = run(&mut Capture::new());

    let raws = |source| {
        records
            .iter()
            .filter_map(|r| match r {
                Record::Raw { source: s, seq, .. } if *s == source => Some(*seq),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    for source in [Source::Jitter, Source::Adc] {
        assert_eq!(raws(source), (0..LINES).collect::<Vec<_>>(), "{source}");
    }

    let summaries: Vec<_> = records
        .iter()
        .filter(|r| !matches!(r, Record::Raw { .. }))
        .copied()
        .collect();
    assert!(matches!(summaries[..], [
        Record::Start { source: Source::Jitter },
        Record::Done { source: Source::Jitter, samples: s1, missed: 1, failed_at: Some(_) },
        Record::Start { source: Source::Adc },
        Record::Done { source: Source::Adc, samples: s2, missed: 0, failed_at: Some(_) },
        Record::Finished,
    ] if s1 == SAMPLES && s2 == SAMPLES));
    // The jitter source's one miss is its first call, which only reads.

    let Some(Record::Raw { bytes, .. }) = records.get(1) else {
        panic!("the first raw record follows the start");
    };
    assert!(bytes.iter().all(|&b| b == 1_234u32.to_le_bytes()[0]));
}

#[test]
fn a_stopped_timer_gives_the_jitter_source_up() {
    fake::reset_hardware();
    let mut capture = Capture::new();
    let mut records = Vec::new();
    capture.step(|r| records.push(r));
    capture.step(|r| records.push(r));
    assert_eq!(
        records,
        [
            Record::Start {
                source: Source::Jitter
            },
            Record::GaveUp {
                source: Source::Jitter,
                samples: 0
            },
        ]
    );
}

#[test]
fn records_format_for_the_host() {
    let raw = Record::Raw {
        source: Source::Adc,
        seq: 7,
        bytes: [0xff; LINE],
    };
    let line = text(raw);
    assert!(line.starts_with("raw src=adc seq=7 b64=////"), "{line}");

    // The longest line still fits the firmware's record buffer, with its NUL.
    let longest = text(Record::Raw {
        source: Source::Jitter,
        seq: LINES - 1,
        bytes: [0; LINE],
    });
    let mut report = wellfed_sdk::hal::startup::Report::new();
    assert!(
        report.record(format_args!("PLRNG1 {longest}\r\n")).is_ok(),
        "{longest}"
    );

    let done = Record::Done {
        source: Source::Jitter,
        samples: 10,
        missed: 2,
        failed_at: None,
    };
    assert_eq!(
        text(done),
        "done src=jitter samples=10 missed=2 health=passed"
    );
}
