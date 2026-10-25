//! The update stage, in the service task on either bank: it takes the
//! loop's update requests from the mailbox (`ota::mailbox`), checks them
//! against the update store and the keys, and on bank 0 installs into
//! bank 1. The service task has the stack the signature checks need, and
//! running the install there serializes its flash writes with the settings
//! writer's; the install lease keeps the feeding owner's journal out.
//!
//! **Bank 1** checks an offer, then bank 0's image: bank 0 must still hold
//! exactly the image it recorded at its last boot (`store::update`'s
//! `Bank0`), a build that installs over the device API. It then records the
//! offer as pending, answers [`Phase::GivingWay`], and once that answer is
//! out clears its own marker's first byte and restarts. The loader boots
//! bank 0.
//!
//! **Bank 0** records its own image at boot (only when it changed), and
//! installs only the pending manifest, or, with none pending (fresh from
//! stock, or after a failed trial), the one offered. It refuses a
//! manifest that was activated and failed its trial, and every offer after
//! a reset that came mid-install, until power is lost (the update-attempt
//! bit). Each sector is erased, programmed and read back under the flash
//! hooks; the activation is recorded in the store before the marker, which
//! goes last and is read back. Any stop before that leaves bank 1 invalid,
//! so the loader keeps booting bank 0.
//!
//! **No write while a feed may move.** Every store and bank write takes the
//! write turn (`interlock::WriteTurn`) first, which it gets only while the
//! loop holds no claim on the outputs, never by the forced path: a flash
//! lease while the outputs may move fails its stopped-outputs check and
//! resets the board. A request waits for the turn in the mailbox, unanswered;
//! an install holds it from its first write until it stops, so every feed
//! meanwhile is refused (`Held::Writing`); a give-way or an activation keeps
//! it until the restart. The install lease (`mailbox`) holds new feeds off
//! at the device API as well. Each write also runs under the attempt marker,
//! as the settings writer's do, set only once the turn is held, so a hang
//! inside it is skipped after the reset.

use core::fmt;
use core::marker::PhantomData;

use wellfed_core::ota::install::{Install, Phase, Progress, Refused, Stop, Taken};
use wellfed_core::ota::offer::{
    self, Admitted, Context, KeyId, KeyRing, KeyStatement, Trust, admit_offer, admit_statement,
};
use wellfed_core::ota::{BANK1_OFFSET, MARKER, SECTOR, SECTOR_BYTES, image};
use wellfed_core::store::clients::{CLIENTS, Client, Role};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::store::update::{Bank0, Candidate, State, UpdateStatus, UpdateStore};
use sha2::{Digest, Sha256};

use crate::flash_lock::arming::{self, Hooks};
use crate::interlock::{INTERLOCK, WRITER_POLL_MS, WriteStep, WriteTurn};
use crate::keys;
use crate::ota::mailbox::{self, Answer, Request};
use crate::sdk;

/// Between steps with nothing to do.
pub const IDLE_MS: u32 = 50;
/// While chunks come or a check runs.
pub const BUSY_MS: u32 = 1;
/// After its answer is read, a give-way or a restart waits this long for
/// the bytes to leave, and at most `LEAVE_MAX_MS` in all.
pub const LEAVE_MS: u32 = 1_000;
pub const LEAVE_MAX_MS: u32 = 5_000;
/// Bank 0's image is hashed this many sectors a step, so no step holds the
/// service task long.
pub const SCAN_SECTORS: u32 = 8;
/// Log progress every this many sectors.
pub const PROGRESS_SECTORS: u32 = 16;
/// One page program at a time, each read back.
const PIECE: usize = 256;
/// The bank-0 region, as `sdk::bank0_read` takes offsets.
const BANK0_BYTES: u32 = sdk::BANK1_OFFSET - sdk::BANK0_OFFSET;

/// Everything the stage writes or reads in flash outside the update store.
pub trait Flash {
    /// The flash hooks are armed: writes can succeed.
    fn armed(&self) -> bool;
    /// Bank 1 by sector, as `sdk::bank1_*` take it (from bank 0 only).
    fn erase(&mut self, sector: u32) -> bool;
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool;
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool;
    /// Bank 0 by byte offset, from either bank.
    fn bank0(&mut self, offset: u32, bytes: &mut [u8]) -> bool;
    /// Clear bank 1's first marker byte, from bank 1 with bank 0 valid.
    fn give_way(&mut self) -> bool;
}

