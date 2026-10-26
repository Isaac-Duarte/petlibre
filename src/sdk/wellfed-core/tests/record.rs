//! The host tools' view of the firmware's UART records.

use wellfed_core::record::{Record, tag};

#[test]
fn records_parse_after_sdk_noise() {
    let record = Record::parse("[sdk] x PLBLE1 request op=wifi ok=1\r").unwrap();
    assert_eq!((record.tag, record.first), (tag::BLE, "request"));
    assert!(record.is(tag::BLE, &[("op", "wifi"), ("ok", "1")]));
    assert!(!record.is(tag::BLE, &[("ok", "0")]));
    assert_eq!(record.body(), "request op=wifi ok=1");

    let start = Record::parse("PLFW1 start build=ab bank=1").unwrap();
    assert_eq!(start.tag, tag::START);
    assert_eq!(start.number::<u32>("bank"), Some(1));
    assert_eq!(start.number::<u32>("build"), None);

    let saved = Record::parse("PLCFG1 net saved generation=4 stored=1").unwrap();
    assert!(saved.body_starts("net saved"));

    for noise in [
        "PLAIN text",
        "PLBLE1request",
        "PLX1 short",
        "PLABCDE1 long",
        "nothing",
    ] {
        assert!(Record::parse(noise).is_none(), "{noise}");
    }
}

#[test]
fn records_sharing_a_line_each_keep_their_own_fields() {
    let line = "PLWIF3 ev seq=3 kind=dns PLOTA1 progress sectors=48/170 leases=816";
    let records: Vec<_> = Record::all(line).collect();

    assert_eq!(records.len(), 2);
    assert_eq!((records[0].tag, records[0].first), ("PLWIF3", "ev"));
    assert_eq!(records[0].field("leases"), None);
    assert_eq!(records[1].tag, tag::UPDATE);
    assert_eq!(records[1].field("sectors"), Some("48/170"));
    assert_eq!(Record::parse(line), Some(records[0]));
}

#[test]
fn a_field_matches_its_whole_key() {
    let record = Record::parse("PLCNF1 confirmed=1 healthy_ms=30000").unwrap();
    assert_eq!(record.field("healthy_ms"), Some("30000"));
    assert_eq!(record.field("healthy"), None);
    assert_eq!(record.field("ms"), None);
}

#[test]
fn a_panic_record_names_its_location() {
    let record =
        Record::parse("PLPNC1 panic file=src/sdk/wellfed-hal/src/owner/state.rs line=212 col=9")
            .unwrap();

    assert_eq!((record.tag, record.first), (tag::PANIC, "panic"));
    assert_eq!(
        record.field("file"),
        Some("src/sdk/wellfed-hal/src/owner/state.rs")
    );
    assert_eq!(record.number::<u32>("line"), Some(212));
    assert_eq!(record.number::<u32>("col"), Some(9));
}
