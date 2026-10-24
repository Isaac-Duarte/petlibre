//! Wire-level tests for the local transport: MQTT 3.1.1 subset encoding
//! and decoding, the session state machine, request parsing and the health
//! and reply encoders.
//!
//! The expected bytes are pinned literally. The session must close on every
//! protocol failure, and replies must never echo an unsupported request name
//! (see docs/firmware/local-transport.md).

use petlibre_core::transport::*;

const REQUEST: &str = "petlibre/1/dev1/request";
const BUILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const LONG: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn connect(will: Option<Will>) -> Vec<u8> {
    let mut buf = [0; MAX_PACKET];
    let c = Connect {
        client_id: "petlibre-fw-dev1",
        username: "petlibre-fw",
        password: b"secret",
        will,
    };

    let n = encode_connect(&mut buf, &c).unwrap();
    buf[..n].to_vec()
}

#[test]
fn connect_encodes_the_3_1_1_subset() {
    let bytes = connect(None);

    let mut expected = vec![0x10, 0];
    expected.extend([0, 4, b'M', b'Q', b'T', b'T', 4, 0xc2, 0, 30]);
    for field in [&b"petlibre-fw-dev1"[..], b"petlibre-fw", b"secret"] {
        expected.extend((field.len() as u16).to_be_bytes());
        expected.extend(field);
    }
    expected[1] = (expected.len() - 2) as u8;
    assert_eq!(bytes, expected);

    let with_will = connect(Some(Will {
        topic: "petlibre/1/dev1/health",
        payload: b"{\"online\":false}",
    }));
    assert_eq!(
        with_will[9],
        0xc2 | 0x04,
        "will flag only: QoS 0, not retained"
    );
    assert!(
        with_will
            .windows(22)
            .any(|w| w == b"petlibre/1/dev1/health")
    );
}

#[test]
fn publish_subscribe_ping_disconnect_encode() {
    let mut buf = [0; MAX_PACKET];
    let n = encode_publish(&mut buf, "a/b", b"xy", true).unwrap();
    assert_eq!(&buf[..n], &[0x31, 7, 0, 3, b'a', b'/', b'b', b'x', b'y']);

    let n = encode_publish(&mut buf, "a/b", b"", false).unwrap();
    assert_eq!(&buf[..n], &[0x30, 5, 0, 3, b'a', b'/', b'b']);

    let n = encode_subscribe(&mut buf, 1, "t").unwrap();
    assert_eq!(&buf[..n], &[0x82, 6, 0, 1, 0, 1, b't', 0]);

    assert_eq!(encode_pingreq(&mut buf), Ok(2));
    assert_eq!(&buf[..2], &[0xc0, 0]);

    assert_eq!(encode_disconnect(&mut buf), Ok(2));
    assert_eq!(&buf[..2], &[0xe0, 0]);

    // A two-byte remaining length.
    let payload = [b'z'; 300];
    let n = encode_publish(&mut buf, "t", &payload, false).unwrap();
    assert_eq!(&buf[..3], &[0x30, 0xaf, 0x02]); // 303 = 0x2f + 2 * 128
    assert_eq!(n, 3 + 303);
}

#[test]
fn encoders_refuse_overflow_instead_of_truncating() {
    let mut small = [0; 8];
    assert_eq!(
        encode_publish(&mut small, "topic", b"payload", false),
        Err(Error::TooLong)
    );

    let mut buf = [0; 2 * MAX_PACKET];
    let big = [0; MAX_PACKET];
    assert_eq!(
        encode_publish(&mut buf, "t", &big, false),
        Err(Error::TooLong)
    );
}