/// The flash through the SDK wrappers, refused unless the hooks are armed
/// (an unarmed lock would fall through to the SDK's own KM0 handshake).
pub struct SdkFlash<H>(PhantomData<H>);

impl<H> SdkFlash<H> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<H> Default for SdkFlash<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Hooks> Flash for SdkFlash<H> {
    fn armed(&self) -> bool {
        H::armed()
    }
    fn erase(&mut self, sector: u32) -> bool {
        // SAFETY: the hooks are armed; the update stage owns bank 1, which
        // bank 0 alone writes.
        H::armed() && unsafe { sdk::bank1_erase(sector) }
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        // SAFETY: as in `erase`.
        H::armed() && unsafe { sdk::bank1_program(sector, offset, data) }
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::bank1_read(sector, offset, bytes)
    }
    fn bank0(&mut self, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::bank0_read(offset, bytes)
    }
    fn give_way(&mut self) -> bool {
        // SAFETY: the hooks are armed; the give-way clears only bank 1's
        // first marker byte, which the update stage owns.
        H::armed() && unsafe { sdk::bank1_give_way() }
    }
}

/// The update store's two sectors, through the SDK wrappers; writes only
/// with the hooks armed. Reads are mapped and need no lease.
pub struct StoreSlots<H>(PhantomData<fn() -> H>);

impl<H> StoreSlots<H> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }

    fn sector(slot: u32) -> Option<u32> {
        (slot < sdk::storage::UPDATE.count).then_some(sdk::storage::UPDATE.first + slot)
    }
}

impl<H> Clone for StoreSlots<H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> Copy for StoreSlots<H> {}

impl<H> Default for StoreSlots<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Hooks> SlotIo for StoreSlots<H> {
    fn unit_bytes(&self) -> u32 {
        sdk::SECTOR_BYTES
    }
    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        Self::sector(slot).is_some_and(|sector| sdk::storage_read(sector, offset, bytes))
    }
    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        // SAFETY: the hooks are armed; the update store's sectors are the
        // update stage's alone.
        H::armed()
            && Self::sector(slot)
                .is_some_and(|sector| unsafe { sdk::storage_program(sector, offset, bytes) })
    }
    fn erase(&self, slot: u32) -> bool {
        // SAFETY: as in `program`.
        H::armed() && Self::sector(slot).is_some_and(|sector| unsafe { sdk::storage_erase(sector) })
    }
    fn writable(&self) -> bool {
        H::armed()
    }
}

/// The build's update-signing public key, from `WELLFED_OTA_PUBLIC_KEY`
/// (64 lowercase hex characters).
pub fn public_key() -> Option<[u8; 32]> {
    wellfed_session::hex::decode_array(option_env!("WELLFED_OTA_PUBLIC_KEY")?.as_bytes())
}

/// What the stage logs: `PLOTA1` for the install, `PLWCH1` for bank 1's
/// side, as the bench has always read them. No record carries a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Opened {
        status: UpdateStatus,
        min_version: u32,
        /// A reset came mid-install this power cycle.
        attempt: bool,
    },
    /// Bank 0's image, as recorded (`recorded`: written this boot).
    Bank0 {
        len: u32,
        recorded: bool,
    },
    /// Bank 0's headers did not give an image to record.
    Bank0Unknown,
    Refused {
        refused: Refused,
    },
    Offer {
        version: u32,
    },
    GaveWay {
        version: u32,
    },
    /// Bank 0 cannot take the offer; bank 1 stays.
    GiveWayRefused,
    Manifest {
        version: u32,
        bytes: u32,
    },
    Progress {
        sectors: u32,
        total: u32,
    },
    Activated {
        version: u32,
        bytes: u32,
        sectors: u32,
    },
    Stopped {
        stop: Stop,
    },
    Key {
        statement: KeyStatement,
    },
}

impl Record {
    /// The record's line tag.
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Offer { .. } | Self::GaveWay { .. } | Self::GiveWayRefused => "PLWCH1",
            _ => "PLOTA1",
        }
    }
}

const fn stage(stop: Stop) -> &'static str {
    match stop {
        Stop::Timeout => "timeout",
        Stop::Cancelled => "cancelled",
        Stop::Order => "order",
        Stop::Marker => "image",
        Stop::Write => "write",
        Stop::Hash => "hash",
        Stop::Activate => "marker",
        Stop::Store => "store",
    }
}

