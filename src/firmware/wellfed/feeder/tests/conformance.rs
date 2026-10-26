//! The device conformance suite on the PLAF103's platform, against the SDK
//! fake: the board's mailboxes, with the feeder runtime's real settings
//! writer taking its turns on the fake's flash, as the simulator's stand-in
//! takes them on its own.

use std::sync::{Mutex, MutexGuard};

use wellfed_feeder::conformance::Rig;
use wellfed_feeder::feeder::Feeder;
use wellfed_feeder_runtime::writer::Writer;
use wellfed_feeder_runtime::{events, keys, network, time};
use wellfed_fw_feeder::platform::Plaf103;
use wellfed_sdk::hal::flash::park::Mmio;
use wellfed_sdk::hal::flash_lock::arming::Hooks;
use wellfed_sdk::hal::flash_lock::provider::Provider;
use wellfed_sdk::hal::interlock::INTERLOCK;
use wellfed_sdk::hal::ota::mailbox;
use wellfed_sdk::hal::sdk::{self, fake};
use wellfed_sdk::hal::{bringup, clock, rng, settings};
use wellfed_sdk::storage::FlashOps;

/// The writer runs with the hooks already armed, as after confirmation.
struct NoMmio;

impl Mmio for NoMmio {
    const KM0: bool = false;
    fn load(_: u32) -> u32 {
        unreachable!("no arming with the hooks armed")
    }
    fn store(_: u32, _: u32) {
        unreachable!("no arming with the hooks armed")
    }
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

/// The fake's flash through hooks that are armed.
struct Armed;

impl Hooks for Armed {
    fn watchdog() -> bool {
        true
    }
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn armed() -> bool {
        true
    }
}

impl FlashOps for Armed {
    fn install(_: *mut Provider) -> bool {
        true
    }
    fn read_id() -> [u8; 3] {
        [0; 3]
    }
    fn armed() -> bool {
        true
    }
    fn erase(sector: u32) -> bool {
        // SAFETY: the fake's storage, which this test owns.
        unsafe { bringup::storage_erase(sector) }
    }
    fn program(sector: u32, offset: u32, data: &[u8]) -> bool {
        // SAFETY: as in `erase`.
        unsafe { bringup::storage_program(sector, offset, data) }
    }
    fn read(sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::storage_read(sector, offset, bytes)
    }
}

/// The mailboxes are process-wide; the fake hardware is per thread.
static SERIAL: Mutex<()> = Mutex::new(());

/// Writer steps per turn: each step does one thing (a store's open, one
/// change, one event), so this many leaves nothing waiting in the suite.
const STEPS: usize = 8;

struct FirmwareRig {
    _serial: MutexGuard<'static, ()>,
    feeder: Box<Feeder<Plaf103>>,
    writer: Writer<Armed>,
    provider: Box<Provider>,
}

impl FirmwareRig {
    /// A first boot on erased flash, once the writer has opened its stores.
    fn start() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        fake::reset_hardware();
        settings::reset();
        // SAFETY: the test holds `SERIAL` and runs no Wi-Fi task.
        unsafe { network::reset() };
        keys::reset();
        events::reset();
        time::reset();
        clock::reset();
        mailbox::reset();
        INTERLOCK.reset();
        rng::reset();
        fake::with(|h| h.adc_noise = 0x1357_9bdf);
        assert_eq!(rng::start().result, Ok(()));

        let mut rig = Self {
            _serial: serial,
            feeder: Box::new(Feeder::new(Plaf103::new())),
            writer: Writer::default(),
            provider: Box::default(),
        };
        while events::epoch().is_none() {
            rig.step();
        }
        rig
    }

    fn step(&mut self) {
        // SAFETY: the provider lives as long as the rig, at a fixed address.
        unsafe { self.writer.step::<NoMmio, Armed>(&mut *self.provider) };
    }
}

impl Rig for FirmwareRig {
    type Platform = Plaf103;

    fn feeder(&mut self) -> &mut Feeder<Plaf103> {
        &mut self.feeder
    }

    fn write(&mut self) {
        for _ in 0..STEPS {
            self.step();
        }
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
}

wellfed_feeder::conformance!(FirmwareRig::start());

/// The loop asks the lease's gateway on port 123 once DHCP gave an
/// address, and an answer to its request sets the clock; one with another
/// nonce, or from another address, does not.
#[test]
fn the_loop_asks_the_gateway_and_an_answer_sets_the_clock() {
    use wellfed_core::clock::ntp;
    use wellfed_feeder::feeder::NoFeeds;

    let mut rig = FirmwareRig::start();
    let gateway = u32::from_be_bytes([10, 42, 103, 1]);
    fake::with(|h| {
        h.ipv4 = u32::from_be_bytes([10, 42, 103, 172]);
        h.gateway = gateway;
        h.uptime_ms = 10_000;
    });
    rig.feeder.pass(10_000, &mut NoFeeds, |_| {});
    let sent = fake::with(|h| core::mem::take(&mut h.udp_tx));
    let [(request, ip, port)] = sent.as_slice() else {
        panic!("one request: {sent:?}");
    };
    assert_eq!((*ip, *port), (gateway, ntp::PORT));
    assert_eq!(request[0], 0x23, "a version 4 client request");

    // 2026-10-22T13:00:00Z.
    let now = 1_792_674_000_000;
    let answer = ntp::serve(request, now, now, 2, 5).unwrap();
    let mut forged = answer;
    forged[24] ^= 1;
    fake::with(|h| {
        h.udp_rx
            .push_back((answer.to_vec(), u32::from_be_bytes([10, 42, 103, 9]), 123));
        h.udp_rx.push_back((forged.to_vec(), gateway, 123));
        h.uptime_ms += 20;
    });
    rig.feeder.pass(10_020, &mut NoFeeds, |_| {});
    assert_eq!(clock::state().reading, None, "neither is an answer");

    fake::with(|h| {
        h.udp_rx.push_back((answer.to_vec(), gateway, 123));
        h.uptime_ms += 20;
    });
    rig.feeder.pass(10_040, &mut NoFeeds, |_| {});
    let reading = clock::state().reading.expect("the clock is set");
    assert!(reading.unix_ms.abs_diff(now) < 100, "{reading:?}");
    assert_eq!(reading.offset_s, None, "no zone, no client sync");
}
