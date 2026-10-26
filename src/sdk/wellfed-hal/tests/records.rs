//! The UART records the bench acts on, as the firmware formats them and the
//! host parser (`wellfed_core::record`) reads them back. Each line is built
//! the way the first-boot app writes it, tag and all, after SDK noise, so a
//! renamed field or a changed word fails here rather than on the bench.

use std::fmt;

use wellfed_core::ota::install::Stop;
use wellfed_core::record::{Record, tag};
use wellfed_hal::ota::{confirm, update};
use wellfed_hal::{audio, ble};

struct Body<F>(F);

impl<F: Fn(&mut fmt::Formatter<'_>) -> fmt::Result> fmt::Display for Body<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (self.0)(f)
    }
}

fn line(tag: &str, body: impl Fn(&mut fmt::Formatter<'_>) -> fmt::Result) -> String {
    format!("[sdk] {tag} {}\r\n", Body(body))
}

fn read(line: &str) -> Record<'_> {
    Record::parse(line).unwrap_or_else(|| panic!("no record in {line:?}"))
}

/// An update record's line, under the tag the stage gives it.
fn update_line(record: update::Record) -> String {
    let tag = record.tag();
    line(tag, |f| update::describe(record, f))
}

#[test]
fn update_records_carry_what_the_ota_watch_reads() {
    let manifest = update_line(update::Record::Manifest {
        version: 33,
        bytes: 696_320,
    });
    let manifest = read(&manifest);
    assert!(manifest.is(tag::UPDATE, &[]));
    assert_eq!(manifest.first, "manifest");
    assert_eq!(manifest.number::<u32>("version"), Some(33));
    assert_eq!(manifest.number::<u32>("bytes"), Some(696_320));

    let progress = update_line(update::Record::Progress {
        sectors: 48,
        total: 170,
    });
    let progress = read(&progress);
    assert_eq!(progress.first, "progress");
    assert_eq!(progress.field("sectors"), Some("48/170"));

    let activated = update_line(update::Record::Activated {
        version: 33,
        bytes: 696_320,
        sectors: 170,
    });
    let activated = read(&activated);
    assert_eq!(activated.first, "activated");
    assert_eq!(activated.number::<u32>("version"), Some(33));

    for stop in [
        Stop::Timeout,
        Stop::Cancelled,
        Stop::Order,
        Stop::Marker,
        Stop::Write,
        Stop::Hash,
        Stop::Activate,
        Stop::Store,
    ] {
        let error = update_line(update::Record::Stopped { stop });
        let error = read(&error);
        assert!(error.is(tag::UPDATE, &[]));
        assert_eq!(error.first, "error");
        assert!(error.field("stage").is_some());
    }
}

#[test]
fn give_way_and_confirmation_records_carry_what_the_ota_watch_reads() {
    let gave_way = update_line(update::Record::GaveWay { version: 34 });
    let gave_way = read(&gave_way);
    assert!(gave_way.is(tag::WATCH, &[("give_way", "1")]));
    assert_eq!(gave_way.number::<u32>("version"), Some(34));

    let offer = update_line(update::Record::Offer { version: 34 });
    let offer = read(&offer);
    assert!(offer.is(tag::WATCH, &[("offer", "1")]));
    assert_eq!(offer.number::<u32>("version"), Some(34));

    let confirmed = line(tag::CONFIRM, |f| {
        confirm::describe(
            confirm::Record::Confirmed {
                healthy_ms: 30_000,
                version: 12,
            },
            f,
        )
    });
    let confirmed = read(&confirmed);
    assert!(confirmed.is(tag::CONFIRM, &[("confirmed", "1")]));
    assert_eq!(confirmed.number::<u32>("healthy_ms"), Some(30_000));
    assert_eq!(confirmed.number::<u32>("version"), Some(12));

    let already = line(tag::CONFIRM, |f| {
        confirm::describe(confirm::Record::Already, f)
    });
    let already = read(&already);
    assert!(already.is(tag::CONFIRM, &[("confirmed", "1")]));
    assert_eq!(already.field("healthy_ms"), None);
}

fn text(write: impl FnOnce(&mut String) -> fmt::Result) -> String {
    let mut out = String::new();
    write(&mut out).unwrap();
    out
}

fn words<T: Copy>(all: &[T], as_str: fn(T) -> &'static str) -> Vec<&'static str> {
    all.iter().map(|&value| as_str(value)).collect()
}

/// `named_enum!` types also print through `Display`, which `describe` uses.
fn displayed<T: fmt::Display + Copy>(
    all: &[T],
    as_str: fn(T) -> &'static str,
) -> Vec<&'static str> {
    for &value in all {
        assert_eq!(value.to_string(), as_str(value));
    }
    words(all, as_str)
}

/// The words the stage and reason fields carry on the UART, as they were
/// when these fields were plain strings: the host tools match them.
#[test]
fn stage_and_reason_words_are_unchanged() {
    assert_eq!(
        displayed(confirm::ErrorStage::ALL, confirm::ErrorStage::as_str),
        ["read", "write", "floor"]
    );
    assert_eq!(
        displayed(audio::ErrorStage::ALL, audio::ErrorStage::as_str),
        ["open"]
    );
    assert_eq!(
        displayed(ble::FailStage::ALL, ble::FailStage::as_str),
        ["task", "radio", "bte", "service", "stack", "timeout"]
    );
    assert_eq!(
        displayed(ble::DropReason::ALL, ble::DropReason::as_str),
        ["notify", "too_long", "timeout", "protocol", "done"]
    );
}

#[test]
fn ble_records_carry_what_a_bench_run_reads() {
    let dropped = line(tag::BLE, |f| {
        ble::describe(
            ble::Record::Drop {
                reason: ble::DropReason::Protocol,
            },
            f,
        )
    });
    let record = read(&dropped);
    assert_eq!(record.first, "drop");
    assert!(record.is(tag::BLE, &[("reason", "protocol")]));

    let advertising = line(tag::BLE, |f| {
        ble::describe(ble::Record::Advertising { on: true }, f)
    });
    assert!(read(&advertising).is(tag::BLE, &[("on", "1")]));

    let disconnect = line(tag::BLE, |f| {
        ble::describe(ble::Record::Disconnect { cause: 0x113 }, f)
    });
    assert!(read(&disconnect).is(tag::BLE, &[("cause", "0x113")]));
}

#[test]
fn stage_and_reason_records_format_as_before() {
    let cases = [
        (
            text(|f| {
                update::describe(
                    update::Record::Stopped {
                        stop: Stop::Activate,
                    },
                    f,
                )
            }),
            "error stage=marker",
        ),
        (
            text(|f| update::describe(update::Record::Stopped { stop: Stop::Hash }, f)),
            "error stage=hash",
        ),
        (
            text(|f| update::describe(update::Record::GiveWayRefused, f)),
            "error stage=give_way",
        ),
        (
            text(|f| {
                confirm::describe(
                    confirm::Record::Error {
                        stage: confirm::ErrorStage::Write,
                    },
                    f,
                )
            }),
            "error stage=write",
        ),
        (
            text(|f| {
                audio::describe(
                    audio::Record::Error {
                        stage: audio::ErrorStage::Open,
                    },
                    f,
                )
            }),
            "error stage=open",
        ),
    ];
    for (got, expected) in cases {
        assert_eq!(got, expected);
    }
}
