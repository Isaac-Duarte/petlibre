//! FW47's mDNS responder against an independent DNS codec (`simple-dns`):
//! the answers a browser and a resolver ask for, legacy unicast, the
//! announcements, and packets it must ignore.

use wellfed_core::mdns::{Destination, PACKET, PORT, Responder};
use simple_dns::rdata::RData;
use simple_dns::{CLASS, Name, Packet, QCLASS, QTYPE, Question, TYPE};

const KEY: [u8; 32] = [0xab; 32];
const IP: u32 = 0x0a2a_6705;

fn responder() -> Responder {
    let mut responder = Responder::new(&KEY, 4747);
    responder.set_ip(Some(IP), 0);
    responder
}

fn query(name: &str, qtype: TYPE, unicast: bool, id: u16) -> Vec<u8> {
    let mut packet = Packet::new_query(id);
    let mut question = Question::new(
        Name::new_unchecked(name),
        QTYPE::TYPE(qtype),
        QCLASS::CLASS(CLASS::IN),
        unicast,
    );
    question.unicast_response = unicast;
    packet.questions.push(question);
    packet.build_bytes_vec().unwrap()
}

/// Every record in a response, as (name, kind, value).
fn records(bytes: &[u8]) -> (Packet<'_>, Vec<(String, String)>) {
    let packet = Packet::parse(bytes).unwrap();
    let mut out = Vec::new();
    for record in packet.answers.iter().chain(&packet.additional_records) {
        let value = match &record.rdata {
            RData::PTR(ptr) => format!("PTR {}", ptr.0),
            RData::SRV(srv) => format!("SRV {} {}", srv.port, srv.target),
            RData::TXT(txt) => {
                let mut pairs: Vec<_> = txt
                    .attributes()
                    .into_iter()
                    .map(|(k, v)| format!("{k}={}", v.unwrap_or_default()))
                    .collect();
                pairs.sort();
                format!("TXT {}", pairs.join(" "))
            }
            RData::A(a) => format!("A {:#x}", a.address),
            other => format!("{other:?}"),
        };
        out.push((record.name.to_string(), value));
    }
    (packet, out)
}

#[test]
fn a_browse_gets_the_service_its_host_and_address() {
    let r = responder();
    assert_eq!(r.label(), "wellfed-abababab");
    let mut out = [0; PACKET];
    let reply = r
        .query(
            &query("_wellfed._tcp.local", TYPE::PTR, false, 7),
            PORT,
            &mut out,
        )
        .unwrap();
    assert_eq!(reply.to, Destination::Multicast);

    let (packet, found) = records(&out[..reply.len]);
    assert_eq!(packet.id(), 0, "multicast answers carry id 0");
    assert_eq!(packet.answers.len(), 1);
    let instance = "wellfed-abababab._wellfed._tcp.local";
    assert_eq!(
        found,
        [
            ("_wellfed._tcp.local".to_owned(), format!("PTR {instance}")),
            (
                instance.to_owned(),
                "SRV 4747 wellfed-abababab.local".to_owned()
            ),
            (
                instance.to_owned(),
                format!("TXT id={} v=1", "ab".repeat(32))
            ),
            (
                "wellfed-abababab.local".to_owned(),
                "A 0xa2a6705".to_owned()
            ),
        ]
    );
}

#[test]
fn host_and_instance_lookups_are_answered_case_insensitively() {
    let r = responder();
    let mut out = [0; PACKET];
    let reply = r
        .query(
            &query("Wellfed-ABABABAB.local", TYPE::A, true, 1),
            PORT,
            &mut out,
        )
        .unwrap();
    assert_eq!(reply.to, Destination::Unicast, "the QU bit");
    let (packet, found) = records(&out[..reply.len]);
    assert_eq!(packet.answers.len(), 1);
    assert_eq!(found[0].1, "A 0xa2a6705");

    let reply = r
        .query(
            &query(
                "wellfed-abababab._wellfed._tcp.local",
                TYPE::SRV,
                false,
                1,
            ),
            PORT,
            &mut out,
        )
        .unwrap();
    let (packet, _) = records(&out[..reply.len]);
    assert_eq!(packet.answers.len(), 1);

    let reply = r
        .query(
            &query("_services._dns-sd._udp.local", TYPE::PTR, false, 1),
            PORT,
            &mut out,
        )
        .unwrap();
    let (_, found) = records(&out[..reply.len]);
    assert_eq!(found[0].1, "PTR _wellfed._tcp.local");
}

#[test]
fn a_legacy_query_gets_a_unicast_answer_with_its_id_and_question() {
    let r = responder();
    let mut out = [0; PACKET];
    let reply = r
        .query(
            &query("wellfed-abababab.local", TYPE::A, false, 0x1234),
            40_000,
            &mut out,
        )
        .unwrap();
    assert_eq!(reply.to, Destination::Unicast);
    let packet = Packet::parse(&out[..reply.len]).unwrap();
    assert_eq!(packet.id(), 0x1234);
    assert_eq!(packet.questions.len(), 1);
    assert!(packet.answers.iter().all(|a| a.ttl <= 10 && !a.cache_flush));
}

#[test]
fn other_names_responses_and_garbage_are_ignored() {
    let r = responder();
    let mut out = [0; PACKET];
    for packet in [
        query("_googlecast._tcp.local", TYPE::PTR, false, 1),
        query("wellfed-00000000.local", TYPE::A, false, 1),
        query("_wellfed._tcp.local", TYPE::A, false, 1),
        vec![0; 5],
        // A pointer that points at itself.
        vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xc0, 12, 0, 12, 0, 1],
    ] {
        assert_eq!(r.query(&packet, PORT, &mut out), None);
    }

    // Our own announcement, looped back, is a response.
    let mut r = responder();
    let len = r.announce(0, &mut out).unwrap();
    let echo = out[..len].to_vec();
    assert_eq!(r.query(&echo, PORT, &mut out), None);

    // No address, no answers.
    let mut r = responder();
    r.set_ip(None, 0);
    assert_eq!(
        r.query(
            &query("_wellfed._tcp.local", TYPE::PTR, false, 1),
            PORT,
            &mut out
        ),
        None
    );
}

#[test]
fn every_address_change_is_announced_twice_a_second_apart() {
    let mut r = Responder::new(&KEY, 4747);
    let mut out = [0; PACKET];
    assert_eq!(r.announce(0, &mut out), None, "no address yet");

    r.set_ip(Some(IP), 100);
    let len = r.announce(100, &mut out).unwrap();
    let (packet, found) = records(&out[..len]);
    assert_eq!((packet.answers.len(), found.len()), (4, 4));
    assert_eq!(r.announce(500, &mut out), None);
    assert!(r.announce(1_100, &mut out).is_some());
    assert_eq!(r.announce(5_000, &mut out), None, "two only");

    r.set_ip(Some(IP + 1), 6_000);
    assert!(r.announce(6_000, &mut out).is_some());
}
