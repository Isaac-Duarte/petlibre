//! The UART records the bench acts on, as the firmware formats them and the
//! host parser (`wellfed_core::record`) reads them back. Each line is built
//! the way the first-boot app writes it, tag and all, after SDK noise, so a
//! renamed field or a changed word fails here rather than on the bench.

use std::fmt;

use wellfed_core::record::{Record, tag};
use wellfed_hal::ota::{confirm, updater, watch};
use wellfed_hal::{audio, ble, stream, transport};

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

#[test]
fn transport_records_carry_what_the_suite_asserts() {
    let ignored = line(tag::TRANSPORT, |f| {
        transport::describe(transport::Record::Ignored, f)
    });
    assert!(read(&ignored).is(tag::TRANSPORT, &[("ignored", "1")]));

    let ready = line(tag::TRANSPORT, |f| {
        transport::describe(transport::Record::Ready { ms: 3 }, f)
    });
    assert_eq!(read(&ready).first, "ready");

    let closed = line(tag::TRANSPORT, |f| {
        transport::describe(
            transport::Record::Closed {
                reason: transport::CloseReason::ReplyTimeout,
            },
            f,
        )
    });
    assert!(read(&closed).is(tag::TRANSPORT, &[("reason", "reply_timeout")]));
}

#[test]
fn updater_records_carry_what_the_ota_watch_reads() {
    let manifest = line(tag::UPDATE, |f| {
        updater::describe(
            updater::Record::Manifest {
                version: 33,
                bytes: 696_320,
            },
            f,
        )
    });
    let manifest = read(&manifest);
    assert_eq!(manifest.first, "manifest");
    assert_eq!(manifest.number::<u32>("version"), Some(33));
    assert_eq!(manifest.number::<u32>("bytes"), Some(696_320));

    let progress = line(tag::UPDATE, |f| {
        updater::describe(
            updater::Record::Progress {
                sectors: 48,
                total: 170,
                leases: 816,
                masked_max_ms: 61,
                masked_long: 0,
            },
            f,
        )
    });
    let progress = read(&progress);
    assert_eq!(progress.first, "progress");
    assert_eq!(progress.field("sectors"), Some("48/170"));
    assert_eq!(progress.number::<u32>("masked_max_ms"), Some(61));

    let activated = line(tag::UPDATE, |f| {
        updater::describe(
            updater::Record::Activated {
                version: 33,
                bytes: 696_320,
                sectors: 170,
            },
            f,
        )
    });
    let activated = read(&activated);
    assert_eq!(activated.first, "activated");
    assert_eq!(activated.number::<u32>("version"), Some(33));

    for &stage in updater::ErrorStage::ALL {
        let error = line(tag::UPDATE, |f| {
            updater::describe(updater::Record::Error { stage }, f)
        });
        let error = read(&error);
        assert_eq!(
            (error.first, error.field("stage")),
            ("error", Some(stage.as_str()))
        );
    }
}

#[test]
fn watch_and_confirmation_records_carry_what_the_ota_watch_reads() {
    let gave_way = line(tag::WATCH, |f| {
        watch::describe(watch::Record::GaveWay { version: 34 }, f)
    });
    let gave_way = read(&gave_way);
    assert!(gave_way.is(tag::WATCH, &[("give_way", "1")]));
    assert_eq!(gave_way.number::<u32>("version"), Some(34));

    let same = line(tag::WATCH, |f| {
        watch::describe(watch::Record::Found(watch::Found::Same), f)
    });
    assert!(read(&same).is(tag::WATCH, &[("offer", "1"), ("same", "1")]));

    let confirmed = line(tag::CONFIRM, |f| {
        confirm::describe(confirm::Record::Confirmed { healthy_ms: 30_000 }, f)
    });
    let confirmed = read(&confirmed);
    assert!(confirmed.is(tag::CONFIRM, &[("confirmed", "1")]));
    assert_eq!(confirmed.number::<u32>("healthy_ms"), Some(30_000));

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
        displayed(updater::ErrorStage::ALL, updater::ErrorStage::as_str),
        [
            "clock", "timeout", "topic", "range", "order", "hash", "write", "state", "key",
            "manifest", "marker"
        ]
    );
    assert_eq!(
        displayed(updater::CloseReason::ALL, updater::CloseReason::as_str),
        ["wifi", "closed", "protocol", "send", "timeout"]
    );
    assert_eq!(
        words(transport::CloseReason::ALL, transport::CloseReason::as_str),
        [
            "wifi",
            "closed",
            "send",
            "refused",
            "subscribe_refused",
            "protocol_too_long",
            "protocol_malformed",
            "protocol_unexpected",
            "out_of_order",
            "reply_timeout",
            "silence",
            "foreign_topic",
            "health_format"
        ]
    );
    assert_eq!(
        displayed(watch::ErrorStage::ALL, watch::ErrorStage::as_str),
        ["key", "give_way", "client_id", "topic"]
    );
    assert_eq!(
        displayed(confirm::ErrorStage::ALL, confirm::ErrorStage::as_str),
        ["read", "write"]
    );
    assert_eq!(
        displayed(audio::ErrorStage::ALL, audio::ErrorStage::as_str),
        ["open"]
    );
    assert_eq!(
        displayed(stream::ErrorStage::ALL, stream::ErrorStage::as_str),
        ["open"]
    );
    assert_eq!(
        displayed(stream::EndReason::ALL, stream::EndReason::as_str),
        ["closed", "stall"]
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
                updater::describe(
                    updater::Record::Error {
                        stage: updater::ErrorStage::Marker,
                    },
                    f,
                )
            }),
            "error stage=marker",
        ),
        (
            text(|f| {
                updater::describe(
                    updater::Record::Closed {
                        reason: updater::CloseReason::Protocol,
                    },
                    f,
                )
            }),
            "closed reason=protocol",
        ),
        (
            text(|f| {
                transport::describe(
                    transport::Record::Closed {
                        reason: transport::CloseReason::SubscribeRefused,
                    },
                    f,
                )
            }),
            "closed reason=subscribe_refused",
        ),
        (
            text(|f| {
                watch::describe(
                    watch::Record::Error {
                        stage: watch::ErrorStage::ClientId,
                    },
                    f,
                )
            }),
            "error stage=client_id",
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
        (
            text(|f| {
                stream::describe(
                    stream::Record::Done {
                        bytes: 4,
                        ms: 5,
                        underruns: 6,
                        reason: stream::EndReason::Stall,
                    },
                    f,
                )
            }),
            "done bytes=4 ms=5 underruns=6 reason=stall",
        ),
    ];
    for (got, expected) in cases {
        assert_eq!(got, expected);
    }
}