const fn refused(refused: Refused) -> &'static str {
    use wellfed_core::ota::offer::OfferError;
    match refused {
        Refused::Offer(OfferError::Downgrade) => "downgrade",
        Refused::Offer(OfferError::Same) => "same",
        Refused::Offer(OfferError::Key(_)) | Refused::Statement(_) => "key",
        Refused::Offer(OfferError::Signature) => "signature",
        Refused::Offer(OfferError::Statement(_)) => "statement",
        Refused::Offer(_) => "manifest",
        Refused::Store(_) | Refused::Unreadable => "store",
        Refused::Busy => "busy",
        Refused::Fallback => "fallback",
        Refused::Pending => "pending",
        Refused::Failed => "failed",
        Refused::Attempt => "attempt",
        Refused::Installing => "installing",
        Refused::NotInstalling => "not-installing",
    }
}

/// The body of a record's line, after its tag.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Opened {
            status,
            min_version,
            attempt,
        } => write!(
            f,
            "update store={} min_version={min_version} attempt={}",
            match status {
                UpdateStatus::Loaded => "loaded",
                UpdateStatus::Empty => "empty",
                UpdateStatus::ReadOnly => "newer",
                UpdateStatus::Io | UpdateStatus::Unopened => "unreadable",
            },
            u8::from(attempt)
        ),
        Record::Bank0 { len, recorded } => {
            write!(f, "bank0 bytes={len} recorded={}", u8::from(recorded))
        }
        Record::Bank0Unknown => f.write_str("bank0 bytes=0 recorded=0"),
        Record::Refused { refused: why } => write!(f, "refused reason={}", refused(why)),
        Record::Offer { version } => write!(f, "offer=1 version={version}"),
        Record::GaveWay { version } => write!(f, "give_way=1 version={version} restart=1"),
        Record::GiveWayRefused => f.write_str("error stage=give_way"),
        Record::Manifest { version, bytes } => {
            write!(f, "manifest version={version} bytes={bytes}")
        }
        Record::Progress { sectors, total } => write!(f, "progress sectors={sectors}/{total}"),
        Record::Activated {
            version,
            bytes,
            sectors,
        } => write!(
            f,
            "activated version={version} bytes={bytes} sectors={sectors}"
        ),
        Record::Stopped { stop } => write!(f, "error stage={}", stage(stop)),
        Record::Key { statement } => match statement {
            KeyStatement::Revoke(id) => {
                write!(f, "key retired={}", wellfed_session::hex::Lower(&id))
            }
            KeyStatement::Add(key) => write!(
                f,
                "key added={}",
                wellfed_session::hex::Lower(&offer::key_id(&key))
            ),
        },
    }
}

/// What confirming did to the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Floor {
    /// Bank 0 activated this build: the floor is at least its version now.
    Raised { version: u32 },
    /// Nothing names this build (installed over UART, or confirmed before).
    Unchanged,
    /// The store could not be read or written.
    Failed,
}

/// `restart` asks the glue to let the watchdog reset the platform: bank 1
/// gave way, or bank 0 activated it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    pub restart: bool,
}

const fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        restart: false,
    }
}

/// Bank 0's image being hashed.
#[derive(Clone)]
struct Scan {
    len: u32,
    at: u32,
    hasher: Sha256,
}

impl Scan {
    /// Bank 0's image from its headers, or `None` when they do not give one.
    fn start(flash: &mut impl Flash) -> Option<Self> {
        let len = image::extent(|at, header| flash.bank0(at, header), BANK0_BYTES)?;
        Some(Self {
            len,
            at: 0,
            hasher: Sha256::new(),
        })
    }

    /// Hash the next sectors; the digest once the image is done, or why not.
    fn step(&mut self, flash: &mut impl Flash) -> Option<Result<[u8; 32], ()>> {
        let mut sector = [0u8; SECTOR];
        for _ in 0..SCAN_SECTORS {
            if self.at >= self.len {
                return Some(Ok(self.hasher.clone().finalize().into()));
            }
            let len = (self.len - self.at).min(SECTOR_BYTES) as usize;
            if !flash.bank0(self.at, &mut sector[..len]) {
                return Some(Err(()));
            }
            self.hasher.update(&sector[..len]);
            self.at += SECTOR_BYTES;
        }
        None
    }
}

#[derive(Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "one value in the stage's static, never moved on a stack"
)]
enum Work {
    Idle,
    /// Bank 1: an admitted offer waits for bank 0's image check.
    Checking {
        admitted: Admitted,
        scan: Scan,
    },
    /// Restart once the last answer is out: giving way (bank 1) or into
    /// the activated bank 1 (bank 0).
    Leaving {
        since: u32,
        give_way: bool,
    },
    /// Given way or activated: the restart is the glue's. Nothing more
    /// runs, and the write turn stays held, so no feed starts before it.
    Restarting {
        give_way: bool,
    },
}

