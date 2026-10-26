//! The device conformance suite on the PLAF103's platform, against the SDK
//! fake: the board's mailboxes, with the feeder runtime's real settings
//! writer taking its turns on the fake's flash, as the simulator's stand-in
//! takes them on its own.

use std::sync::{Mutex, MutexGuard};

use wellfed_feeder::conformance::Rig;
use wellfed_feeder::feeder::Feeder;
use wellfed_feeder_runtime::writer::Writer;
use wellfed_feeder_runtime::{calibration, events, keys, network, time, traces};
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
        traces::reset();
        calibration::reset();
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

/// A server that denies the feeder is not asked again for a day, though
/// the clock, never set, needs the time on every pass.
#[test]
fn a_denying_server_is_left_alone_while_the_clock_needs_the_time() {
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
    let [(request, _, _)] = sent.as_slice() else {
        panic!("one request: {sent:?}");
    };
    let mut deny = ntp::serve(request, 1_792_674_000_000, 1_792_674_000_000, 2, 5).unwrap();
    deny[1] = 0;
    deny[12..16].copy_from_slice(b"DENY");
    fake::with(|h| h.udp_rx.push_back((deny.to_vec(), gateway, 123)));

    // A pass every 20 ms for 100 s, then one every minute to the day's end.
    let mut now = 10_020u32;
    while now < 110_000 {
        fake::with(|h| h.uptime_ms = now);
        rig.feeder.pass(now, &mut NoFeeds, |_| {});
        now += 20;
    }
    while now < 10_000 + ntp::DENIED_MS - 60_000 {
        fake::with(|h| h.uptime_ms = now);
        rig.feeder.pass(now, &mut NoFeeds, |_| {});
        now += 60_000;
    }
    assert!(clock::state().reading.is_none());
    let asked = fake::with(|h| h.udp_tx.len());
    assert_eq!(asked, 0, "asked a denying server {asked} times");
}

/// A feed's trace on the board's platform: recorded from the owner's
/// observations and sealed with its result, answered from RAM at once,
/// held back while a feed claims the outputs, appended to the storage
/// ring on the writer's next turn, and read from the ring once RAM has
/// lost it (a restart). A sealed trace counts as a queued write, so the
/// next feed's claim lets it go first.
#[test]
fn a_feed_trace_is_stored_on_the_writer_turn_and_read_back_after_a_restart() {
    use wellfed_core::feed::controller::MotorCommand;
    use wellfed_core::feed::observe::Observation;
    use wellfed_device_api::Device;
    use wellfed_feeder::feeder::NoFeeds;
    use wellfed_feeder::trace::DriveFigures;
    use wellfed_fw_feeder::feeding::writes_queued;
    use wellfed_proto::api::{EventBody, FeedOutcome, FeedReason, Refusal, Source};
    use wellfed_proto::trace::PAGE_MAX;

    let mut rig = FirmwareRig::start();
    rig.write();
    let read = |rig: &mut FirmwareRig, feed: u32| -> Result<(Vec<u8>, bool), Refusal> {
        let mut body = Vec::new();
        let mut feeds = NoFeeds;
        let mut device = rig.feeder.device(0, &mut feeds);
        loop {
            let offset = u16::try_from(body.len()).unwrap();
            let page = device.feed_trace(feed, offset)?;
            assert!(page.bytes.len() <= PAGE_MAX);
            body.extend_from_slice(page.bytes);
            if body.len() == usize::from(page.total) {
                return Ok((body, page.stored));
            }
        }
    };

    assert!(traces::begin(9));
    assert_eq!(read(&mut rig, 9), Err(Refusal::Busy), "still recorded");
    for poll in 0..600u32 {
        traces::observe(&Observation {
            at_ms: 1_000 + 30 * poll,
            drive_at_ms: Some(1_000),
            current: Some(60 + poll % 9),
            battery_mv: None,
            beams: None,
            rising: 0,
            falling: u16::from(poll > 300),
            last_edge: (poll > 300).then_some((1_000 + 9_000, false)),
            motor: MotorCommand::Drive0,
        });
    }
    let fed = EventBody::Fed {
        feed: 9,
        requested: 1,
        completed: 0,
        outcome: FeedOutcome::Failed,
        reason: FeedReason::NoFeedback,
        by: Source::Panel,
    };
    let figures = DriveFigures {
        drive_ms: 18_000,
        samples: 600,
        ..DriveFigures::default()
    };
    let summary = traces::seal(&fed, &figures).expect("the trace of feed 9");
    assert_eq!(summary.drive_ms, 18_000);
    let (in_ram, stored) = read(&mut rig, 9).unwrap();
    assert!(!stored && in_ram.len() > PAGE_MAX);
    assert!(writes_queued(), "a sealed trace goes before the next feed");

    // A feed holds the outputs: the append waits for them.
    INTERLOCK
        .claim_feed(u32::MAX)
        .expect("no join or write runs");
    rig.write();
    assert!(traces::queued() && traces::fill().kept == 0);
    INTERLOCK.release_feed();
    rig.write();
    assert!(!traces::queued() && !writes_queued());
    assert_eq!(traces::fill().kept, 1);
    assert_eq!(read(&mut rig, 9), Ok((in_ram.clone(), true)));

    // A restart loses RAM; the ring still has it.
    traces::reset();
    assert_eq!(read(&mut rig, 9), Ok((in_ram, true)));
    assert_eq!(read(&mut rig, 10), Err(Refusal::NotFound));
}
