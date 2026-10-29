//! A consumer image's first boot on the simulator, in virtual time: with no
//! build Wi-Fi and none stored it starts BLE setup mode itself, the setup
//! exchange `wellfed setup` runs (`wellfed_client::setup`) pairs over the
//! simulator's BLE link and stores the home Wi-Fi, and the boot after that
//! starts no setup. Until then the panel's Dispense feeds while no phone
//! runs a session, and the idle reboot waits for that feed; a setup the
//! owner started with Lock and Mic still refuses it.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_client::setup::{Ask, Credentials, Session, Step};
use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_device_api::mailbox::{KeyResult, WifiResult};
use wellfed_feeder::feeder::platform::BuildWifi;
use wellfed_feeder::feeder::setup::{self, Cause, Ended};
use wellfed_feeder::feeder::{self, Platform, feeding, owner};
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::sim::{Record, Sim};
use wellfed_model::setup::{Enrolled, Reply, WifiSaved};
use wellfed_proto::api::Refusal;
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

/// A simulated consumer feeder across its boots, its uptime this boot, and
/// what its loop recorded this boot.
struct Feeder {
    flash: Rc<Flash>,
    banks: Rc<RefCell<Banks>>,
    boots: u32,
    build_wifi: BuildWifi,
    sim: Sim,
    now: u32,
    records: Vec<Record>,
}

impl Feeder {
    /// A new feeder, as stock leaves it after installing a consumer image.
    fn first_boot() -> Self {
        Self::new(BuildWifi::Absent)
    }

    /// A new feeder from an image with or without build Wi-Fi.
    fn new(build_wifi: BuildWifi) -> Self {
        let flash = Flash::memory();
        let banks = Banks::new();
        let sim = boot(&flash, &banks, 1, build_wifi);
        Self {
            flash,
            banks,
            boots: 1,
            build_wifi,
            sim,
            now: 0,
            records: Vec::new(),
        }
    }

    /// The restart the feeder asked for, onto the same flash.
    fn reboot(&mut self) {
        assert!(self.sim.take_reboot(), "the feeder asked for a restart");
        self.boots += 1;
        self.sim = boot(&self.flash, &self.banks, self.boots, self.build_wifi);
        self.now = 0;
        self.records.clear();
    }

    /// The owner presses `button` once, and the loop runs a while.
    fn press(&mut self, button: Button) {
        let chord = Buttons::of(button);
        self.sim.gesture(Gesture::Down { chord });
        self.sim.gesture(Gesture::Up {
            chord,
            level: 0,
            held_ms: 80,
        });
        self.run(100);
    }

    /// The owner holds `buttons` together, as the console's `hold` does.
    fn hold(&mut self, buttons: &[Button]) {
        let mut chord = Buttons::of(buttons[0]);
        for &button in buttons {
            chord = chord.with(button);
            self.sim.gesture(Gesture::Down { chord });
        }
        self.sim.gesture(Gesture::Hold { chord, level: 1 });
        self.sim.gesture(Gesture::Up {
            chord,
            level: 1,
            held_ms: 5_000,
        });
        self.run(100);
    }

    /// The panel's Dispense presses this boot, by their result.
    fn dispensed(&self) -> Vec<Result<u32, Refusal>> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Feeder(feeder::Record::Feeding(feeding::Record::Dispense { result })) => {
                    Some(*result)
                }
                _ => None,
            })
            .collect()
    }

    /// Run the loop for `ms`, a pass at least every second.
    fn run(&mut self, ms: u32) {
        let end = self.now + ms;
        while self.now < end {
            let records = &mut self.records;
            let delay = self.sim.pass(self.now, |record| records.push(record));
            self.now += delay.clamp(1, 1000).min(end - self.now);
        }
    }

    /// The setup starts this boot recorded, by cause.
    fn starts(&self) -> Vec<Cause> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Feeder(feeder::Record::Owner(owner::Record::Setup(
                    setup::Record::Start { cause },
                ))) => Some(*cause),
                _ => None,
            })
            .collect()
    }

    /// The phone writes `message`, and the loop runs a while.
    fn write(&mut self, message: &[u8]) {
        self.sim.platform().ble_write(message);
        self.run(200);
    }

    /// The one message the feeder notified since the last read.
    fn notified(&mut self) -> Vec<u8> {
        let [message] = self
            .sim
            .platform()
            .ble_notified()
            .try_into()
            .expect("one notification");
        message
    }

    /// The owner keys `code` in on the panel, a press at a time.
    fn key_in(&mut self, code: Code) {
        for &button in code.presses() {
            self.press(button);
        }
    }
}

fn boot(flash: &Rc<Flash>, banks: &Rc<RefCell<Banks>>, count: u32, wifi: BuildWifi) -> Sim {
    let random = harness::counter(0x33)();
    let mut sim = Sim::open(flash, banks, None, random, 0, count, None).expect("the feeder opens");
    sim.platform().set_build_wifi(wifi);
    sim
}