/// Keep at a fixed address once started: the install's sector buffer is
/// larger than any stack wants to move.
pub struct Update<I> {
    bank: u8,
    identity: [u8; 32],
    key: Option<[u8; 32]>,
    io: I,
    store: UpdateStore<I>,
    opened: bool,
    /// A reset came mid-install this power cycle.
    attempt: bool,
    /// Bank 0 still has its image to record.
    record_bank0: bool,
    scan: Option<Scan>,
    work: Work,
    /// The write turn, and whether the stage holds it.
    turn: WriteTurn,
    holding: bool,
    install: Option<Install>,
    stopped: Option<Stop>,
    sectors: u32,
}

impl<I: SlotIo + Copy> Update<I> {
    /// The stage for the build `identity` running from `bank`, trusting the
    /// built-in `key`.
    pub fn new(bank: u8, identity: [u8; 32], key: Option<[u8; 32]>, io: I) -> Self {
        Self {
            bank,
            identity,
            key,
            io,
            store: UpdateStore::default(),
            opened: false,
            attempt: false,
            record_bank0: bank == 0,
            scan: None,
            work: Work::Idle,
            turn: WriteTurn::new(),
            holding: false,
            install: None,
            stopped: None,
            sectors: 0,
        }
    }

    fn open(&mut self) -> Record {
        self.store.open(self.io);
        self.opened = true;
        self.attempt = self.bank == 0 && arming::update_attempted();
        self.publish();
        Record::Opened {
            status: self.store.status,
            min_version: self.store.record().min_version,
            attempt: self.attempt,
        }
    }

    fn min(&self) -> u32 {
        self.store.record().min_version
    }

    fn progress(&self) -> Progress {
        let idle = Progress {
            stopped: self.stopped,
            ..Progress::idle(self.bank, self.min())
        };
        match (&self.work, &self.install) {
            (Work::Checking { admitted, .. }, _) => idle.of(Phase::Checking, admitted),
            (_, Some(install)) => install.progress(self.bank, self.min()),
            (Work::Leaving { give_way: true, .. } | Work::Restarting { give_way: true }, None) => {
                let mut progress = idle;
                if let State::Pending(c) = self.store.record().state {
                    progress.phase = Phase::GivingWay;
                    progress.digest = Some(c.digest);
                    progress.version = c.version;
                }
                progress
            }
            _ => idle,
        }
    }

    fn publish(&self) {
        mailbox::publish(&self.progress());
    }

    /// The build confirmed itself: raise the floor if bank 0 activated it.
    /// On [`Floor::Failed`] the build must not confirm: the floor would
    /// stay behind it. Called under the confirmation's write turn and
    /// attempt marker, so it takes neither itself.
    pub fn raise_floor(&mut self) -> Floor {
        if !self.opened {
            self.open();
        }
        if !self.store.readable() {
            return Floor::Failed;
        }
        match self.store.record().state {
            State::Activated(c) if c.build_id == self.identity => {
                match self.store.confirm(&self.identity) {
                    Ok(version) => {
                        self.publish();
                        Floor::Raised { version }
                    }
                    Err(_) => Floor::Failed,
                }
            }
            _ => Floor::Unchanged,
        }
    }

    /// Take the write turn, or keep it; false while a feed holds the
    /// outputs or another writer runs. Never forced: the stage writes only
    /// when the outputs are not claimed at all.
    fn take_turn(&mut self, now: u32) -> bool {
        if !self.holding {
            self.holding = self.turn.begin(&INTERLOCK, now, || false) != WriteStep::Wait;
        }
        self.holding
    }

    /// Give the turn back once nothing of the stage's needs it: no request
    /// being answered, no install, no check, no restart on its way.
    fn settle_turn(&mut self) {
        let busy = self.install.is_some() || !matches!(self.work, Work::Idle);
        if self.holding && !busy && mailbox::pending().is_none() {
            self.holding = false;
            INTERLOCK.end_write();
        }
    }

