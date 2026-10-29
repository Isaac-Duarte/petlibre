//! A consumer image's first boot on the simulator, in virtual time: with no
//! build Wi-Fi and none stored it starts BLE setup mode itself, the setup
//! exchange `wellfed setup` runs (`wellfed_client::setup`) pairs over the
//! simulator's BLE link and stores the home Wi-Fi, and the boot after that
//! starts no setup.

use std::cell::RefCell;
use std::rc::Rc;

use wellfed_client::setup::{Ask, Credentials, Session, Step};
use wellfed_core::buttons::{Buttons, Gesture};
use wellfed_device_api::mailbox::{KeyResult, WifiResult};
use wellfed_feeder::feeder::platform::BuildWifi;
use wellfed_feeder::feeder::setup::{self, Cause, Ended};
use wellfed_feeder::feeder::{self, Platform, owner};
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::sim::{Record, Sim};
use wellfed_model::setup::{Enrolled, Reply, WifiSaved};
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

/// A simulated consumer feeder across its boots, its uptime this boot, and
/// what its loop recorded this boot.
struct Feeder {
    flash: Rc<Flash>,
    banks: Rc<RefCell<Banks>>,
    boots: u32,
    sim: Sim,
    now: u32,
    records: Vec<Record>,
}

impl Feeder {
    /// A new feeder, as stock leaves it after installing a consumer image.
    fn first_boot() -> Self {
        let flash = Flash::memory();
        let banks = Banks::new();
        let sim = boot(&flash, &banks, 1);
        Self {
            flash,
            banks,
            boots: 1,
            sim,
            now: 0,
            records: Vec::new(),
        }
    }

    /// The restart the feeder asked for, onto the same flash.
    fn reboot(&mut self) {
        assert!(self.sim.take_reboot(), "the feeder asked for a restart");
        self.boots += 1;
        self.sim = boot(&self.flash, &self.banks, self.boots);
        self.now = 0;
        self.records.clear();
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
            let chord = Buttons::of(button);
            self.sim.gesture(Gesture::Down { chord });
            self.sim.gesture(Gesture::Up {
                chord,
                level: 0,
                held_ms: 80,
            });
            self.run(100);
        }
    }
}

fn boot(flash: &Rc<Flash>, banks: &Rc<RefCell<Banks>>, count: u32) -> Sim {
    let random = harness::counter(0x33)();
    let mut sim = Sim::open(flash, banks, None, random, 0, count, None).expect("the feeder opens");
    sim.platform().set_build_wifi(BuildWifi::Absent);
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
