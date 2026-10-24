//! FW08 transport link against fake SDK sockets and a scripted broker: it
//! waits for DNS, connects with an offline will, publishes non-retained
//! health, answers only the read-only request allowlist, and on any protocol,
//! socket or Wi-Fi failure closes with a reason and backs off.

use std::sync::{Mutex, MutexGuard};

use petlibre_hal::health::Boot;
use petlibre_hal::sdk::WifiEvent;
use petlibre_hal::sdk::fake::api::FAKE_TRANSPORT;
use petlibre_hal::sdk::fake::{self, TcpRecvScript};
use petlibre_hal::transport::*;
use petlibre_hal::{net_status, stopped};

/// `net_status` is process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const CONNACK: [u8; 4] = [0x20, 2, 0, 0];
const SUBACK: [u8; 5] = [0x90, 3, 0, 1, 0];
const PINGRESP: [u8; 2] = [0xd0, 0];

fn wifi(kind: u32, result: i32, value: u32) {
    net_status::observe(&WifiEvent {
        kind,
        result,
        value,
        attempt: 1,
        rssi: -20,
        ..WifiEvent::default()
    });
}

/// Joined with working DNS, outputs stopped, nothing scripted yet.
fn start() -> (MutexGuard<'static, ()>, Link) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    net_status::reset();
    fake::with(|h| h.interrupts = 1);
    assert!(stopped::initialize().verified());

    wifi(2, 0, 1); // scan found
    wifi(4, 2, 0x0a2a_6705); // DHCP assigned
    wifi(7, 0, 0x5db8_d822); // DNS ok

    let boot = Boot {
        bank: 1,
        reset_flags: 0,
        boot_count: 3,
    };
    (guard, Link::new(FAKE_TRANSPORT, boot))
}

fn rx(data: &[u8]) {
    fake::with(|h| h.tcp_rx.push_back(TcpRecvScript::Data(data.to_vec())));
}

fn publish(topic: &str, payload: &str) -> Vec<u8> {
    let mut out = vec![0x30, 0];
    out.extend((topic.len() as u16).to_be_bytes());
    out.extend(topic.as_bytes());
    out.extend(payload.as_bytes());
    out[1] = (out.len() - 2) as u8;
    out
}

/// (topic, payload) of every PUBLISH sent, in order.
fn published() -> Vec<(String, String)> {
    fake::with(|h| h.tcp_tx.clone())
        .into_iter()
        .filter(|(_, p)| p[0] & 0xf0 == 0x30)
        .map(|(_, p)| {
            let (length, at) = if p[1] & 0x80 != 0 {
                ((p[1] & 0x7f) as usize + 128 * p[2] as usize, 3)
            } else {
                (p[1] as usize, 2)
            };
            assert_eq!(p.len(), at + length);
            let n = u16::from_be_bytes([p[at], p[at + 1]]) as usize;
            let topic = String::from_utf8(p[at + 2..at + 2 + n].to_vec()).unwrap();
            (topic, String::from_utf8(p[at + 2 + n..].to_vec()).unwrap())
        })
        .collect()
}

fn run(link: &mut Link, steps: usize) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..steps {
        link.step(&mut |r| records.push(r));
    }
    records
}

fn pings() -> usize {
    fake::with(|h| h.tcp_tx.iter().filter(|(_, p)| p == &vec![0xc0, 0]).count())
}

/// Like `run`, with a broker that answers every ping.
fn run_answering_pings(link: &mut Link, steps: usize) -> Vec<Record> {
    let mut records = Vec::new();
    for _ in 0..steps {
        let before = pings();
        link.step(&mut |r| records.push(r));
        if pings() > before {
            rx(&PINGRESP);
        }
    }
    records
}

/// Connect and reach Ready; returns the records.
fn connect(link: &mut Link) -> Vec<Record> {
    fake::with(|h| h.tcp_connects.push_back(Some(7)));
    rx(&CONNACK);
    rx(&SUBACK);
    run(link, 3)
}

