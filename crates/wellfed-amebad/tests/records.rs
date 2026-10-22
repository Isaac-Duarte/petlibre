//! The UART records the bench acts on, as the firmware formats them and the
//! host parser (`wellfed_core::record`) reads them back. Each line is built
//! the way the first-boot app writes it, tag and all, after SDK noise, so a
//! renamed field or a changed word fails here rather than on the bench.

use std::fmt;

use wellfed_amebad::{confirm, transport, updater, watch};
use wellfed_core::record::{Record, tag};

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
    let refused = line(tag::TRANSPORT, |f| {
        transport::describe(
            transport::Record::Request {
                kind: transport::RequestKind::Unauthenticated,
                ok: false,
            },
            f,
        )
    });
    assert!(read(&refused).is(tag::TRANSPORT, &[("op", "unauthenticated"), ("ok", "0")]));

    let ping = line(tag::TRANSPORT, |f| {
        transport::describe(
            transport::Record::Request {
                kind: transport::RequestKind::Ping,
                ok: true,
            },
            f,
        )
    });
    assert!(read(&ping).is(tag::TRANSPORT, &[("op", "ping"), ("ok", "1")]));

    let ready = line(tag::TRANSPORT, |f| {
        transport::describe(transport::Record::Ready { ms: 3 }, f)
    });
    assert_eq!(read(&ready).first, "ready");

    let closed = line(tag::TRANSPORT, |f| {
        transport::describe(transport::Record::Closed { reason: "timeout" }, f)
    });
    assert_eq!(read(&closed).first, "closed");
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
            },
            f,
        )
    });
    let progress = read(&progress);
    assert_eq!(progress.first, "progress");
    assert_eq!(progress.field("sectors"), Some("48/170"));

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

    for stage in ["hash", "range", "timeout"] {
        let error = line(tag::UPDATE, |f| {
            updater::describe(updater::Record::Error { stage }, f)
        });
        let error = read(&error);
        assert_eq!((error.first, error.field("stage")), ("error", Some(stage)));
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
