//! Bounded properties of the feeder's network-facing state machines: the
//! device API's server and BLE setup's responder survive any bytes, and a
//! genuine client's setup ends as the stores' rules say. The drivers live
//! in `support`, which the cargo-fuzz targets share.

mod support;

use proptest::prelude::*;
use support::Setup;
use wellfed_device_api::clients::Label;
use wellfed_device_api::wifi::Credentials;
use wellfed_proto::api::FRAME_MAX;
use wellfed_proto::setup::{self, Request};

/// A clear opening frame the server takes (session, invite or recovery),
/// then arbitrary bytes, so the cases reach the handshakes behind it.
fn opening() -> impl Strategy<Value = Vec<u8>> {
    let open = proptest::sample::select(vec![vec![0u8], vec![1, 0], vec![1, 7], vec![2]]);
    let body = proptest::collection::vec(any::<u8>(), 0..160);
    (open, body, any::<bool>()).prop_map(|(open, body, framed)| {
        let mut bytes = support::framed(&open);
        if framed {
            bytes.extend(support::framed(&body));
        } else {
            bytes.extend(body);
        }
        bytes
    })
}

/// A sealed request: arbitrary, or a request tag, an id and an op tag in
/// front of arbitrary fields, so the cases reach every op.
fn request() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..48),
        (
            0u8..2,
            any::<[u8; 16]>(),
            0u8..14,
            proptest::collection::vec(any::<u8>(), 0..48)
        )
            .prop_map(|(tag, id, op, rest)| {
                let mut bytes = vec![tag];
                bytes.extend(id);
                bytes.push(op);
                bytes.extend(rest);
                bytes
            }),
    ]
}

proptest! {
    #[test]
    fn a_connection_survives_any_bytes(
        bytes in prop_oneof![opening(), proptest::collection::vec(any::<u8>(), 0..2 * FRAME_MAX)],
    ) {
        support::connection(&bytes);
    }

    #[test]
    fn a_session_survives_any_requests(
        messages in proptest::collection::vec(request(), 0..6),
    ) {
        let messages: Vec<&[u8]> = messages.iter().map(Vec::as_slice).collect();
        support::session(&messages);
    }

    #[test]
    fn a_full_table_survives_any_bytes(bytes in opening()) {
        support::crowd(&bytes);
    }

    #[test]
    fn setup_survives_any_messages(
        messages in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..160), 0..5),
    ) {
        let messages: Vec<&[u8]> = messages.iter().map(Vec::as_slice).collect();
        support::setup_messages(&messages);
    }

    /// A genuine client is offered exactly when its label is one the
    /// client store takes, proven exactly when both sides hold the same
    /// code, and its Wi-Fi is saved exactly when the Wi-Fi store takes it.
    #[test]
    fn a_genuine_setup_ends_as_the_stores_rules_say(
        label in proptest::collection::vec(any::<u8>(), 0..20),
        raw in any::<bool>(),
        feeder_code in any::<u16>(),
        same_code in any::<bool>(),
        other_code in any::<u16>(),
        ssid in proptest::collection::vec(any::<u8>(), 0..36),
        psk in "[ -~]{0,66}|\\PC{0,20}",
        send_wifi in any::<bool>(),
    ) {
        let text = String::from_utf8_lossy(&label).into_owned();
        let mut request = [0; 64];
        let request = if raw {
            &label[..]
        } else {
            let len = wellfed_proto::encode(&Request { label: &text }, &mut request).unwrap();
            &request[..len]
        };
        let client_code = if same_code { feeder_code } else { other_code };
        let wifi = send_wifi.then_some(setup::Wifi { ssid: &ssid, psk: &psk });

        let outcome = support::setup_client(request, feeder_code, client_code, wifi);

        let decoded = wellfed_proto::decode::<Request<'_>>(request).ok();
        let offered = decoded.is_some_and(|r| Label::new(r.label.as_bytes()).is_some());
        let codes_match = wellfed_session::setup_code::Code::from_bits(feeder_code)
            == wellfed_session::setup_code::Code::from_bits(client_code);
        let credentials = wifi.map(|w| Credentials::new(w.ssid, w.psk.as_bytes()));
        let expected = match credentials {
            _ if !offered || !codes_match => Setup::Refused,
            Some(None) => Setup::Refused,
            Some(Some(credentials)) => Setup::Proven(Some(credentials)),
            None => Setup::Proven(None),
        };
        prop_assert_eq!(outcome, expected);
    }
}

/// The session driver reaches the request handlers: a real request is
/// answered, so the properties above are not only exercising refusals.
#[test]
fn the_session_driver_is_answered() {
    use wellfed_proto::api::{ClientMessage, Op};

    let mut out = [0; 64];
    let status = ClientMessage::Request {
        id: [1; 16],
        op: Op::Status,
    };
    let len = wellfed_proto::encode(&status, &mut out).unwrap();
    let feed = ClientMessage::Request {
        id: [2; 16],
        op: Op::Feed { portions: 1 },
    };
    let mut second = [0; 64];
    let second_len = wellfed_proto::encode(&feed, &mut second).unwrap();
    assert_eq!(support::session(&[&out[..len], &second[..second_len]]), 2);
}

/// A trace's widest page leaves in one frame.
#[test]
fn a_trace_page_is_answered_within_a_frame() {
    use wellfed_proto::api::{ClientMessage, Op};

    let mut out = [0; 64];
    let request = ClientMessage::Request {
        id: [7; 16],
        op: Op::FeedTrace {
            feed: u32::MAX,
            offset: u16::MAX,
        },
    };
    let len = wellfed_proto::encode(&request, &mut out).unwrap();
    assert_eq!(support::session(&[&out[..len]]), 1);
}

/// Every statistics section at its widest leaves in one frame.
#[test]
fn every_stats_section_is_answered_within_a_frame() {
    use wellfed_proto::api::{ClientMessage, Op, StatsSection};

    let requests: Vec<Vec<u8>> = StatsSection::ALL
        .iter()
        .zip(1u8..)
        .map(|(&section, id)| {
            let mut out = [0; 64];
            let request = ClientMessage::Request {
                id: [id; 16],
                op: Op::Stats { section },
            };
            let len = wellfed_proto::encode(&request, &mut out).unwrap();
            out[..len].to_vec()
        })
        .collect();
    let frames: Vec<&[u8]> = requests.iter().map(Vec::as_slice).collect();
    assert_eq!(support::session(&frames), StatsSection::ALL.len());
}

/// Found by the `setup` fuzz target: a valid label, matching codes and a
/// 255-byte SSID and passphrase overflowed the driver's own answer buffer.
/// The feeder must refuse the oversized answer, not the driver panic.
#[test]
fn an_oversized_wifi_answer_is_refused() {
    let ssid = [b'a'; 255];
    let psk = "b".repeat(255);
    let wifi = setup::Wifi {
        ssid: &ssid,
        psk: &psk,
    };
    assert_eq!(
        support::setup_client(&[1, b'a'], 5, 5, Some(wifi)),
        Setup::Refused
    );
}
