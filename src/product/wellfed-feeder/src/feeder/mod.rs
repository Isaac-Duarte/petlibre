//! [`Feeder`]: the feeder's loop, one [`Feeder::pass`] at a time, over a
//! [`Platform`]. The PLAF103 firmware runs it in its loop task and the
//! simulator in its serve loop, so both serve the same panel, pairing, BLE
//! setup and device API.
//!
//! A pass, in order:
//! 1. the identity, if the settings writer published a new one
//!    ([`identity`]);
//! 2. the panel's owner: gestures, a client's queued lock change, the idle
//!    relock, pairing's timers and BLE setup mode while it runs ([`owner`],
//!    [`setup`]); then the reboot setup mode asks for, and the lock it now
//!    holds for the device API ([`lock`]);
//! 3. the supply judged, its change recorded ([`supply`]); a Dispense
//!    press the panel allowed, queued for the feeding owner, and readiness
//!    noted ([`feeding`]); then the panel's lights from what the pass now
//!    knows ([`lights`]), and a press's beep ([`beep`]);
//! 4. the device API: its sockets, the server's step and the live samples
//!    ([`serve`]), with the time settings and the NTP exchange ([`time`]);
//! 5. the speaker, while a tune a client asked for plays (setup mode steps
//!    it itself). A feed plays none.
//!
//! The feeding owner is the platform's: the firmware steps it before the
//! pass and hands it in as [`Feeds`], and records its `Fed` events through
//! [`Feeder::record_outcome`]. Every durable event goes through the
//! recorder ([`events`]).

pub mod beep;
pub mod events;
pub mod feeding;
pub mod identity;
pub mod lights;
pub mod lock;
pub mod owner;
pub mod platform;
pub mod schedule;
pub mod serve;
pub mod setup;
pub mod supply;
pub mod time;

use core::fmt;

use wellfed_proto::api::{Blocked, EventBody, Phase, Readiness, Refusal, Source, TraceSummary};
use wellfed_session::noise::Keypair;

pub use platform::Platform;

use beep::Beeper;
use identity::Identity;
use lights::Lights;
use lock::LockBox;
use owner::Owner;
use platform::DriverRecord;
use schedule::Schedule;
use serve::{FeederDevice, Inputs, Serve};
use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::{Device, Server};

/// The feeding owner, as the device API reaches it. Generic rather than
/// `dyn`, so the calls stay direct for the firmware's stack audit.
pub trait Feeds {
    /// A feed is being prepared or driven: only IK session handshakes go
    /// on, and only until one is refused or fails.
    fn feeding(&self) -> bool;
    /// A feed is queued or runs: no install starts, and no restart.
    fn busy(&self) -> bool;
    /// The owner's own reason not to start a feed now, if any
    /// ([`feeding::readiness`] adds the loop's).
    fn blocked(&self) -> Option<Blocked>;
    /// Hand one feed to the owner; the number its journal holds it under.
    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal>;
    /// Whether the feed `feed` is reserved yet: `None` while it waits.
    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>>;
    /// Stop the feed `feed` if it is still running.
    fn cancel(&self, feed: u32) -> bool;
    /// The food beam's latest reading (true with food), once the owner has
    /// taken one; it reads the beam only while its emitters are on.
    fn food(&self) -> Option<bool> {
        None
    }
    /// Where the rotor rests and what the feeds' marks and outlet beam
    /// counted this boot, once the owner has opened.
    fn marks(&self) -> Option<Marks> {
        None
    }
    /// Whether the owner judges the battery pack below the floor a feed
    /// needs; `None` before it has judged, or with no owner.
    fn below_floor(&self) -> Option<bool> {
        None
    }
    /// The number of the newest feed the journal holds, once it is open:
    /// the next feed is reserved as the one after it.
    fn high_water(&self) -> Option<u32> {
        None
    }
}

/// The rotor's phase and this boot's mark counts, from the feeding owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marks {
    pub phase: Phase,
    /// Feeds that began with a homing mark, uncredited.
    pub homed: u32,
    /// Outlet-beam breaks seen during feeds.
    pub outlet_breaks: u32,
}