#[test]
fn waits_for_dns_then_connects_subscribes_and_publishes_health() {
    let (_guard, mut link) = start();
    wifi(5, 0, 1000); // a link loss: DNS no longer known good
    assert_eq!(link.step(&mut |_| panic!("no record before DNS")), IDLE_MS);
    assert!(fake::with(|h| h.tcp_tx.is_empty()));

    wifi(4, 2, 0x0a2a_6705);
    wifi(7, 0, 0x5db8_d822);

    let records = connect(&mut link);
    assert!(matches!(records[0], Record::Connect { n: 1, ok: true, .. }));
    assert!(matches!(records[1], Record::Ready { .. }));
    assert_eq!(
        records[2],
        Record::Publish {
            seq: 1,
            bytes: records_bytes(&records[2]),
            trigger: Trigger::Connect
        }
    );
    assert!(link.ready());

    let tx = fake::with(|h| h.tcp_tx.clone());
    let connect = &tx[0].1;
    assert_eq!(connect[0], 0x10);
    assert!(contains(connect, b"petlibre-fw-dev1") && contains(connect, FAKE_TRANSPORT.password));
    assert!(
        contains(connect, br#""online":false"#),
        "the will reports offline"
    );

    assert_eq!(tx[1].1[0], 0x82, "then SUBSCRIBE");
    assert!(contains(&tx[1].1, b"petlibre/1/dev1/request"));

    let sent = published();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "petlibre/1/dev1/health");
    let health = &sent[0].1;

    for field in [
        r#""online":true"#,
        r#""seq":1"#,
        r#""bank":1"#,
        r#""boot_count":3"#,
        r#""outputs":"stopped""#,
        r#""faults":[]"#,
        r#""rssi":-20"#,
        r#""connects":1"#,
        r#""last_error":null"#,
    ] {
        assert!(health.contains(field), "{field} in {health}");
    }

    assert!(
        tx.iter()
            .all(|(_, p)| p[0] & 0x01 == 0 || p[0] & 0xf0 != 0x30),
        "nothing retained"
    );
}

