//! Setup mode in the loop, against the fake BLE peripheral and the real
//! client session (`petlibre-client`), with the settings writer played by the
//! test: an ask whose code the owner keys in and the client proves enrolls
//! the client's key and saves its Wi-Fi before `Done` and the reboot; an
//! unanswered, abandoned or wrongly keyed ask enrolls nothing. The speaker
//! ticks for each press and plays the outcome, unless `sound` is off.

use std::sync::{Mutex, MutexGuard};

use petlibre_client::setup::{
    Ask, Credentials as ClientCredentials, Error as ClientError, Session, Step as ClientStep, frame,
};
use petlibre_core::ble_setup::{PREFIX, parse_prefix};
use petlibre_core::buttons::{Button, Buttons, Gesture};
use petlibre_core::noise::Keypair;
use petlibre_core::setup_code::{Code, PRESSES};
use petlibre_core::store::clients::Role;
use petlibre_core::store::net::NetStore;
use petlibre_core::tone::Tune;
use petlibre_core::transport::SettingsView;
use petlibre_feeder::pairing::{Config, Event, Pairing, Path, Refusal as PairRefusal};
use petlibre_feeder::panel::Lock;
use petlibre_fw_feeder::setup::{Ended, PROVE_MS, Record, Setup};
use petlibre_proto::setup::{Enrolled, Refusal, Reply, WifiSaved};
use petlibre_sdk::hal::keys::{self, Change, KeyResult};
use petlibre_sdk::hal::power::{self, Monitor};
use petlibre_sdk::hal::sdk::fake::{self, ble_record};
use petlibre_sdk::hal::sdk::{Advertising, BleEvent};
use petlibre_sdk::hal::{audio, ble, network, settings};

/// The mailboxes are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

struct Bench {
    _serial: MutexGuard<'static, ()>,
    setup: Setup,
    pairing: Pairing,
    identity: Keypair,
    power: Monitor,
    records: Vec<Record>,
}

impl Bench {
    /// Setup mode just started, its stack ready, on mains.
    fn start() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fake::reset_hardware();
        keys::reset();
        network::reset();
        power::reset();
        settings::reset();
        audio::reset();
        fake::with(|h| h.input[0] |= 1 << 19);
        petlibre_sdk::hal::rng::reset();
        fake::with(|h| h.adc_noise = 0x1357_9bdf);
        assert_eq!(petlibre_sdk::hal::rng::start().result, Ok(()));
        publish(0, None);