/// A feeder with no feeding owner: every feed is refused before it
/// reaches the motor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoFeeds;

impl Feeds for NoFeeds {
    fn feeding(&self) -> bool {
        false
    }

    fn busy(&self) -> bool {
        false
    }

    fn blocked(&self) -> Option<Blocked> {
        Some(Blocked::Unserved)
    }

    fn submit(&mut self, _: u8, _: Source) -> Result<u32, Refusal> {
        Err(Refusal::Unavailable)
    }

    fn reserved(&self, _: u32) -> Option<Result<(), Refusal>> {
        Some(Err(Refusal::Unavailable))
    }

    fn cancel(&self, _: u32) -> bool {
        false
    }
}

/// What one pass did, for the record lines. No record carries a key, a
/// secret, a credential or the setup code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record<D> {
    /// `PLID1`.
    Identity(identity::Record),
    /// `PLPANEL1`, `PLSET1`, or a setup driver's own tag.
    Owner(owner::Record<D>),
    /// `PLAPI1`.
    Serve(serve::Record),
    /// `PLFEED1`.
    Feeding(feeding::Record),
    /// `PLIND1`.
    Lights(lights::Record),
    /// `PLSCH1`.
    Schedule(schedule::Record),
    /// The speaker's own record while a requested tune plays, under its tag.
    Driver(D),
}

impl<D: DriverRecord> Record<D> {
    /// The line's tag.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Identity(_) => "PLID1",
            Self::Owner(owner::Record::Setup(setup::Record::Driver(record))) => record.tag(),
            Self::Owner(owner::Record::Setup(_)) => "PLSET1",
            Self::Owner(_) => "PLPANEL1",
            Self::Serve(_) => "PLAPI1",
            Self::Feeding(_) => "PLFEED1",
            Self::Lights(_) => "PLIND1",
            Self::Schedule(_) => "PLSCH1",
            Self::Driver(record) => record.tag(),
        }
    }
}

/// The line's body.
impl<D: DriverRecord> fmt::Display for Record<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Identity(record) => identity::describe(record, f),
            Self::Owner(record) => owner::describe(record, f),
            Self::Serve(record) => serve::describe(record, f),
            Self::Feeding(record) => feeding::describe(record, f),
            Self::Lights(record) => lights::describe(record, f),
            Self::Schedule(record) => schedule::describe(record, f),
            Self::Driver(record) => fmt::Display::fmt(&record, f),
        }
    }
}

pub struct Feeder<P: Platform> {
    platform: P,
    identity: Identity,
    owner: Owner,
    lock: LockBox,
    serve: Serve<P::Socket>,
    readiness: feeding::Watch,
    supply: supply::Watch,
    lights: Lights,
    beep: Beeper,
    schedule: Schedule,
}

impl<P: Platform> Feeder<P> {
    /// Const, so the firmware builds it in its static: the server's table
    /// and setup mode's state would not fit the loop's stack.
    pub const fn new(platform: P) -> Self {
        Self {
            platform,
            identity: Identity::new(),
            owner: Owner::new(),
            lock: LockBox::new(),
            serve: Serve::new(),
            readiness: feeding::Watch::new(),
            supply: supply::Watch::new(),
            lights: Lights::new(),
            beep: Beeper::new(),
            schedule: Schedule::new(),
        }
    }

    pub fn platform(&self) -> &P {
        &self.platform
    }

    pub fn platform_mut(&mut self) -> &mut P {
        &mut self.platform
    }

    /// The identity key pair, once the writer has published one.
    pub fn identity(&self) -> Option<&Keypair> {
        self.identity.keypair()
    }

    /// The panel's lock, once the first pass has run.
    pub fn lock(&self) -> Option<Lock> {
        self.owner.lock()
    }

    /// Whether BLE setup mode runs.
    pub fn setup_on(&self) -> bool {
        self.owner.setup_on()
    }

