//! Byte fixtures for every device API message of major version 1. Postcard
//! is positional and not self-describing, so a reordered, removed or
//! retyped field changes these bytes: the test fails here instead of in a
//! home. A new variant or op appended at the end adds a fixture and leaves
//! the others alone.
//!
//! `WELLFED_BLESS=1 cargo nextest run -p wellfed-proto` appends the
//! fixtures of new variants to `fixtures/v1.txt`. It refuses to rewrite a
//! line already there: v1 is frozen, so a struct that gained a field, or a
//! variant that moved, fails here even when blessed. The rules are in the
//! [device API](../../../../docs/firmware/device-api.md#evolution).

use wellfed_proto::api::*;
use wellfed_proto::{decode, encode};
use wellfed_session::hex::Lower;
use serde::Serialize;

const ID: RequestId = [0x11; 16];

fn hex(bytes: &[u8]) -> String {
    Lower(bytes).to_string()
}

/// A fixture: its name, its bytes, and how to check they decode back.
type Fixture = (&'static str, Vec<u8>, fn(&[u8]) -> bool);

/// Every message shape.
fn messages() -> Vec<Fixture> {
    fn entry<T: Serialize>(value: &T) -> Vec<u8> {
        let mut out = vec![0; FRAME_MAX];
        let len = encode(value, &mut out).unwrap();
        out.truncate(len);
        out
    }
    let client = |bytes: &[u8]| decode::<ClientMessage<'_>>(bytes).is_ok();
    let feeder = |bytes: &[u8]| decode::<FeederMessage<'_>>(bytes).is_ok();
    let request = |op| ClientMessage::Request { id: ID, op };
    let response = |result| FeederMessage::Response { id: ID, result };
    let wifi = Wifi {
        ssid: b"lab",
        psk: "passphrase",
    };
    let status = Status {
        build: "b",
        bank: 1,
        uptime_ms: 2,
        boot_count: 3,
        reset_flags: 4,
        faults: 0,
        rssi: -50,
        wifi: WifiSource::Stored,
        link_losses: 5,
        dns_losses: 6,
        clock_ms: Some(1_700_000_000_000),
        power: Some(Power {
            adapter: true,
            battery_mv: Some(4_800),
        }),
        locked: Some(false),
        settings_generation: 6,
        wifi_generation: 7,
        last_event: Some(8),
        sessions: 1,
    };
    let mut clients = [None; CLIENTS_MAX];
    clients[0] = Some(Client {
        key: [0x22; 32],
        label: "phone",
        role: Role::Owner,
        you: true,
    });

    vec![
        ("open-session", entry(&Open::Session), |b| {
            decode::<Open>(b).is_ok()
        }),
        ("open-invite", entry(&Open::Invite { id: 7 }), |b| {
            decode::<Open>(b).is_ok()
        }),
        ("open-recovery", entry(&Open::Recovery), |b| {
            decode::<Open>(b).is_ok()
        }),
        (
            "hello",
            entry(&Hello {
                major: 1,
                capabilities: Capabilities::V1,
            }),
            |b| decode::<Hello>(b).is_ok(),
        ),
        (
            "welcome-open",
            entry(&Welcome::Open {
                major: 1,
                capabilities: Capabilities::V1,
                build: "b",
                role: Role::Member,
                epoch: 0x1234_5678,
            }),
            |b| decode::<Welcome<'_>>(b).is_ok(),
        ),
        (
            "welcome-refused",
            entry(&Welcome::Refused(OpenRefusal::Version { supported: 1 })),
            |b| decode::<Welcome<'_>>(b).is_ok(),
        ),
        ("join", entry(&Join { label: "phone" }), |b| {
            decode::<Join<'_>>(b).is_ok()
        }),
        (
            "joined",
            entry(&Joined::Enrolled { role: Role::Server }),
            |b| decode::<Joined>(b).is_ok(),
        ),
        ("op-status", entry(&request(Op::Status)), client),
        ("op-get-settings", entry(&request(Op::GetSettings)), client),
        (
            "op-put-settings",
            entry(&request(Op::PutSettings {
                base: 9,
                changes: SettingsChanges {
                    lights: Some(false),
                    sound: None,
                    volume: Some(30),
                },
            })),
            client,
        ),
        (
            "op-put-wifi",
            entry(&request(Op::PutWifi {
                base: 9,
                wifi: Some(wifi),
            })),
            client,
        ),
        (
            "op-set-lock",
            entry(&request(Op::SetLock { locked: true })),
            client,
        ),
        (
            "op-play",
            entry(&request(Op::Play { tune: Tune::Meal })),
            client,
        ),
        (
            "op-time-sync",
            entry(&request(Op::TimeSync(TimeSync {
                unix_ms: 1_792_674_000_000,
                offset_s: -21_600,
                next_change_s: 1_793_520_000,
                next_offset_s: -25_200,
            }))),
            client,
        ),
        (
            "op-create-invite",
            entry(&request(Op::CreateInvite { role: Role::Member })),
            client,
        ),
        ("op-list-clients", entry(&request(Op::ListClients)), client),
        (
            "op-revoke-client",
            entry(&request(Op::RevokeClient { key: [0x33; 32] })),
            client,
        ),
        ("op-feed", entry(&request(Op::Feed { portions: 2 })), client),
        ("op-reboot", entry(&request(Op::Reboot)), client),
        (
            "subscribe",
            entry(&ClientMessage::Subscribe {
                after: Some(41),
                samples: true,
            }),
            client,
        ),
        (
            "outcome-done",
            entry(&response(Outcome::Done {
                generation: Some(10),
            })),
            feeder,
        ),
        (
            "outcome-status",
            entry(&response(Outcome::Status(status))),
            feeder,
        ),
        (
            "outcome-settings",
            entry(&response(Outcome::Settings(Settings {
                generation: 10,
                lights: true,
                sound: false,
                volume: 40,
                writable: true,
            }))),
            feeder,
        ),
        (
            "outcome-clients",
            entry(&response(Outcome::Clients(Clients { clients }))),
            feeder,
        ),
        (
            "outcome-invite",
            entry(&response(Outcome::Invite(Invite {
                id: 3,
                secret: [0x44; 32],
                seconds: 600,
            }))),
            feeder,
        ),
        (
            "outcome-stale",
            entry(&response(Outcome::Refused(Refusal::Stale { current: 11 }))),
            feeder,
        ),
        (
            "event",
            entry(&FeederMessage::Event(Event {
                seq: 12,
                at: Some(1_792_674_000),
                body: EventBody::Lock {
                    locked: true,
                    by: Source::Client([1, 2, 3, 4]),
                },
            })),
            feeder,
        ),
        ("gap", entry(&FeederMessage::Gap { from: 1, to: 5 }), feeder),
        (
            "sample",
            entry(&FeederMessage::Sample(Sample {
                at_ms: 13,
                metric: Metric::Rssi(-61),
            })),
            feeder,
        ),
        (
            "op-cancel-feed",
            entry(&request(Op::CancelFeed { feed: 14 })),
            client,
        ),
        (
            "outcome-feeding",
            entry(&response(Outcome::Feeding { feed: 14 })),
            feeder,
        ),
        (
            "event-fed",
            entry(&FeederMessage::Event(Event {
                seq: 15,
                at: Some(1_792_674_000),
                body: EventBody::Fed {
                    feed: 14,
                    requested: 2,
                    completed: 1,
                    outcome: FeedOutcome::Failed,
                    reason: FeedReason::NoFeedback,
                    by: Source::Panel,
                },
            })),
            feeder,
        ),
        (
            "outcome-fault",
            entry(&response(Outcome::Refused(Refusal::Fault))),
            feeder,
        ),
        (
            "hello-feed",
            entry(&Hello {
                major: 1,
                capabilities: Capabilities::FEED,
            }),
            |b| decode::<Hello>(b).is_ok(),
        ),
        (
            "hello-update",
            entry(&Hello {
                major: 1,
                capabilities: Capabilities::FEED.union(Capabilities::UPDATE),
            }),
            |b| decode::<Hello>(b).is_ok(),
        ),
        (
            "op-offer-update",
            entry(&request(Op::OfferUpdate {
                manifest: &[0x51; 4],
                downgrade: Some(&[0x52; 3]),
            })),
            client,
        ),
        (
            "op-update-chunk",
            entry(&request(Op::UpdateChunk {
                offset: 4096,
                data: &[0x53; 5],
            })),
            client,
        ),
        ("op-update-state", entry(&request(Op::UpdateState)), client),
        (
            "op-cancel-update",
            entry(&request(Op::CancelUpdate)),
            client,
        ),
        (
            "op-enroll-update-key",
            entry(&request(Op::EnrollUpdateKey {
                key: [0x54; 32],
                label: "laptop",
            })),
            client,
        ),
        (
            "op-update-key-statement",
            entry(&request(Op::UpdateKeyStatement {
                statement: &[0x55; 6],
            })),
            client,
        ),
        (
            "outcome-update",
            entry(&response(Outcome::Update(UpdateView {
                phase: UpdatePhase::Receiving,
                bank: 0,
                min_version: 168,
                digest: Some([0x56; 32]),
                version: 170,
                image_len: 942_080,
                written: 8192,
                wanted: [Some(8192), Some(8704), None, None, None, None, None, None],
                stopped: Some(UpdateStop::Timeout),
            }))),
            feeder,
        ),
        (
            "outcome-update-refused",
            entry(&response(Outcome::Refused(Refusal::Update(
                UpdateRefusal::Downgrade,
            )))),
            feeder,
        ),
        (
            "hello-time",
            entry(&Hello {
                major: 1,
                capabilities: Capabilities::FEED
                    .union(Capabilities::UPDATE)
                    .union(Capabilities::TIME),
            }),
            |b| decode::<Hello>(b).is_ok(),
        ),
        ("op-get-time", entry(&request(Op::GetTime)), client),
        (
            "op-put-time",
            entry(&request(Op::PutTime {
                base: 3,
                changes: TimeChanges {
                    zone: Some("MST7MDT,M3.2.0,M11.1.0"),
                    ntp: Some(NtpServer::Address {
                        ip: [10, 42, 103, 1],
                        port: 123,
                    }),
                },
            })),
            client,
        ),
        (
            "outcome-time",
            entry(&response(Outcome::Time(TimeView {
                generation: 4,
                writable: true,
                zone: "MST7MDT,M3.2.0,M11.1.0",
                ntp: NtpServer::Gateway { port: 123 },
                clock_ms: Some(1_792_674_000_000),
                error_ms: Some(1_120),
                offset_s: Some(-21_600),
                rate_ppb: 735_000,
                rate_bound_ppb: 100_000,
                learned: true,
                syncs: 9,
                last: Some(TimeSource::Ntp),
                last_age_s: Some(60),
                ntp_last: Some(NtpResult::Used),
            }))),
            feeder,
        ),
    ]
}

/// Every handshake binds the prologue, so changing it would split the
/// protocol without the typed refusal a major version gets.
#[test]
fn the_prologue_never_changes() {
    assert_eq!(PROLOGUE, b"wellfed-api-v1");
    assert_eq!(PROTOCOL_MAJOR, 1);
}

#[test]
fn every_v1_message_keeps_its_bytes() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v1.txt");
    let current: String = messages()
        .iter()
        .map(|(name, bytes, _)| format!("{name} {}\n", hex(bytes)))
        .collect();
    if std::env::var_os("WELLFED_BLESS").is_some() {
        let fixed = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            current.starts_with(&fixed),
            "a pinned v1 message changed bytes: that breaks every deployed peer, so it is never blessed"
        );
        std::fs::write(path, &current).unwrap();
    }

    let fixed = std::fs::read_to_string(path).unwrap();
    for (line, (name, bytes, decodes)) in fixed.lines().zip(messages()) {
        assert_eq!(
            line,
            format!("{name} {}", hex(&bytes)),
            "{name} changed bytes"
        );
        assert!(decodes(&bytes), "{name} no longer decodes");
    }
    assert_eq!(fixed, current, "a fixture was added or removed: bless it");
}