        let mut bench = Self {
            _serial: serial,
            setup: Setup::new(0),
            pairing: Pairing::new(Config::DEFAULT),
            identity: Keypair::from_secret([1; 32]),
            power: Monitor::default(),
            records: Vec::new(),
        };
        bench.step(None);
        ble_record(BleEvent::READY, 0, 0);
        bench.step(None);
        assert!(matches!(
            bench.records.as_slice(),
            [Record::Ble(ble::Record::Ready { .. })]
        ));
        bench.records.clear();
        bench
    }

    fn now(&self) -> u32 {
        fake::with(|h| h.uptime_ms)
    }

    fn step(&mut self, event: Option<Event>) {
        self.power.step();
        let now = self.now();
        let records = &mut self.records;
        let delay = self.setup.step(
            now,
            &mut self.pairing,
            event,
            Some(&self.identity),
            &mut |r| records.push(r),
        );
        fake::with(|h| h.uptime_ms += delay.min(100));
        fake::play_audio(2);
    }

    /// Settings loaded, with `sound` as given.
    fn sound(&mut self, sound: bool) {
        settings::publish(SettingsView {
            lights: true,
            sound,
            volume: 40,
            generation: 1,
            writable: true,
        });
    }

    /// The tunes the speaker starts over `n` steps.
    fn listen(&mut self, n: usize) -> Vec<Tune> {
        self.steps(n);
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Audio(audio::Record::Play { tune, .. }) => Some(*tune),
                _ => None,
            })
            .collect()
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step(None);
        }
    }

    /// A host connects and subscribes.
    fn connect(&mut self) {
        ble_record(BleEvent::CONNECT, 0, 12);
        ble_record(BleEvent::NOTIFY, 0, 1);
        self.steps(3);
        assert!(self.records.contains(&Record::Session));
    }

    /// Run the client up to its `EnterCode`.
    fn ask(&mut self, client: &Keypair) -> Session {
        let ask = Ask {
            label: "phone".into(),
            wifi: Some(ClientCredentials::new(b"Lab-B".to_vec(), "pass word!".into()).unwrap()),
        };
        let code = Code::from_bits(0b01_11_00_10_10_00_11_01);
        let mut session = Session::new(client, [3; 32], ask, code, [5; 32]).unwrap();
        self.connect();
        write(&session.start().unwrap());
        self.steps(3);
        let [second] = notified().try_into().unwrap();
        let ClientStep::Send(third) = session.receive(&second).unwrap() else {
            panic!("the client answers")
        };
        write(&third);
        self.steps(3);
        assert!(self.records.contains(&Record::Asked));
        let [entering] = notified().try_into().unwrap();
        assert_eq!(
            session.receive(&entering).unwrap(),
            ClientStep::Reply(Reply::EnterCode { seconds: 60 })
        );
        session
    }

    /// The owner keys `code` in; pairing's event for the last press.
    fn key_in(&mut self, code: Code) -> Option<Event> {
        let now = self.now();
        let mut last = None;
        for &button in code.presses() {
            let press = Gesture::Down {
                chord: Buttons::of(button),
            };
            last = self.pairing.gesture(now, press, Lock::Locked);
            self.setup.pressed();
        }
        last
    }

    /// Hand the loop the keyed-in code, then carry the proof between it and
    /// the client until either side stops; the client's error, if any.
    fn prove(&mut self, session: &mut Session, entered: Option<Event>) -> Option<ClientError> {
        assert!(matches!(entered, Some(Event::Entered { .. })));
        self.step(entered);
        assert!(self.records.contains(&Record::Proving));
        loop {
            let [step] = notified().try_into().unwrap();
            let answer = match session.receive(&step) {
                Ok(ClientStep::Send(answer)) => answer,
                Ok(ClientStep::Reply(reply)) => panic!("no verdict mid-proof: {reply:?}"),
                Err(error) => return Some(error),
            };
            write(&answer);
            self.steps(3);
            if self
                .records
                .iter()
                .any(|r| matches!(r, Record::Proven { .. }))
            {
                return None;
            }
        }
    }

    /// The client's own code, keyed in and proven.
    fn confirm(&mut self, session: &mut Session) {
        let entered = self.key_in(session.code());
        assert!(self.prove(session, entered).is_none());
        assert!(self.records.contains(&Record::Proven { wifi: true }));
        assert_eq!(session.feeder_key(), Some(*self.identity.public()));
    }

    fn press(&mut self) -> Option<Event> {
        let now = self.now();
        let press = Gesture::Down {
            chord: Buttons::of(Button::Dispense),
        };
        self.pairing.gesture(now, press, Lock::Locked)
    }
}

/// What the settings writer would publish after opening or saving.
fn publish(generation: u64, credentials: Option<petlibre_core::store::net::Credentials>) {
    use petlibre_core::store::slot::SlotIo;

    /// The credential sectors of the fake storage, read-only as the join
    /// half reads them.
    struct FakeNet;

    impl SlotIo for FakeNet {
        fn unit_bytes(&self) -> u32 {
            petlibre_sdk::hal::sdk::SECTOR_BYTES
        }
        fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
            petlibre_sdk::hal::sdk::storage_read(network::NET_BASE + slot, offset, bytes)
        }
        fn program(&self, _: u32, _: u32, _: &[u8]) -> bool {
            false
        }
        fn erase(&self, _: u32) -> bool {
            false
        }
        fn writable(&self) -> bool {
            false
        }
    }

    let mut store = NetStore::default();
    store.open(FakeNet);
    store.generation = generation;
    store.credentials = credentials;
    network::publish(&store, true);
}

fn write(message: &[u8]) {
    for piece in frame(message).unwrap().chunks(20) {
        fake::with(|h| h.ble_control.extend(piece));
    }
}

fn notified() -> Vec<Vec<u8>> {
    let bytes = fake::with(|h| std::mem::take(&mut h.ble_notified));
    let mut out = Vec::new();
    let mut rest = &bytes[..];
    while rest.len() >= PREFIX {
        let len = parse_prefix([rest[0], rest[1]]).unwrap();
        out.push(rest[PREFIX..PREFIX + len].to_vec());
        rest = &rest[PREFIX + len..];
    }
    out
}