fn records_bytes(record: &Record) -> usize {
    match record {
        Record::Publish { bytes, .. } => *bytes,
        _ => 0,
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn publishes_periodically_and_on_wifi_changes_and_pings_when_idle() {
    let (_guard, mut link) = start();
    connect(&mut link);
    let base = fake::with(|h| h.uptime_ms);

    // Idle receives advance the fake clock by POLL_MS each.
    let steps = (petlibre_core::transport::PING_AFTER_MS / POLL_MS) as usize;
    run(&mut link, steps);
    assert_eq!(pings(), 1);
    rx(&PINGRESP);

    let records = run_answering_pings(
        &mut link,
        (petlibre_core::transport::HEALTH_PERIOD_MS / POLL_MS) as usize,
    );

    assert!(link.ready(), "{records:?}");
    assert!(
        records.iter().any(|r| matches!(
            r,
            Record::Publish {
                trigger: Trigger::Period,
                seq: 2,
                ..
            }
        )),
        "{records:?}"
    );
    assert!(fake::with(|h| h.uptime_ms) - base >= petlibre_core::transport::HEALTH_PERIOD_MS);

    // A DNS loss ends this generation: the transport closes rather than publishing.
    wifi(5, 1, 20_000);
    let records = run(&mut link, 1);
    assert_eq!(
        records,
        vec![Record::Closed {
            reason: CloseReason::Wifi
        }]
    );
    assert!(fake::with(|h| h.tcp_closed == vec![7]));
    assert!(
        !fake::with(|h| h.tcp_tx.iter().any(|(_, p)| p == &vec![0xe0, 0])),
        "never DISCONNECT: the will must fire"
    );
}

#[test]
fn requests_are_ignored_and_never_answered() {
    let (_guard, mut link) = start();
    connect(&mut link);

    for op in ["ping", "health", "settings", "lock"] {
        rx(&publish(
            "petlibre/1/dev1/request",
            &format!(r#"{{"id":"{ID}","op":"{op}"}}"#),
        ));
        assert_eq!(run(&mut link, 1), vec![Record::Ignored], "{op}");
    }

    let replies = published()
        .into_iter()
        .filter(|(t, _)| t.ends_with("/reply"))
        .count();
    assert_eq!(replies, 0, "requests go over the device API since FW47");
    assert!(link.ready(), "an ignored request keeps the session");
}

#[test]
fn failures_close_with_a_reason_and_back_off() {
    let (_guard, mut link) = start();

    // Refused connects wait 1 s, then 2 s.
    let records = run(&mut link, 1);
    assert!(matches!(
        records[0],
        Record::Connect {
            n: 1,
            ok: false,
            ..
        }
    ));
    assert_eq!(link.counters().last_error, Some("connect"));
    assert_eq!(run(&mut link, 1), vec![], "inside the backoff");

    fake::with(|h| h.uptime_ms += BACKOFF_MIN_MS * 2);
    assert!(matches!(
        run(&mut link, 1)[0],
        Record::Connect {
            n: 2,
            ok: false,
            ..
        }
    ));

    let cases: [(&[u8], &str); 5] = [
        (&[0x20, 2, 0, 5], "refused"),
        (&[0x10, 0], "protocol_unexpected"),
        (&[0x20, 3, 0, 0, 0], "protocol_malformed"),
        (&[0x20, 2, 1, 0], "out_of_order"),
        (&[0xd0, 0], "out_of_order"),
    ];
    for (bytes, reason) in cases {
        fake::with(|h| {
            h.uptime_ms += BACKOFF_MAX_MS;
            h.tcp_connects.push_back(Some(9));
        });
        rx(bytes);
        let records = run(&mut link, 2);
        assert!(
            matches!(records.last(), Some(Record::Closed { reason: r }) if r.as_str() == reason),
            "{bytes:?}"
        );
        assert!(!link.ready());
    }

    // A foreign topic after Ready, and the broker hanging up.
    fake::with(|h| h.uptime_ms += BACKOFF_MAX_MS);
    connect(&mut link);
    rx(&publish("petlibre/1/dev1/health", "{}"));
    assert_eq!(
        run(&mut link, 1).last(),
        Some(&Record::Closed {
            reason: CloseReason::ForeignTopic
        })
    );

    fake::with(|h| h.uptime_ms += BACKOFF_MAX_MS);
    connect(&mut link);
    fake::with(|h| h.tcp_rx.push_back(TcpRecvScript::Closed));
    assert_eq!(
        run(&mut link, 1),
        vec![Record::Closed {
            reason: CloseReason::Closed
        }]
    );

    // A failed send closes too.
    fake::with(|h| h.uptime_ms += BACKOFF_MAX_MS);
    connect(&mut link);
    // The next periodic health publish is the send that fails.
    fake::with(|h| {
        h.tcp_send_fails = true;
        h.uptime_ms += 30_000;
    });
    assert_eq!(
        run(&mut link, 1),
        vec![Record::Closed {
            reason: CloseReason::Send
        }]
    );

    // A silent broker times out the CONNACK.
    fake::with(|h| {
        h.tcp_send_fails = false;
        h.uptime_ms += BACKOFF_MAX_MS;
        h.tcp_connects.push_back(Some(11));
    });

    let records = run(
        &mut link,
        1 + (petlibre_core::transport::REPLY_TIMEOUT_MS / POLL_MS) as usize,
    );
    assert_eq!(
        records.last(),
        Some(&Record::Closed {
            reason: CloseReason::ReplyTimeout
        })
    );
    assert!(records.iter().all(|r| !matches!(r, Record::Publish { .. })));
}

#[test]
fn split_and_coalesced_packets_are_reassembled() {
    let (_guard, mut link) = start();
    fake::with(|h| h.tcp_connects.push_back(Some(7)));
    rx(&CONNACK[..1]);
    rx(&CONNACK[1..]);

    let mut both = SUBACK.to_vec();
    both.extend(publish(
        "petlibre/1/dev1/request",
        &format!(r#"{{"id":"{ID}","op":"ping"}}"#),
    ));
    rx(&both);

    let records = run(&mut link, 4);
    assert!(records.contains(&Record::Ignored), "{records:?}");
    assert!(link.ready());
}

#[test]
fn records_describe_without_credentials() {
    let mut text = String::new();
    describe(
        Record::Connect {
            n: 2,
            ok: true,
            ms: 40,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "connect n=2 ok=1 ms=40");

    let mut text = String::new();
    describe(
        Record::Closed {
            reason: CloseReason::Silence,
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(text, "closed reason=silence");
}