    /// One step; `now` is uptime in milliseconds.
    pub fn step(&mut self, flash: &mut impl Flash, now: u32) -> Step {
        const { assert!(IDLE_MS <= WRITER_POLL_MS) };
        if !self.opened {
            return next(BUSY_MS, Some(self.open()));
        }
        if self.record_bank0
            && let Some(record) = self.record_step(flash, now)
        {
            self.settle_turn();
            return next(BUSY_MS, record);
        }

        let step = match self.work {
            Work::Checking { .. } => self.check_step(flash),
            Work::Leaving { since, give_way } => self.leave(flash, since, give_way, now),
            Work::Restarting { .. } => Step {
                delay_ms: 0,
                record: None,
                restart: true,
            },
            // The request waits in the mailbox for the write turn.
            Work::Idle => match mailbox::pending() {
                Some(_) if !self.take_turn(now) => next(IDLE_MS, None),
                Some(request) => self.request(request, flash, now),
                None if self.install.is_some() => self.install_step(flash, now),
                None => next(IDLE_MS, None),
            },
        };
        self.settle_turn();
        self.publish();
        step
    }

    /// Bank 0's own image: hash it, then record it once the hooks are
    /// armed. `None` once there is nothing left to do this step.
    fn record_step(&mut self, flash: &mut impl Flash, now: u32) -> Option<Option<Record>> {
        let scan = match self.scan.as_mut() {
            Some(scan) => scan,
            None => match Scan::start(flash) {
                Some(scan) => self.scan.insert(scan),
                None => {
                    self.record_bank0 = false;
                    return Some(Some(Record::Bank0Unknown));
                }
            },
        };
        let len = scan.len;
        let Some(done) = scan.step(flash) else {
            return Some(None);
        };
        let Ok(sha256) = done else {
            self.scan = None;
            self.record_bank0 = false;
            return Some(Some(Record::Bank0Unknown));
        };
        if !flash.armed() {
            // Keep the digest; the settings writer arms the hooks soon.
            return None;
        }
        // Keep it too while an install or a request holds the flash (they
        // come first), or until the write turn is free of feeds.
        if self.install.is_some()
            || !matches!(self.work, Work::Idle)
            || mailbox::pending().is_some()
            || !self.take_turn(now)
        {
            return None;
        }

        self.scan = None;
        self.record_bank0 = false;
        let bank0 = Bank0 {
            build_id: self.identity,
            len,
            sha256,
            key: self.key.map(|key| offer::key_id(&key)),
        };
        let recorded = self.store.record().bank0 != Some(bank0);
        let saved = under_marker(|| self.store.set_bank0(bank0).is_ok());
        Some(Some(if saved {
            Record::Bank0 { len, recorded }
        } else {
            Record::Bank0Unknown
        }))
    }

    fn refuse(&mut self, refused: Refused) -> Step {
        mailbox::finish(Err(refused));
        next(IDLE_MS, Some(Record::Refused { refused }))
    }

    fn request(&mut self, request: Request, flash: &mut impl Flash, now: u32) -> Step {
        match request {
            Request::Offer {
                manifest,
                downgrade,
                device,
            } => self.offer(&manifest, downgrade.as_ref(), device, flash, now),
            Request::Statement { bytes, len } => {
                let len = usize::from(len).min(bytes.len());
                self.statement(&bytes[..len], flash)
            }
            Request::Cancel => self.cancel(flash),
        }
    }