#[test]
fn decode_accepts_only_the_expected_packets() {
    assert_eq!(decode(&[]), Ok(None));
    assert_eq!(decode(&[0x20]), Ok(None));
    assert_eq!(decode(&[0x20, 2, 0]), Ok(None));

    assert_eq!(
        decode(&[0x20, 2, 0, 0, 0xff]),
        Ok(Some((
            Packet::ConnAck {
                session_present: false,
                code: 0
            },
            4
        )))
    );
    assert_eq!(
        decode(&[0x20, 2, 0, 5]),
        Ok(Some((
            Packet::ConnAck {
                session_present: false,
                code: 5
            },
            4
        )))
    );
    assert_eq!(
        decode(&[0x90, 3, 0, 1, 0]),
        Ok(Some((
            Packet::SubAck {
                packet_id: 1,
                code: 0
            },
            5
        )))
    );

    assert_eq!(decode(&[0xd0, 0]), Ok(Some((Packet::PingResp, 2))));
    let publish = [0x30, 6, 0, 1, b't', b'{', b'}', b'!'];
    assert_eq!(
        decode(&publish),
        Ok(Some((
            Packet::Publish {
                topic: "t",
                payload: b"{}!",
                retain: false
            },
            8
        )))
    );

    // Wrong lengths, reserved bits, QoS > 0, unknown types, over-long lengths.
    assert_eq!(decode(&[0x20, 3, 0, 0, 0]), Err(Error::Malformed));
    assert_eq!(decode(&[0x20, 2, 2, 0]), Err(Error::Malformed));
    assert_eq!(decode(&[0x21, 2, 0, 0]), Err(Error::Malformed));
    assert_eq!(decode(&[0x32, 4, 0, 1, b't', 0]), Err(Error::Unexpected));
    assert_eq!(decode(&[0x10, 0]), Err(Error::Unexpected));
    assert_eq!(
        decode(&[0x30, 0x80, 0x80, 0x80, 0x80, 1]),
        Err(Error::Malformed)
    );
    // One byte over MAX_PACKET (833).
    assert_eq!(decode(&[0x30, 0xc1, 0x06]), Err(Error::TooLong));
    assert_eq!(decode(&[0x30, 3, 0, 5, b't']), Err(Error::Malformed));
    assert_eq!(decode(&[0x30, 3, 0, 1, 0xff]), Err(Error::Malformed));
}

#[test]
fn session_runs_connect_subscribe_ping_and_requests() {
    let mut s = Session::connecting(1000);
    assert_eq!(s.state(), State::AwaitConnAck);
    assert_eq!(s.tick(5000), Tick::Idle);

    assert_eq!(
        s.on_packet(
            1200,
            Packet::ConnAck {
                session_present: false,
                code: 0
            },
            REQUEST
        ),
        Event::Connected
    );

    let mut buf = [0; MAX_PACKET];
    let n = s.subscribe(1200, &mut buf, REQUEST).unwrap();
    assert_eq!(buf[0], 0x82);
    assert!(n > 4);
    assert_eq!(
        s.on_packet(
            1300,
            Packet::SubAck {
                packet_id: 1,
                code: 0
            },
            REQUEST
        ),
        Event::Ready
    );
    assert_eq!(s.state(), State::Ready);

    // Publishing defers the ping; then it is due.
    s.sent(10_000);
    assert_eq!(s.tick(24_999), Tick::Idle);
    assert_eq!(s.tick(25_000), Tick::Ping);
    assert_eq!(s.tick(26_000), Tick::Idle, "one ping outstanding");
    assert_eq!(s.on_packet(26_100, Packet::PingResp, REQUEST), Event::Pong);

    let payload = br#"{"id":"x","op":"ping"}"#;
    assert_eq!(
        s.on_packet(
            27_000,
            Packet::Publish {
                topic: REQUEST,
                payload,
                retain: false
            },
            REQUEST
        ),
        Event::Request(payload)
    );
    assert_eq!(s.state(), State::Ready);
}

