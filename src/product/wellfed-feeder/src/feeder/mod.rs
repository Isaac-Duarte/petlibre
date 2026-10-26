//! [`Feeder`]: the feeder's loop, one [`Feeder::pass`] at a time, over a
//! [`Platform`]. The PLAF103 firmware runs it in its loop task and the
//! simulator in its serve loop, so both serve the same panel, pairing, BLE
//! setup and device API.
//!
//! A pass, in order:
//! 1. the identity, if the settings writer published a new one
//!    ([`identity`]);
//! 2. the panel's owner: gestures, a client's queued lock change, the idle
//!    relock, pairing's timers, the lock indicator and BLE setup mode while
//!    it runs ([`owner`], [`setup`]); then the reboot setup mode asks for,
//!    and the lock it now holds for the device API ([`lock`]);
//! 3. the device API: its sockets, the server's step and the live samples
//!    ([`serve`]).
//!
//! The feeding owner is the platform's: the firmware steps it before the
//! pass and hands it in as [`Feeds`], and records its `Fed` events through
//! [`Feeder::record_outcome`]. Every durable event goes through the
//! recorder ([`events`]).

pub mod events;
pub mod identity;
pub mod lock;
pub mod owner;
pub mod platform;
pub mod serve;
pub mod setup;

use core::fmt;

use wellfed_proto::api::{EventBody, Refusal, Source};
use wellfed_session::noise::Keypair;

pub use platform::Platform;

use identity::Identity;
use lock::LockBox;
use owner::Owner;
use wellfed_device_api::mailbox::Lock;
use wellfed_device_api::{Device, Server};
use platform::DriverRecord;
use serve::{FeederDevice, Inputs, Serve};

/// The feeding owner, as the device API reaches it. Generic rather than
/// `dyn`, so the calls stay direct for the firmware's stack audit.
pub trait Feeds {
    /// A feed is being prepared or driven: handshakes wait.
    fn feeding(&self) -> bool;
    /// A feed is queued or runs: no install starts.
    fn busy(&self) -> bool;
    /// Hand one feed to the owner; the number its journal holds it under.
    fn submit(&mut self, portions: u8, by: Source) -> Result<u32, Refusal>;
    /// Whether the feed `feed` is reserved yet: `None` while it waits.
    fn reserved(&self, feed: u32) -> Option<Result<(), Refusal>>;
    /// Stop the feed `feed` if it is still running.
    fn cancel(&self, feed: u32) -> bool;
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
        }
    }
}

pub struct Feeder<P: Platform> {
    platform: P,
    identity: Identity,
    owner: Owner,
    lock: LockBox,
    serve: Serve<P::Socket>,
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
        if self.owner.reboot_due(now_ms) {
            self.platform.reset();
        }

        // Requests read what the owner holds after this step.
        let lock = self.owner.lock();
        if let Some(lock) = lock {
            self.lock.publish(lock.locked());
        }
        let inputs = Inputs {
            now_ms,
            locked: lock.map(Lock::locked),
            recovery: self.owner.take_recovery(),
            signer: self.owner.take_signer(),
            pairing: self.owner.pairing(),
            identity: self.identity.keypair(),
            feeds,
        };
        let serve_delay = self
            .serve
            .step(&mut self.platform, &mut self.lock, inputs, |r| {
                record(Record::Serve(r));
            });
        delay.min(serve_delay)
    }

    /// Record a feed's `Fed` event, which may take the queue's reserved
    /// slot.
    pub fn record_outcome(&mut self, now_ms: u32, body: EventBody) {
        events::record_outcome(&mut self.platform, now_ms, body);
    }

    /// The device API's view of the feeder at `now_ms`, with the feeding
    /// owner `feeds`, as the server sees it in a pass.
    pub fn device<'a, F: Feeds>(&'a mut self, now_ms: u32, feeds: &'a mut F) -> impl Device + 'a {
        let boot = self.serve.boot(&mut self.platform, now_ms);
        FeederDevice {
            platform: &mut self.platform,
            lock: &mut self.lock,
            feeds,
            boot,
            locked: self.owner.lock().map(Lock::locked),
            now_ms,
        }
    }
}