    /// The device API's server, for a host test to look into.
    pub fn server(&mut self) -> &mut Server {
        self.serve.server()
    }

    /// Whether the last pass ran a handshake operation, its costly part: a
    /// loop with a feed in flight steps the feeding owner again at once.
    pub fn handshook(&mut self) -> bool {
        self.serve.server().handshook()
    }

    /// Whether a feed can start now, with the feeding owner `feeds`.
    pub fn readiness<F: Feeds>(&self, feeds: &F) -> Readiness {
        feeding::held(self.ready(feeds), self.schedule.holds())
    }

    /// As [`Self::readiness`], without the schedule's own hold: what the
    /// schedule's feed is asked under.
    fn ready<F: Feeds>(&self, feeds: &F) -> Readiness {
        feeding::readiness(
            feeds,
            self.owner.setup_on(),
            self.platform.installing(),
            self.supply.holds(),
        )
    }

    /// One pass at `now_ms`, with the feeding owner `feeds`; `record`
    /// receives what happened, in order. The delay until the next pass.
    /// The caller reads `now_ms` before the pass, so it comes before the
    /// identity step (an X25519 derivation when a new identity arrives).
    pub fn pass<F: Feeds>(
        &mut self,
        now_ms: u32,
        feeds: &mut F,
        mut record: impl FnMut(Record<P::Record>),
    ) -> u32 {
        if let Some(r) = self.identity.step(&mut self.platform) {
            record(Record::Identity(r));
        }

        let remote = self.lock.take();
        let delay = self.owner.step(
            &mut self.platform,
            now_ms,
            remote,
            self.identity.keypair(),
            |r| record(Record::Owner(r)),
        );
        // Setup mode's restart waits for a feed in flight, which it would
        // otherwise leave uncertain; setup mode starts no feed itself.
        if self.owner.reboot_due(now_ms) && !feeds.busy() {
            // The writer's restart save takes this boot's counts as they are now.
            let counts = self.serve.stats().counts(now_ms);
            self.platform.hand_counts(counts);
            self.platform.reset();
        }

        // The boot's facts (and its `Boot` event) first, so the supply's
        // brownout hold is armed before the first readiness.
        let boot = self.serve.boot(&mut self.platform, now_ms);
        let reading = supply::supply(self.platform.supply(now_ms), feeds.below_floor());
        let reset = crate::stats::reset_cause(boot.reset_flags);
        if let Some(changed) = self.supply.note(now_ms, reset, reading) {
            events::record(&mut self.platform, now_ms, EventBody::Supply(changed));
        }

        if self.owner.take_dispense() {
            let ready = self.readiness(feeds);
            let portions = feeding::DISPENSE_PORTIONS;
            let result = feeding::queue(feeds, ready, portions, Source::Panel);
            record(Record::Feeding(feeding::Record::Dispense { result }));
            if result.is_err() {
                let reason = feeding::refused_reason(feeds, ready);
                self.readiness.refused(&mut self.platform, now_ms, reason);
                self.lights.refused(now_ms, |r| record(Record::Lights(r)));
            }
        }
        let ready = self.ready(feeds);
        self.schedule
            .step(&mut self.platform, feeds, now_ms, ready, |r| {
                record(Record::Schedule(r))
            });
        let ready = self.readiness(feeds);
        self.readiness.note(&mut self.platform, now_ms, ready, |r| {
            record(Record::Feeding(r))
        });

        let local_ms = self
            .platform
            .clock_state(now_ms)
            .reading
            .and_then(|r| r.local_ms());
        let states = self.states(feeds, ready, local_ms);
        let lights_delay = self.lights.step(&mut self.platform, now_ms, states, |r| {
            record(Record::Lights(r))
        });

        // Every press beeps when sound may play; not before the settings load.
        let pressed = self.owner.take_pressed();
        let allowed = self
            .platform
            .settings()
            .is_some_and(|view| view.sound_at(local_ms));
        let at_rest = !feeds.busy();
        let beep_delay = self
            .beep
            .step(&mut self.platform, now_ms, pressed, allowed, at_rest);

        // Requests read what the owner holds after this step.
        let lock = self.owner.lock();
        if let Some(lock) = lock {
            self.lock.publish(lock.locked());
        }
        self.serve.stats().presses(self.owner.presses());
        let inputs = Inputs {
            now_ms,
            setup: self.owner.setup_on(),
            locked: lock.map(Lock::locked),
            recovery: self.owner.take_recovery(),
            signer: self.owner.take_signer(),
            pairing: self.owner.pairing(),
            identity: self.identity.keypair(),
            supply: reading,
            battery_hold: self.supply.holds(),
            meal: self.schedule.holds(),
            feeds,
        };
        let serve_delay = self
            .serve
            .step(&mut self.platform, &mut self.lock, inputs, |r| {
                record(Record::Serve(r))
            });

        // After the owner and the device API, so playback never delays
        // either; setup mode steps the speaker itself.
        let mut speaker_delay = u32::MAX;
        if !self.owner.setup_on() && !self.platform.speaker_idle() {
            let step = self.platform.speaker_step();
            if let Some(r) = step.record {
                record(Record::Driver(r));
            }
            speaker_delay = step.delay_ms;
        }
        delay
            .min(serve_delay)
            .min(speaker_delay)
            .min(lights_delay)
            .min(beep_delay)
    }