#[test]
fn session_closes_on_every_failure() {
    let connack = Packet::ConnAck {
        session_present: false,
        code: 0,
    };
    let suback = Packet::SubAck {
        packet_id: 1,
        code: 0,
    };

    let ready = || {
        let mut s = Session::connecting(0);
        s.on_packet(1, connack, REQUEST);
        s.on_packet(2, suback, REQUEST);
        s
    };

    let cases: Vec<(Session, Packet, Fault)> = vec![
        (
            Session::connecting(0),
            Packet::ConnAck {
                session_present: false,
                code: 5,
            },
            Fault::Refused(5),
        ),
        (
            Session::connecting(0),
            Packet::ConnAck {
                session_present: true,
                code: 0,
            },
            Fault::OutOfOrder,
        ),
        (Session::connecting(0), suback, Fault::OutOfOrder),
        (Session::connecting(0), Packet::PingResp, Fault::OutOfOrder),
        (ready(), connack, Fault::OutOfOrder),
        (ready(), Packet::PingResp, Fault::OutOfOrder),
        (
            ready(),
            Packet::Publish {
                topic: "petlibre/1/dev1/health",
                payload: b"",
                retain: false,
            },
            Fault::ForeignTopic,
        ),
        (
            ready(),
            Packet::Publish {
                topic: REQUEST,
                payload: b"{}",
                retain: true,
            },
            Fault::OutOfOrder,
        ),
        (
            ready(),
            Packet::Publish {
                topic: REQUEST,
                payload: &[b' '; MAX_PAYLOAD + 1],
                retain: false,
            },
            Fault::Protocol(Error::TooLong),
        ),
    ];

    for (index, (mut s, packet, fault)) in cases.into_iter().enumerate() {
        assert_eq!(
            s.on_packet(10, packet, REQUEST),
            Event::Closed(fault),
            "case {index}"
        );
        assert_eq!(s.state(), State::Closed(fault));
        assert_eq!(s.tick(20), Tick::Closed(fault), "closed stays closed");
        assert_eq!(
            s.on_packet(30, Packet::PingResp, REQUEST),
            Event::Closed(fault)
        );
    }

    let mut s = Session::connecting(0);
    s.on_packet(1, connack, REQUEST);
    assert_eq!(
        s.on_packet(
            2,
            Packet::SubAck {
                packet_id: 1,
                code: 0x80
            },
            REQUEST
        ),
        Event::Closed(Fault::SubscribeRefused(0x80))
    );

    // Timeouts: CONNACK, SUBACK, PINGRESP, and silence.
    let mut s = Session::connecting(u32::MAX - 100);
    assert_eq!(s.tick(REPLY_TIMEOUT_MS - 102), Tick::Idle);
    assert_eq!(
        s.tick(REPLY_TIMEOUT_MS - 101),
        Tick::Closed(Fault::ReplyTimeout),
        "across the wrap"
    );

    let mut s = Session::connecting(0);
    s.on_packet(1, connack, REQUEST);
    let mut buf = [0; MAX_PACKET];
    s.subscribe(1, &mut buf, REQUEST).unwrap();
    assert_eq!(
        s.tick(1 + REPLY_TIMEOUT_MS),
        Tick::Closed(Fault::ReplyTimeout)
    );

    let mut s = ready();
    assert_eq!(s.tick(PING_AFTER_MS + 2), Tick::Ping);
    assert_eq!(
        s.tick(PING_AFTER_MS + 2 + REPLY_TIMEOUT_MS),
        Tick::Closed(Fault::ReplyTimeout)
    );

    let mut s = ready();
    for now in (1000..SILENCE_MS).step_by(1000) {
        s.sent(now); // Keeps pings from being due; the broker stays silent.
        assert_eq!(s.tick(now), Tick::Idle);
    }
    assert_eq!(s.tick(SILENCE_MS + 2), Tick::Closed(Fault::Silence));

    assert_eq!(
        s.on_error(Error::Malformed),
        Fault::Protocol(Error::Malformed)
    );
}

fn health() -> Health {
    Health {
        online: true,
        seq: 12,
        uptime_ms: 61_234,
        build: BUILD,
        bank: 1,
        reset_flags: 0,
        boot_count: 7,
        outputs: Outputs::Stopped,
        faults: 0,
        rssi: -20,
        wifi_attempt: 2,
        link_losses: 0,
        dns_losses: 1,
        last_loss: Loss::Dns,
        connects: 1,
        wifi_source: WifiSource::Build,
        last_error: None,
        clock: None,
        clock_syncs: 0,
        power: None,
        role: Role::Normal,
    }
}

fn render(h: &Health) -> Result<String, core::fmt::Error> {
    let mut buf = [0; MAX_HEALTH];
    let mut out = Buffer::new(&mut buf);
    h.write(&mut out)?;
    Ok(String::from_utf8(out.as_bytes().to_vec()).unwrap())
}

