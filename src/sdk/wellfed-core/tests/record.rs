//! The host tools' view of the firmware's UART records.

use wellfed_core::feed::owner::OwnerState;
use wellfed_core::ota::install::{RefusedReason, StopStage};
use wellfed_core::record::{
    Api, Confirm, Event, Owner, Record, Settings, Tag, Trial, Update, Watch,
};

fn event(line: &str) -> Option<Event<'_>> {
    Record::parse(line)
        .unwrap_or_else(|| panic!("no record in {line:?}"))
        .event()
}

#[test]
fn records_parse_after_sdk_noise() {
    let record = Record::parse("[sdk] x PLBLE1 drop reason=protocol\r").unwrap();
    assert_eq!((record.kind(), record.first), (Some(Tag::Ble), "drop"));
    assert_eq!(record.field("reason"), Some("protocol"));
    assert_eq!(record.body(), "drop reason=protocol");

    let start = Record::parse("PLFW1 start build=ab bank=1").unwrap();
    assert_eq!(start.kind(), Some(Tag::Start));
    assert_eq!(start.number::<u32>("bank"), Some(1));
    assert_eq!(start.number::<u32>("build"), None);

    for noise in [
        "PLAIN text",
        "PLBLE1request",
        "PLX1 short",
        "PLABCDEF1 long",
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
    assert_eq!(records[0].kind(), None);
    assert_eq!(records[0].field("leases"), None);
    assert_eq!(records[1].kind(), Some(Tag::Update));
    assert_eq!(records[1].field("sectors"), Some("48/170"));
    assert_eq!(Record::parse(line), Some(records[0]));
}

/// A five-letter tag ends the record before it rather than joining its
/// body.
#[test]
fn a_panel_record_ends_the_one_before_it() {
    let line = "PLOWN1 state=1 fault=0 supervisor=0 stopped=0 optics_off=0 PLPANEL1 key=0";
    let records: Vec<_> = Record::all(line).collect();

    assert_eq!(records.len(), 2);
    assert_eq!(records[1].tag, "PLPANEL1");
    assert_eq!(records[0].field("key"), None);
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
    assert_eq!(
        event("PLPNC1 panic file=src/sdk/wellfed-hal/src/owner/state.rs line=212 col=9"),
        Some(Event::Panic {
            file: "src/sdk/wellfed-hal/src/owner/state.rs",
            line: Some(212),
        })
    );
}

#[test]
fn startup_and_trial_records_decode() {
    assert_eq!(
        event("PLFW1 start build=AB12 bank=1 reset=00000000 mmu=0,0,0"),
        Some(Event::Start {
            build: "AB12",
            bank: Some(1),
        })
    );
    assert_eq!(
        event("PLFW1 start build=ab bank=unknown"),
        Some(Event::Start {
            build: "ab",
            bank: None,
        })
    );
    assert_eq!(event("PLFW1 result diagnostic=pass backend=unopened"), None);

    assert_eq!(
        event("PLTRL1 confirmed=0 boot=2 limit=3"),
        Some(Event::Trial(Trial::Booting {
            boot: 2,
            limit: Some(3),
        }))
    );
    assert_eq!(
        event("PLTRL1 confirmed=1"),
        Some(Event::Trial(Trial::Confirmed))
    );
    assert_eq!(
        event("PLTRL1 give_way=1 marker=00 reset=1"),
        Some(Event::Trial(Trial::GiveWay))
    );
    assert_eq!(
        event("PLTRL1 confirmed=1 resets=2"),
        Some(Event::Trial(Trial::Confirmed))
    );
    assert_eq!(event("PLTRL1 bank=0"), None);
    assert_eq!(event("PLTRL1 bank=0 resets=1"), None);
    // A reset storm: KM0 gives way (the give-way line follows), or holds.
    assert_eq!(
        event("PLTRL1 storm=5 limit=5"),
        Some(Event::Trial(Trial::Storm {
            resets: 5,
            held: false
        }))
    );
    assert_eq!(
        event("PLTRL1 storm=6 held=1"),
        Some(Event::Trial(Trial::Storm {
            resets: 6,
            held: true
        }))
    );
}

#[test]
fn update_records_decode_their_words() {
    assert_eq!(
        event("PLOTA1 refused reason=not-installing"),
        Some(Event::Update(Update::Refused {
            reason: RefusedReason::NotInstalling,
        }))
    );
    assert_eq!(
        event("PLOTA1 error stage=image sector=1"),
        Some(Event::Update(Update::Stopped {
            stage: StopStage::Image,
        }))
    );
    assert_eq!(event("PLOTA1 error stage=unheard-of"), None);
    assert_eq!(
        event("PLWCH1 error stage=give_way"),
        Some(Event::Watch(Watch::GiveWayRefused))
    );
    assert_eq!(
        event("PLCNF1 confirmed=1 already=1"),
        Some(Event::Confirm(Confirm::Confirmed { healthy_ms: None }))
    );
}

#[test]
fn settings_owner_and_api_records_decode() {
    assert_eq!(
        event("PLCFG1 net saved generation=4 stored=1"),
        Some(Event::Settings(Settings::NetSaved { stored: true }))
    );
    assert_eq!(
        event("PLCFG1 keys change=enroll result=ok enrolled=1"),
        Some(Event::Settings(Settings::KeysChanged))
    );
    assert_eq!(
        event("PLCFG1 net open status=0 generation=1 stored=1"),
        None
    );

    assert_eq!(
        event("PLOWN1 state=4 fault=0 supervisor=0 stopped=0 optics_off=0"),
        Some(Event::Owner(Owner::State {
            state: OwnerState::Feeding,
            stopped: false,
        }))
    );
    assert_eq!(
        event("PLOWN1 feed portions=2 held=1"),
        Some(Event::Owner(Owner::Held))
    );
    assert_eq!(
        event("PLOWN1 feed id=7 portions=2 submit=0"),
        Some(Event::Owner(Owner::Submitted {
            id: 7,
            accepted: true
        }))
    );
    assert_eq!(
        event("PLOWN1 feed id=7 portions=2 submit=4"),
        Some(Event::Owner(Owner::Submitted {
            id: 7,
            accepted: false
        }))
    );

    assert_eq!(
        event("PLAPI1 session slot=0 state=ended reason=noise"),
        Some(Event::Api(Api::Session {
            reason: Some("noise"),
        }))
    );
}

/// The tags as the firmware prints them; some of its lines name the tag as
/// a literal (`c"PLOWN1 stall begin=1"`), so a renamed tag fails here.
#[test]
fn tags_are_unchanged() {
    let words: Vec<_> = Tag::ALL.iter().map(|tag| tag.as_str()).collect();
    assert_eq!(
        words,
        [
            "PLFW1", "PLTRL1", "PLCNF1", "PLWCH1", "PLOTA1", "PLCFG1", "PLTIM1", "PLBLE1",
            "PLOWN1", "PLAPI1", "PLPNC1"
        ]
    );
}

/// The owner's numbers, as `state=` and `fault=` print them, decode back.
#[test]
fn owner_codes_round_trip() {
    use wellfed_core::feed::owner::OwnerFault;

    for code in 0..9 {
        let state = OwnerState::from_code(code).unwrap();
        assert_eq!(state.code(), code);
    }
    assert_eq!(OwnerState::from_code(9), None);
    for code in 0..8 {
        let fault = OwnerFault::from_code(code).unwrap();
        assert_eq!(fault.code(), code);
    }
    assert_eq!(OwnerFault::from_code(8), None);
}