#[test]
fn a_proven_ask_enrolls_saves_the_wifi_and_reboots() {
    let mut bench = Bench::start();
    bench.step(None);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::On);

    let client = Keypair::from_secret([2; 32]);
    let mut session = bench.ask(&client);
    bench.confirm(&mut session);

    // The settings writer enrolls the key.
    let Some(Change::Enroll { key, label, role }) = keys::pending() else {
        panic!("an enrollment is queued")
    };
    assert_eq!(
        (key, label.as_bytes(), role),
        (*client.public(), &b"phone"[..], Role::Owner)
    );
    keys::finish(KeyResult::Done);
    bench.step(None);
    assert!(bench.records.contains(&Record::Enroll {
        result: KeyResult::Done
    }));

    // Then saves the Wi-Fi.
    let Some(Some(credentials)) = network::pending() else {
        panic!("the credentials are queued")
    };
    assert_eq!(credentials.ssid(), b"Lab-B");
    network::finish();
    publish(1, Some(credentials));
    bench.step(None);

    let done = Reply::Done {
        enrolled: Enrolled::New,
        wifi: WifiSaved::Saved,
    };
    let [reply] = notified().try_into().unwrap();
    assert_eq!(session.receive(&reply).unwrap(), ClientStep::Reply(done));
    assert!(
        bench
            .records
            .contains(&Record::Reboot { ended: Ended::Done })
    );

    // Advertising stops, and the reboot comes once the reply had time.
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);
    assert!(!bench.setup.reboot_due(bench.now()));
    fake::with(|h| h.uptime_ms += petlibre_fw_feeder::setup::REBOOT_MS);
    assert!(bench.setup.reboot_due(bench.now()));
}

#[test]
fn an_unanswered_ask_is_refused_and_enrolls_nothing() {
    let mut bench = Bench::start();
    let mut session = bench.ask(&Keypair::from_secret([2; 32]));

    fake::with(|h| h.uptime_ms += Config::DEFAULT.confirm_ms);
    let timeout = bench.pairing.tick(bench.now());
    assert_eq!(
        timeout,
        Some(Event::Refused {
            path: Path::Setup,
            refusal: PairRefusal::Unconfirmed
        })
    );
    bench.step(timeout);

    let [reply] = notified().try_into().unwrap();
    assert_eq!(
        session.receive(&reply).unwrap(),
        ClientStep::Reply(Reply::Refused(Refusal::Unconfirmed))
    );
    assert_eq!(keys::pending(), None);
    assert_eq!(
        fake::with(|h| h.ble_disconnects),
        0,
        "the client may hang up first"
    );
    fake::with(|h| h.uptime_ms += petlibre_fw_feeder::setup::LINGER_MS);
    bench.step(None);
    assert_eq!(fake::with(|h| h.ble_disconnects), 1);
    assert!(!bench.setup.reboot_due(bench.now()), "setup mode stays");
}

#[test]
fn a_wrong_code_enrolls_nothing_and_shares_no_wifi() {
    let mut bench = Bench::start();
    let mut session = bench.ask(&Keypair::from_secret([2; 32]));
    let mut presses = *session.code().presses();
    presses[5] = if presses[5] == Button::Lock {
        Button::Reset
    } else {
        Button::Lock
    };
    let entered = bench.key_in(Code::new(presses));

    // The client stops at the feeder's first opening, before its Wi-Fi.
    assert!(matches!(
        bench.prove(&mut session, entered),
        Some(ClientError::WrongCode)
    ));
    assert_eq!(session.feeder_key(), None);

    // The feeder gives up on the stalled proof and stays in setup mode.
    fake::with(|h| h.uptime_ms += PROVE_MS);
    bench.step(None);
    assert!(bench.records.contains(&Record::Replied {
        reply: Reply::Refused(Refusal::Unconfirmed)
    }));
    assert_eq!(keys::pending(), None);
    assert_eq!(network::pending(), None);
    assert!(!bench.setup.reboot_due(bench.now()));
}