#[test]
fn health_renders_the_v1_schema_within_its_limit() {
    let text = render(&health()).unwrap();
    let build = BUILD;
    assert_eq!(
        text,
        format!(
            r#"{{"schema":"petlibre.health","v":1,"online":true,"seq":12,"uptime_ms":61234,"build":"{build}","bank":1,"reset_flags":"00000000","boot_count":7,"outputs":"stopped","faults":[],"wifi":{{"rssi":-20,"attempt":2,"losses":{{"link":0,"dns":1}},"last_loss":"dns","source":"build"}},"transport":{{"connects":1,"last_error":null}},"clock":{{"syncs":0}},"power":null}}"#
        )
    );

    // The largest values still fit.
    let worst = Health {
        seq: u32::MAX,
        uptime_ms: u32::MAX,
        reset_flags: u32::MAX,
        boot_count: u32::MAX,
        outputs: Outputs::Unverified,
        faults: u32::MAX,
        rssi: i32::MIN,
        wifi_attempt: u32::MAX,
        link_losses: u32::MAX,
        dns_losses: u32::MAX,
        last_loss: Loss::Link,
        connects: u32::MAX,
        wifi_source: WifiSource::Fallback,
        last_error: Some("protocol_malformed"),
        // The clock's own bounds (`wall_clock`).
        clock: Some(ClockHealth {
            unix_ms: 4_102_444_799_999,
            offset_s: -43_200,
            uncertainty_ms: 60_000,
        }),
        clock_syncs: u32::MAX,
        power: Some(PowerHealth {
            adapter: true,
            // A 3.3 V pin through the divider, and the staleness bound
            // (`petlibre_hal::power`).
            battery_mv: Some(6_808),
            age_ms: 15_000,
        }),
        role: Role::Recovery,
        ..health()
    };

    let text = render(&worst).unwrap();
    assert!(text.len() <= MAX_HEALTH, "{}", text.len());
    assert!(text.contains(r#""faults":["watchdog","flash_hook","stopped_readback"]"#));
    assert!(
        text.contains(r#""outputs":"unverified""#)
            && text.contains(r#""last_loss":"link","source":"fallback""#)
    );
    assert!(
        text.contains(
            r#""last_error":"protocol_malformed"},"role":"recovery","clock":{"syncs":4294967295,"unix_ms":"#
        )
    );
    assert!(text.ends_with(
        r#""uncertainty_ms":60000},"power":{"adapter":true,"battery_mv":6808,"age_ms":15000}}"#
    ));

    // No pack fitted.
    let no_pack = render(&Health {
        power: Some(PowerHealth {
            adapter: true,
            battery_mv: None,
            age_ms: 0,
        }),
        ..health()
    })
    .unwrap();
    assert!(no_pack.ends_with(r#","power":{"adapter":true,"battery_mv":null,"age_ms":0}}"#));

    let sensing = render(&Health {
        outputs: Outputs::Sensing,
        ..health()
    })
    .unwrap();
    assert!(sensing.contains(r#""outputs":"sensing","faults":[]"#));

    let offline = render(&Health {
        online: false,
        last_loss: Loss::None,
        ..health()
    })
    .unwrap();
    assert!(offline.contains(r#""online":false"#) && offline.contains(r#""last_loss":null"#));

    // Non-hex identities and unsafe error codes are refused, not escaped.
    assert!(
        render(&Health {
            build: "ab\"c",
            ..health()
        })
        .is_err()
    );
    assert!(
        render(&Health {
            build: LONG,
            ..health()
        })
        .is_err()
    );
    assert!(
        render(&Health {
            last_error: Some("a\"b"),
            ..health()
        })
        .is_err()
    );

    let mut tiny = [0; 16];
    assert!(health().write(&mut Buffer::new(&mut tiny)).is_err());
}

#[test]
fn the_updater_subscribes_a_data_topic_and_only_it_receives_data() {
    let (req, data) = ("petlibre/1/X/request", "petlibre/1/X/ota/data");
    let mut buf = [0; MAX_PACKET];
    let mut s = Session::connecting(0);

    assert_eq!(
        s.on_packet(
            1,
            Packet::ConnAck {
                session_present: false,
                code: 0
            },
            req
        ),
        Event::Connected
    );

    assert!(
        s.subscribe_data(1, &mut buf, data).is_err(),
        "not before ready"
    );

    s.subscribe(1, &mut buf, req).unwrap();
    assert_eq!(
        s.on_packet(
            2,
            Packet::SubAck {
                packet_id: 1,
                code: 0
            },
            req
        ),
        Event::Ready
    );

    let n = s.subscribe_data(3, &mut buf, data).unwrap();
    assert_eq!(&buf[..2], &[0x82, (n - 2) as u8]);
    assert_eq!(
        s.on_packet_with(
            4,
            Packet::SubAck {
                packet_id: 2,
                code: 0
            },
            req,
            Some(data)
        ),
        Event::DataReady
    );

    let publish = Packet::Publish {
        topic: data,
        payload: &[1, 2, 3],
        retain: false,
    };
    assert_eq!(
        s.on_packet_with(5, publish, req, Some(data)),
        Event::Data(&[1, 2, 3])
    );

    // The normal role never names a data topic: the same publish is foreign.
    let mut normal = s;
    assert_eq!(
        normal.on_packet(6, publish, req),
        Event::Closed(Fault::ForeignTopic)
    );

    // A refused data subscription closes.
    let mut refused = Session::connecting(0);
    refused.on_packet(
        1,
        Packet::ConnAck {
            session_present: false,
            code: 0,
        },
        req,
    );
    refused.subscribe(1, &mut buf, req).unwrap();
    refused.on_packet(
        2,
        Packet::SubAck {
            packet_id: 1,
            code: 0,
        },
        req,
    );
    refused.subscribe_data(3, &mut buf, data).unwrap();
    assert_eq!(
        refused.on_packet_with(
            4,
            Packet::SubAck {
                packet_id: 2,
                code: 0x80
            },
            req,
            Some(data)
        ),
        Event::Closed(Fault::SubscribeRefused(0x80))
    );
}

/// The C join shim reads these codes (0 build, 1 stored, 2 fallback).
#[test]
fn wifi_source_codes_round_trip() {
    for (source, code) in [
        (WifiSource::Build, 0),
        (WifiSource::Stored, 1),
        (WifiSource::Fallback, 2),
    ] {
        assert_eq!(source.code(), code);
        assert_eq!(WifiSource::from_code(code), source);
    }
    assert_eq!(WifiSource::from_code(3), WifiSource::Build);
}
