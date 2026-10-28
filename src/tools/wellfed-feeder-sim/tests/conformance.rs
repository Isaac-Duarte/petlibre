//! The device conformance suite on the simulator's platform: the device
//! API's view of the simulated feeder answers as the firmware's does.

use std::sync::{Mutex, MutexGuard};

use wellfed_feeder::conformance::Rig;
use wellfed_feeder::feeder::Feeder;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::platform::SimPlatform;
use wellfed_hal::interlock::INTERLOCK;
use wellfed_hal::ota::{mailbox, signers};

/// The interlock and the update mailbox are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

struct SimRig {
    _serial: MutexGuard<'static, ()>,
    feeder: Box<Feeder<SimPlatform>>,
}

impl SimRig {
    /// A first boot on erased flash.
    fn start() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        INTERLOCK.reset();
        mailbox::reset();
        signers::reset();
        let random = harness::counter(0x31)();
        let platform = SimPlatform::open(&Flash::memory(), random, 1, 1, [0x11; 32], None)
            .expect("the stores open on erased flash");
        Self {
            _serial: serial,
            feeder: Box::new(Feeder::new(platform)),
        }
    }
}

impl Rig for SimRig {
    type Platform = SimPlatform;

    fn feeder(&mut self) -> &mut Feeder<SimPlatform> {
        &mut self.feeder
    }

    fn write(&mut self) {
        self.feeder.platform_mut().write();
    }

    fn claim_outputs(&mut self, claimed: bool) {
        if claimed {
            INTERLOCK
                .claim_feed(u32::MAX)
                .expect("no join or write runs");
        } else {
            INTERLOCK.release_feed();
        }
    }

    fn gesture(&mut self, gesture: wellfed_core::buttons::Gesture) {
        self.feeder.platform_mut().gesture_in(gesture);
    }
}

wellfed_feeder::conformance!(SimRig::start());