    /// The keys a check may use, with the owners' enrolled update keys.
    fn ring<'a>(&'a self, built_in: &'a [[u8; 32]], enrolled: &'a [[u8; 32]]) -> KeyRing<'a> {
        let record = self.store.record();
        KeyRing {
            built_in,
            added: record.added_key(),
            enrolled,
            revoked: record.revoked(),
        }
    }

    /// Check the offer. Out of line: the signature checks are the deepest
    /// frames the service task runs.
    #[inline(never)]
    fn offer(
        &mut self,
        manifest: &[u8],
        downgrade: Option<&[u8; mailbox::DOWNGRADE_LEN]>,
        device: [u8; 32],
        flash: &mut impl Flash,
        now: u32,
    ) -> Step {
        if !self.store.readable() {
            return self.refuse(Refused::Unreadable);
        }
        if self.attempt {
            return self.refuse(Refused::Attempt);
        }
        if !flash.armed() {
            return self.refuse(Refused::Busy);
        }

        let built_in: &[[u8; 32]] = match &self.key {
            Some(key) => core::slice::from_ref(key),
            None => &[],
        };
        let enrolled = enrolled();
        let (keys, count) = &enrolled;
        let context = Context {
            ring: self.ring(built_in, &keys[..*count]),
            floor: self.min(),
            device,
            running: self.identity,
        };
        let admitted = match admit_offer(manifest, downgrade.map(|d| &d[..]), &context) {
            Ok(admitted) => admitted,
            Err(error) => return self.refuse(Refused::Offer(error)),
        };
        let version = admitted.signed.manifest.version;

        if self.bank == 1 {
            return self.check_bank0(admitted, flash, version);
        }
        self.start_install(admitted, now)
    }

    /// Bank 1: bank 0 must hold a valid marker and exactly the image it
    /// recorded, and must find the key that signed the offer; the image
    /// check runs over the next steps. Both banks read the same store and
    /// client table, so only a built-in key can differ between them.
    fn check_bank0(&mut self, admitted: Admitted, flash: &mut impl Flash, version: u32) -> Step {
        let mut marker = [0u8; 8];
        let valid = flash.bank0(0, &mut marker) && marker == MARKER;
        let recorded = self.store.record().bank0;
        // Bank 0 must find every built-in key the offer leans on: the
        // manifest's signer and a downgrade statement's.
        let finds = |key_id: KeyId, trust: Trust| {
            trust != Trust::BuiltIn || recorded.is_some_and(|bank0| bank0.key == Some(key_id))
        };
        let trusted = finds(admitted.signed.key_id, admitted.trust)
            && admitted
                .downgrade
                .is_none_or(|signer| finds(signer.key_id, signer.trust));
        let scan = if valid && recorded.is_some() && trusted {
            Scan::start(flash)
        } else {
            None
        };
        let Some(scan) = scan else {
            mailbox::finish(Err(Refused::Fallback));
            return next(IDLE_MS, Some(Record::GiveWayRefused));
        };
        self.work = Work::Checking { admitted, scan };
        next(BUSY_MS, Some(Record::Offer { version }))
    }

    fn check_step(&mut self, flash: &mut impl Flash) -> Step {
        let Work::Checking { admitted, scan } = &mut self.work else {
            return next(IDLE_MS, None);
        };
        let Some(done) = scan.step(flash) else {
            return next(BUSY_MS, None);
        };
        let len = scan.len;
        let admitted = *admitted;
        self.work = Work::Idle;

        let matches = done.is_ok_and(|sha256| {
            self.store
                .record()
                .bank0
                .is_some_and(|bank0| bank0.len == len && bank0.sha256 == sha256)
        });
        if !matches {
            mailbox::finish(Err(Refused::Fallback));
            return next(IDLE_MS, Some(Record::GiveWayRefused));
        }
        if let Err(error) = under_marker(|| self.store.set_pending(candidate(&admitted))) {
            mailbox::finish(Err(Refused::Store(error)));
            return next(
                IDLE_MS,
                Some(Record::Refused {
                    refused: Refused::Store(error),
                }),
            );
        }

        self.work = Work::Leaving {
            since: sdk::uptime_ms(),
            give_way: true,
        };
        mailbox::finish(Ok(Answer::Progress(self.progress())));
        next(BUSY_MS, None)
    }

    /// Bank 0: install the pending manifest, or this one when none is
    /// pending.
    fn start_install(&mut self, admitted: Admitted, now: u32) -> Step {
        let candidate = candidate(&admitted);
        match self.store.record().state {
            State::Pending(c) if c.digest != candidate.digest => {
                return self.refuse(Refused::Pending);
            }
            State::Activated(c) if c.digest == candidate.digest => {
                return self.refuse(Refused::Failed);
            }
            State::Pending(_) => {}
            State::None | State::Activated(_) => {
                if let Err(error) = under_marker(|| self.store.set_pending(candidate)) {
                    return self.refuse(Refused::Store(error));
                }
            }
        }

        let Some(install) = Install::new(admitted, now) else {
            return self.refuse(Refused::Busy);
        };
        let manifest = admitted.signed.manifest;
        self.install = Some(install);
        self.stopped = None;
        self.sectors = 0;
        // Chunks a stopped install left behind are not this one's.
        mailbox::drop_chunks();
        // A reset from here until the install ends refuses offers until
        // power is lost.
        arming::set_update_attempt(true);
        mailbox::finish(Ok(Answer::Progress(self.progress())));
        next(
            BUSY_MS,
            Some(Record::Manifest {
                version: manifest.version,
                bytes: manifest.image_len,
            }),
        )
    }

    #[inline(never)]
    fn statement(&mut self, bytes: &[u8], flash: &mut impl Flash) -> Step {
        if !self.store.readable() {
            return self.refuse(Refused::Unreadable);
        }
        if !flash.armed() {
            return self.refuse(Refused::Busy);
        }
        let built_in: &[[u8; 32]] = match &self.key {
            Some(key) => core::slice::from_ref(key),
            None => &[],
        };
        let statement = match admit_statement(bytes, &self.ring(built_in, &[])) {
            Ok(statement) => statement,
            Err(error) => return self.refuse(Refused::Statement(error)),
        };
        let saved = under_marker(|| match statement {
            KeyStatement::Revoke(id) => self.store.revoke(&id),
            KeyStatement::Add(key) => self.store.add_key(&key),
        });
        if let Err(error) = saved {
            return self.refuse(Refused::Store(error));
        }
        mailbox::finish(Ok(Answer::Key(statement)));
        next(IDLE_MS, Some(Record::Key { statement }))
    }

    fn cancel(&mut self, flash: &mut impl Flash) -> Step {
        // A recorded candidate can only be forgotten with a write: without
        // one, the cancel did nothing and says so.
        let recorded = self.store.record().state != State::None;
        if recorded && !flash.armed() {
            return self.refuse(Refused::Busy);
        }
        let record = self.install.is_some().then(|| self.stop(Stop::Cancelled));
        if recorded && under_marker(|| self.store.clear()).is_err() {
            return self.refuse(Refused::Store(
                wellfed_core::store::update::UpdateError::Io,
            ));
        }
        self.stopped = record.map(|_| Stop::Cancelled);
        mailbox::finish(Ok(Answer::Progress(self.progress())));
        next(IDLE_MS, record)
    }

    /// End the install in order: bank 1 stays invalid, the lease and the
    /// attempt bit are released.
    fn stop(&mut self, stop: Stop) -> Record {
        if let Some(install) = self.install.as_mut() {
            install.abort(stop);
        }
        let stop = self
            .install
            .take()
            .and_then(|i| i.stopped())
            .unwrap_or(stop);
        self.stopped = Some(stop);
        arming::set_update_attempt(false);
        mailbox::drop_chunks();
        mailbox::release();
        Record::Stopped { stop }
    }

    fn install_step(&mut self, flash: &mut impl Flash, now: u32) -> Step {
        let Some(install) = self.install.as_mut() else {
            return next(IDLE_MS, None);
        };
        if let Some(stop) = install.tick(now) {
            let record = self.stop(stop);
            return next(IDLE_MS, Some(record));
        }

        let mut outcome = None;
        let leased = mailbox::installing();
        let took = mailbox::take_chunk(|offset, data| {
            outcome = Some(if leased {
                take(install, flash, offset, data, now)
            } else {
                Err(Stop::Cancelled)
            });
        });
        if !took {
            return next(BUSY_MS, None);
        }

        match outcome {
            Some(Err(stop)) => {
                let record = self.stop(stop);
                next(IDLE_MS, Some(record))
            }
            Some(Ok(true)) => self.written(flash, now),
            _ => next(BUSY_MS, None),
        }
    }

    /// A sector went in: log progress, or activate after the last.
    fn written(&mut self, flash: &mut impl Flash, now: u32) -> Step {
        self.sectors += 1;
        let Some(install) = self.install.as_ref() else {
            return next(IDLE_MS, None);
        };
        if install.ready() {
            return self.activate(flash, now);
        }
        let total = install
            .admitted()
            .signed
            .manifest
            .image_len
            .div_ceil(SECTOR_BYTES);
        let record = self
            .sectors
            .is_multiple_of(PROGRESS_SECTORS)
            .then_some(Record::Progress {
                sectors: self.sectors,
                total,
            });
        next(BUSY_MS, record)
    }

    /// Record the activation, then program the withheld marker last and
    /// read it back: bank 1 is valid.
    fn activate(&mut self, flash: &mut impl Flash, now: u32) -> Step {
        let Some(install) = self.install.as_mut() else {
            return next(IDLE_MS, None);
        };
        let admitted = *install.admitted();
        let candidate = candidate(&admitted);
        if under_marker(|| self.store.set_activated(candidate)).is_err() {
            let record = self.stop(Stop::Store);
            return next(IDLE_MS, Some(record));
        }

        let Some(install) = self.install.as_mut() else {
            return next(IDLE_MS, None);
        };
        let marked = match install.commit() {
            Ok((offset, marker)) => {
                let mut readback = [0u8; 8];
                offset == BANK1_OFFSET
                    && under_marker(|| flash.program(0, 0, &marker))
                    && flash.read(0, 0, &mut readback)
                    && readback == marker
            }
            Err(_) => false,
        };
        if install.marked(marked).is_err() {
            // Bank 1 stays invalid: a later offer of the same manifest may
            // try again, as it never ran.
            let _ = under_marker(|| self.store.set_pending(candidate));
            let record = self.stop(Stop::Activate);
            return next(IDLE_MS, Some(record));
        }

        arming::set_update_attempt(false);
        self.work = Work::Leaving {
            since: now,
            give_way: false,
        };
        let manifest = admitted.signed.manifest;
        next(
            BUSY_MS,
            Some(Record::Activated {
                version: manifest.version,
                bytes: manifest.image_len,
                sectors: self.sectors,
            }),
        )
    }

    /// Restart once the answer is out: after bank 1's give-way answer was
    /// read and had time to leave, or a bounded wait.
    fn leave(&mut self, flash: &mut impl Flash, since: u32, give_way: bool, now: u32) -> Step {
        let waited = now.wrapping_sub(since);
        let answered = !give_way || mailbox::taken();
        if !(answered && waited >= LEAVE_MS) && waited < LEAVE_MAX_MS {
            return next(IDLE_MS, None);
        }
        if give_way {
            let version = match self.store.record().state {
                State::Pending(c) => c.version,
                _ => 0,
            };
            // Under the marker, then cleared: the bank-0 boot must not find
            // it set after a give-way that went in.
            if !under_marker(|| flash.give_way()) {
                self.work = Work::Idle;
                mailbox::release();
                return next(IDLE_MS, Some(Record::GiveWayRefused));
            }
            self.work = Work::Restarting { give_way: true };
            return Step {
                delay_ms: 0,
                record: Some(Record::GaveWay { version }),
                restart: true,
            };
        }
        self.work = Work::Restarting { give_way: false };
        Step {
            delay_ms: 0,
            record: None,
            restart: true,
        }
    }
}