#[test]
fn a_client_that_leaves_cancels_its_ask() {
    let mut bench = Bench::start();
    bench.ask(&Keypair::from_secret([2; 32]));
    ble_record(BleEvent::DISCONNECT, 0x13, 0);
    bench.steps(2);
    assert_eq!(bench.press(), None, "the press goes to the panel again");
}

#[test]
fn a_foreign_first_message_drops_the_host() {
    let mut bench = Bench::start();
    bench.connect();
    write(&[0; 5]);
    bench.steps(3);
    assert!(bench.records.contains(&Record::Ble(ble::Record::Drop {
        reason: ble::DropReason::Protocol
    })));
}

#[test]
fn it_advertises_only_on_mains_and_reboots_when_idle() {
    let mut bench = Bench::start();
    fake::with(|h| h.input[0] &= !(1 << 19));
    bench.step(None);
    assert_eq!(fake::with(|h| h.ble_advertising), Advertising::Off);

    fake::with(|h| h.uptime_ms += petlibre_fw_feeder::setup::IDLE_MS);
    bench.step(None);
    assert!(
        bench
            .records
            .contains(&Record::Reboot { ended: Ended::Idle })
    );
    assert!(bench.setup.reboot_due(bench.now()));
}

#[test]
fn a_proven_ask_completes_even_if_the_client_leaves_and_returns() {
    let mut bench = Bench::start();
    let mut session = bench.ask(&Keypair::from_secret([2; 32]));
    bench.confirm(&mut session);
    assert!(matches!(keys::pending(), Some(Change::Enroll { .. })));

    // The client leaves and a new one connects while the writer works.
    ble_record(BleEvent::DISCONNECT, 0x13, 0);
    bench.steps(2);
    bench.records.clear();
    ble_record(BleEvent::CONNECT, 0, 12);
    ble_record(BleEvent::NOTIFY, 0, 1);
    bench.steps(3);
    assert!(!bench.records.contains(&Record::Session), "no new session");
    assert!(bench.records.contains(&Record::Fault {
        fault: petlibre_fw_feeder::setup::Fault::Busy
    }));

    keys::finish(KeyResult::Done);
    bench.step(None);
    let Some(Some(credentials)) = network::pending() else {
        panic!("the Wi-Fi still follows the enrollment")
    };
    network::finish();
    publish(1, Some(credentials));
    bench.step(None);
    assert!(
        bench
            .records
            .contains(&Record::Reboot { ended: Ended::Done })
    );
}

#[test]
fn the_owner_hears_each_press_then_the_outcome() {
    let mut bench = Bench::start();
    bench.sound(true);
    let mut session = bench.ask(&Keypair::from_secret([2; 32]));
    bench.confirm(&mut session);
    keys::finish(KeyResult::Done);
    bench.step(None);
    let Some(Some(credentials)) = network::pending() else {
        panic!("the credentials are queued")
    };
    network::finish();
    publish(1, Some(credentials));

    let mut expected = vec![Tune::Key; PRESSES];
    expected.push(Tune::Coin);
    assert_eq!(bench.listen(200), expected);
    assert!(!fake::with(|h| h.audio_open), "the codec closes after");
}

#[test]
fn a_wrong_code_sounds_the_alert_and_sound_off_mutes_it() {
    for sound in [true, false] {
        let mut bench = Bench::start();
        bench.sound(sound);
        let mut session = bench.ask(&Keypair::from_secret([2; 32]));
        let mut presses = *session.code().presses();
        presses[2] = if presses[2] == Button::Mic {
            Button::Dispense
        } else {
            Button::Mic
        };
        let entered = bench.key_in(Code::new(presses));
        assert!(bench.prove(&mut session, entered).is_some());
        fake::with(|h| h.uptime_ms += PROVE_MS);

        let mut expected = vec![Tune::Key; PRESSES];
        expected.push(Tune::Alert);
        let heard = bench.listen(200);
        assert_eq!(heard, if sound { expected } else { Vec::new() });
    }
}

#[test]
fn a_new_client_replaces_an_ask_whose_link_vanished() {
    let mut bench = Bench::start();
    bench.ask(&Keypair::from_secret([2; 32]));

    // The next client connects with no disconnect seen for the first.
    bench.records.clear();
    let mut session = bench.ask(&Keypair::from_secret([3; 32]));
    bench.confirm(&mut session);
}