#[test]
fn a_consumer_first_boot_starts_setup_and_pairing_ends_it_for_good() {
    let mut feeder = Feeder::first_boot();
    feeder.run(1_000);
    assert_eq!(feeder.starts(), [Cause::Unprovisioned]);
    assert!(feeder.sim.platform().ble_advertising());

    // The phone runs the setup exchange up to its code.
    let client = Keypair::from_secret([7; 32]);
    let ask = Ask {
        label: "phone".into(),
        wifi: Some(Credentials::new(b"home-net".to_vec(), "pass word!".into()).unwrap()),
    };
    let code = Code::from_bits(0b01_11_00_10_10_00_11_01);
    let mut session = Session::new(&client, [3; 32], ask, code, [5; 32]).unwrap();
    assert!(feeder.sim.platform().ble_connect());
    feeder.run(200);
    feeder.write(&session.start().unwrap());
    let Step::Send(third) = session.receive(&feeder.notified()).unwrap() else {
        panic!("the phone answers the feeder's handshake")
    };
    feeder.write(&third);
    assert_eq!(
        session.receive(&feeder.notified()).unwrap(),
        Step::Reply(Reply::EnterCode { seconds: 60 })
    );

    // The owner keys the code in; the phone and the feeder prove it.
    feeder.key_in(code);
    let reply = loop {
        match session.receive(&feeder.notified()).unwrap() {
            Step::Send(answer) => feeder.write(&answer),
            Step::Reply(reply) => break reply,
        }
    };
    assert_eq!(
        reply,
        Reply::Done {
            enrolled: Enrolled::New,
            wifi: WifiSaved::Saved,
        }
    );
    assert_eq!(session.feeder_key(), Some(feeder.sim.identity().unwrap()));
    assert!(feeder.records.contains(&Record::Keys(KeyResult::Done)));
    assert!(
        feeder.dispensed().iter().all(Result::is_err),
        "no press of the code fed"
    );
    let ended = setup::Record::Reboot { ended: Ended::Done };
    assert!(
        feeder
            .records
            .contains(&Record::Feeder(feeder::Record::Owner(
                owner::Record::Setup(ended)
            )))
    );

    // It reboots without BLE, and the next boot has a network to join.
    feeder.run(setup::REBOOT_MS + 1_000);
    feeder.reboot();
    feeder.run(5_000);
    assert_eq!(feeder.starts(), []);
    assert!(!feeder.sim.platform().ble_advertising());
    let WifiResult::Current { view, .. } = feeder.sim.platform().wifi() else {
        panic!("the simulator's Wi-Fi store is loaded")
    };
    assert!(view.stored_ssid_crc.is_some());
}

#[test]
fn an_idle_first_boot_reboots_into_setup_again() {
    let mut feeder = Feeder::first_boot();
    feeder.run(1_000);
    assert_eq!(feeder.starts(), [Cause::Unprovisioned]);

    feeder.run(setup::IDLE_MS + 1_000);
    feeder.reboot();
    feeder.run(1_000);
    assert_eq!(feeder.starts(), [Cause::Unprovisioned]);
}

#[test]
fn dispense_feeds_in_an_unprovisioned_setup_with_no_session() {
    let mut feeder = Feeder::first_boot();
    feeder.run(1_000);
    feeder.hold(&[Button::Lock]);
    feeder.press(Button::Dispense);
    feeder.run(30_000);
    assert!(matches!(feeder.dispensed()[..], [Ok(_)]), "one feed");
    assert!(feeder.sim.platform().ble_advertising(), "setup goes on");
}

#[test]
fn dispense_feeds_nothing_while_a_phone_runs_a_session() {
    let mut feeder = Feeder::first_boot();
    feeder.run(1_000);
    feeder.hold(&[Button::Lock]);
    let client = Keypair::from_secret([7; 32]);
    let ask = Ask {
        label: "phone".into(),
        wifi: None,
    };
    let code = Code::from_bits(0b01_11_00_10_10_00_11_01);
    let mut session = Session::new(&client, [3; 32], ask, code, [5; 32]).unwrap();
    assert!(feeder.sim.platform().ble_connect());
    feeder.run(200);
    feeder.write(&session.start().unwrap());

    feeder.press(Button::Dispense);
    assert_eq!(feeder.dispensed(), [Err(Refusal::Busy)]);
}

#[test]
fn the_idle_reboot_waits_for_a_feed_in_flight() {
    let mut feeder = Feeder::first_boot();
    feeder.run(1_000);
    feeder.sim.stall_feeds(true);
    feeder.run(setup::IDLE_MS - 2_000);
    feeder.hold(&[Button::Lock]);
    feeder.press(Button::Dispense);
    assert!(matches!(feeder.dispensed()[..], [Ok(_)]));

    feeder.run(10_000);
    assert!(
        feeder
            .records
            .contains(&Record::Feeder(feeder::Record::Owner(
                owner::Record::Setup(setup::Record::Reboot { ended: Ended::Idle })
            )))
    );
    assert!(!feeder.sim.take_reboot(), "the stalled feed holds it");

    feeder.sim.stall_feeds(false);
    feeder.run(30_000);
    feeder.reboot();
}

#[test]
fn lock_and_mic_setup_still_refuses_dispense() {
    let mut feeder = Feeder::new(BuildWifi::Present);
    feeder.run(1_000);
    feeder.hold(&[Button::Lock]);
    feeder.hold(&[Button::Lock, Button::Mic]);
    assert_eq!(feeder.starts(), [Cause::Hold]);

    feeder.press(Button::Dispense);
    assert_eq!(feeder.dispensed(), [Err(Refusal::Busy)]);
}
