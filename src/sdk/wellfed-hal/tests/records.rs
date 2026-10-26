//! The UART records the bench acts on, as the firmware formats them and the
//! host parser (`wellfed_core::record`) decodes them back. Each line is
//! built the way the first-boot app writes it, tag and all, after SDK noise,
//! so a renamed field or a changed word fails here rather than on the bench.

use std::fmt;

use wellfed_core::ota::SectorStep;
use wellfed_core::ota::install::{Refused, RefusedReason, Stop, StopStage};
use wellfed_core::ota::offer::OfferError;
use wellfed_core::record::{Confirm, Event, Record, Tag, Update, Watch};
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

/// An update record, under the tag the stage gives it, decodes to
/// `expected`.
fn update_decodes(record: update::Record, expected: Option<Event<'_>>) {
    let text = line(record.tag(), |f| update::describe(record, f));
    assert_eq!(read(&text).event(), expected, "{text}");
}

fn confirm_decodes(record: confirm::Record, expected: Option<Event<'_>>) {
    let text = line(Tag::Confirm.as_str(), |f| confirm::describe(record, f));
    assert_eq!(read(&text).event(), expected, "{text}");
}

#[test]
fn update_records_decode_to_what_they_say() {
    let cases = [
        (
            update::Record::Manifest {
                version: 33,
                bytes: 696_320,
            },
            Event::Update(Update::Manifest {
                version: Some(33),
                bytes: Some(696_320),
            }),
        ),
        (
            update::Record::Progress {
                sectors: 48,
                total: 170,
            },
            Event::Update(Update::Progress {
                sectors: 48,
                total: 170,
            }),
        ),
        (
            update::Record::Activated {
                version: 33,
                bytes: 696_320,
                sectors: 170,
            },
            Event::Update(Update::Activated { version: Some(33) }),
        ),
        (
            update::Record::Refused {
                refused: Refused::Offer(OfferError::Same),
            },
            Event::Update(Update::Refused {
                reason: Refused::Offer(OfferError::Same).reason(),
            }),
        ),
        (
            update::Record::GaveWay { version: 34 },
            Event::Watch(Watch::GiveWay { version: Some(34) }),
        ),
        (
            update::Record::Offer { version: 34 },
            Event::Watch(Watch::Offer { version: Some(34) }),
        ),
        (
            update::Record::GiveWayRefused,
            Event::Watch(Watch::GiveWayRefused),
        ),
    ];
    for (record, expected) in cases {
        update_decodes(record, Some(expected));
    }

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
        update_decodes(
            update::Record::Stopped { stop, write: None },
            Some(Event::Update(Update::Stopped {
                stage: stop.stage(),
            })),
        );
    }
}

#[test]
fn confirmation_records_decode_to_what_they_say() {
    confirm_decodes(
        confirm::Record::Confirmed {
            healthy_ms: 30_000,
            version: 12,
        },
        Some(Event::Confirm(Confirm::Confirmed {
            healthy_ms: Some(30_000),
        })),
    );
    confirm_decodes(
        confirm::Record::Already,
        Some(Event::Confirm(Confirm::Confirmed { healthy_ms: None })),
    );
    confirm_decodes(
        confirm::Record::Error {
            stage: confirm::ErrorStage::Write,
        },
        Some(Event::Confirm(Confirm::Error { stage: "write" })),
    );
}

/// The words the bench matches, as they were when these fields were plain
/// strings.
#[test]
fn update_words_are_unchanged() {
    assert_eq!(
        displayed(StopStage::ALL, StopStage::as_str),
        [
            "timeout",
            "cancelled",
            "order",
            "image",
            "write",
            "hash",
            "marker",
            "store"
        ]
    );
    assert_eq!(
        displayed(RefusedReason::ALL, RefusedReason::as_str),
        [
            "downgrade",
            "same",
            "key",
            "signature",
            "statement",
            "manifest",
            "store",
            "busy",
            "fallback",
            "pending",
            "failed",
            "attempt",
            "installing",
            "not-installing"
        ]
    );
    assert_eq!(
        displayed(SectorStep::ALL, SectorStep::as_str),
        ["erase", "program", "read", "readback"]
    );

    // The bench acts on these two: the offer is the running build, and an
    // earlier attempt reset this power cycle.
    assert_eq!(
        Refused::Offer(OfferError::Same).reason(),
        RefusedReason::Same
    );
    assert_eq!(Refused::Attempt.reason(), RefusedReason::Attempt);
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
    let dropped = line(Tag::Ble.as_str(), |f| {
        ble::describe(
            ble::Record::Drop {
                reason: ble::DropReason::Protocol,
            },
            f,
        )
    });
    let record = read(&dropped);
    assert_eq!(record.first, "drop");
    assert_eq!(record.kind(), Some(Tag::Ble));
    assert_eq!(record.field("reason"), Some("protocol"));

    let advertising = line(Tag::Ble.as_str(), |f| {
        ble::describe(ble::Record::Advertising { on: true }, f)
    });
    assert_eq!(read(&advertising).flag("on"), Some(true));

    let disconnect = line(Tag::Ble.as_str(), |f| {
        ble::describe(ble::Record::Disconnect { cause: 0x113 }, f)
    });
    assert_eq!(read(&disconnect).field("cause"), Some("0x113"));
}

#[test]
fn stage_and_reason_records_format_as_before() {
    let cases = [
        (
            text(|f| {
                update::describe(
                    update::Record::Stopped {
                        stop: Stop::Activate,
                        write: None,
                    },
                    f,
                )
            }),
            "error stage=marker",
        ),
        (
            text(|f| {
                update::describe(
                    update::Record::Stopped {
                        stop: Stop::Hash,
                        write: None,
                    },
                    f,
                )
            }),
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