/// One write under the attempt marker, as the settings writer brackets its
/// own: a hang inside it is skipped once after the reset. Only with the
/// write turn held.
fn under_marker<T>(write: impl FnOnce() -> T) -> T {
    arming::set_attempt();
    let out = write();
    arming::clear_attempt();
    out
}

/// The owners' enrolled update keys, as the settings writer last
/// published the client table.
fn enrolled() -> ([[u8; 32]; CLIENTS], usize) {
    let mut table: [Option<Client>; CLIENTS] = [None; CLIENTS];
    let mut keys = [[0u8; 32]; CLIENTS];
    let mut count = 0;
    if keys::clients_since(0, &mut table).is_some() {
        for client in table.iter().flatten() {
            if client.role == Role::UpdateSigner {
                keys[count] = client.key;
                count += 1;
            }
        }
    }
    (keys, count)
}

fn candidate(admitted: &Admitted) -> Candidate {
    Candidate {
        digest: admitted.digest,
        version: admitted.signed.manifest.version,
        build_id: admitted.signed.manifest.build_id,
        kind: admitted.signed.kind,
    }
}

/// One chunk into the install; `Ok(true)` when it completed a sector,
/// which is then written and read back.
fn take(
    install: &mut Install,
    flash: &mut impl Flash,
    offset: u32,
    data: &[u8],
    now: u32,
) -> Result<bool, Stop> {
    let Taken::Program(program) = install.chunk(offset, data, now)? else {
        return Ok(false);
    };
    let sector = (program.offset - BANK1_OFFSET) / SECTOR_BYTES;
    let ok = under_marker(|| write_sector(flash, sector, program.data));
    install.programmed(ok)?;
    Ok(true)
}

/// Erase one sector, program it page by page and read every page back.
/// A short last page is padded with erased bytes to the 4-byte program unit.
fn write_sector(flash: &mut impl Flash, sector: u32, data: &[u8]) -> bool {
    if data.len() > SECTOR || !flash.erase(sector) {
        return false;
    }

    let mut page = [u8::MAX; PIECE];
    let mut readback = [0u8; PIECE];
    for (piece, offset) in data.chunks(PIECE).zip((0u32..).step_by(PIECE)) {
        let padded = piece.len().next_multiple_of(4);
        page[..padded].fill(u8::MAX);
        page[..piece.len()].copy_from_slice(piece);
        if !flash.program(sector, offset, &page[..padded])
            || !flash.read(sector, offset, &mut readback[..padded])
            || readback[..padded] != page[..padded]
        {
            return false;
        }
    }
    true
}