    /// The feeder's states as the lights show them, after this pass's
    /// owner step and readiness, at the local wall time `local_ms`.
    fn states<F: Feeds>(
        &mut self,
        feeds: &F,
        ready: Readiness,
        local_ms: Option<i64>,
    ) -> lights::States {
        let blocked = feeds.blocked();
        lights::States {
            fault: matches!(blocked, Some(Blocked::Fault | Blocked::Journal)),
            updating: self.platform.installing(),
            setup: self.owner.setup_on(),
            pairing: self.owner.pairing().awaiting().is_some(),
            battery: ready == Readiness::Blocked(Blocked::Battery)
                || blocked == Some(Blocked::Battery),
            feeding: feeds.busy(),
            locked: self.owner.lock().is_some_and(Lock::locked),
            online: self.platform.ipv4().is_some(),
            // True until the settings load.
            lights: self
                .platform
                .settings()
                .is_none_or(|view| view.lights_at(local_ms)),
        }
    }

    /// Record a feed's `Fed` event, which may take the queue's reserved
    /// slot. A restart's uncertain feed that a plan had claimed is the
    /// plan's.
    pub fn record_outcome(&mut self, now_ms: u32, mut body: EventBody) {
        if let EventBody::Fed { feed, by, .. } = &mut body {
            *by = Schedule::source(&mut self.platform, *feed, *by);
        }
        self.serve.stats().fed(&body);
        events::record_outcome(&mut self.platform, now_ms, body);
    }

    /// A feed's trace was sealed: its figures join the statistics.
    pub fn traced(&mut self, summary: &TraceSummary) {
        self.serve.stats().traced(summary);
    }

    /// The last loop pass (the feeding owner's step and [`Self::pass`])
    /// took `us`, for the loop metric.
    pub fn pass_took(&mut self, us: u32) {
        self.serve.stats().pass_took(us);
    }

    /// The device API's view of the feeder at `now_ms`, with the feeding
    /// owner `feeds`, as the server sees it in a pass.
    pub fn device<'a, F: Feeds>(&'a mut self, now_ms: u32, feeds: &'a mut F) -> impl Device + 'a {
        let boot = self.serve.boot(&mut self.platform, now_ms);
        let ntp_last = self.serve.ntp_last();
        let (stats, tasks, page) = self.serve.stats_parts();
        FeederDevice {
            platform: &mut self.platform,
            lock: &mut self.lock,
            feeds,
            boot,
            locked: self.owner.lock().map(Lock::locked),
            setup: self.owner.setup_on(),
            now_ms,
            ntp_last,
            stats,
            tasks,
            page,
            battery_hold: self.supply.holds(),
            meal: self.schedule.holds(),
        }
    }
}
