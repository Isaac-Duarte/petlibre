//! The update stage against the fake SDK: the loop's half of the mailbox
//! as the device API drives it, the update store in the fake storage
//! region, bank 1's NOR model and bank 0's image. Flash leases are not
//! modelled here (`flash_probe` covers the provider); the hooks count as
//! armed once the settings writer would have armed them.
//!
//! Every way an install can fail stops it before bank 1's marker is
//! written, so the loader keeps booting bank 0.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, MutexGuard};

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::install::{IDLE_MS, Phase, Progress, Refused, Stop};
use wellfed_core::ota::offer::{
    self, ImageKind, KeyError, KeyStatement, OfferError, Signed, StatementError, key_id,
};
use wellfed_core::ota::{MARKER, MAX_CHUNK, Manifest, SECTOR};
use wellfed_core::store::slot::SlotIo;
use wellfed_core::store::update::{Bank0, Candidate, State, UpdateStore};
use wellfed_hal::flash::park::{self, Mmio};
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::flash_lock::arming::{self, Arming, Event, Hooks, Poll, UPDATE_BIT};
use wellfed_hal::flash_lock::provider::{ATTEMPT_BIT, Provider};
use wellfed_hal::interlock::{Held, INTERLOCK};
use wellfed_hal::ota::mailbox::{self, Answer, Request};
use wellfed_hal::ota::update::*;
use wellfed_hal::sdk::fake;
use wellfed_hal::{keys, stopped};
use sha2::{Digest, Sha256};

/// The mailbox and the update bit are process-wide.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(true) };
    /// The stage's store writes fail from here on.
    static STORE_FAILS: Cell<bool> = const { Cell::new(false) };
}

struct FakeHooks;

impl Hooks for FakeHooks {
    fn watchdog() -> bool {
        true
    }
    fn install(_provider: *mut Provider) -> bool {
        ARMED.with(|a| a.set(true));
        true
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
}

type Slots = StoreSlots<FakeHooks>;

/// The stage's store, whose writes can be made to fail.
#[derive(Clone, Copy, Default)]
struct StageSlots(Slots);

impl SlotIo for StageSlots {
    fn unit_bytes(&self) -> u32 {
        self.0.unit_bytes()
    }
    fn read(&self, slot: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.0.read(slot, offset, bytes)
    }
    fn program(&self, slot: u32, offset: u32, bytes: &[u8]) -> bool {
        !STORE_FAILS.with(Cell::get) && self.0.program(slot, offset, bytes)
    }
    fn erase(&self, slot: u32) -> bool {
        !STORE_FAILS.with(Cell::get) && self.0.erase(slot)
    }
    fn writable(&self) -> bool {
        self.0.writable()
    }
}

/// The SDK's flash with failures to inject, watching every program.
#[derive(Default)]
struct TestFlash {
    inner: SdkFlash<FakeHooks>,
    /// Fail the bank-1 program call with this index (from zero).
    fail_program: Option<usize>,
    /// Program this call's data with one bit cleared, so readback differs.
    corrupt_program: Option<usize>,
    programs: usize,
    /// Bank 1's first 8 bytes before each program, and the store's state
    /// when the marker went in.
    marker_seen: Vec<[u8; 8]>,
    state_at_marker: Option<State>,
    /// The attempt register as each bank-1 program found it: what a reset
    /// inside that write would leave behind.
    register_seen: Vec<u32>,
}

impl Flash for TestFlash {
    fn armed(&self) -> bool {
        self.inner.armed()
    }
    fn erase(&mut self, sector: u32) -> bool {
        self.inner.erase(sector)
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        self.marker_seen
            .push(fake::with(|h| h.bank1[..8].try_into().unwrap()));
        if sector == 0 && offset == 0 && data == MARKER {
            self.state_at_marker = Some(store().record().state);
        }
        self.register_seen
            .push(fake::with(|h| h.backup[MARKER_REGISTER as usize]));
        let n = self.programs;
        self.programs += 1;
        if self.fail_program == Some(n) {
            return false;
        }
        if self.corrupt_program == Some(n) {
            let mut bad = data.to_vec();
            bad[0] &= 0xfe;
            bad[0] ^= u8::from(bad[0] == data[0]);
            return self.inner.program(sector, offset, &bad);
        }
        self.inner.program(sector, offset, data)
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.inner.read(sector, offset, bytes)
    }
    fn bank0(&mut self, offset: u32, bytes: &mut [u8]) -> bool {
        self.inner.bank0(offset, bytes)
    }
    fn give_way(&mut self) -> bool {
        self.inner.give_way()
    }
}

/// The update store as the fake storage holds it now.
fn store() -> UpdateStore<Slots> {
    let mut store = UpdateStore::default();
    store.open(Slots::new());
    store
}

const BENCH: [u8; 32] = [7; 32];
const DEVICE: [u8; 32] = [0xd0; 32];
const RUNNING: [u8; 32] = [0xee; 32];
const LEN: usize = 2 * SECTOR + 300;

fn bench() -> SigningKey {
    SigningKey::from_bytes(&BENCH)
}

fn image(len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    out[..8].copy_from_slice(&MARKER);
    out
}

fn manifest_for(
    key: &SigningKey,
    version: u32,
    payload: &[u8],
    build: u8,
    kind: ImageKind,
) -> Vec<u8> {
    let signed = Signed {
        manifest: Manifest {
            version,
            image_len: u32::try_from(payload.len()).unwrap(),
            target_bank: 1,
            image_sha256: Sha256::digest(payload).into(),
            build_id: [build; 32],
        },
        kind,
        key_id: key_id(&key.verifying_key().to_bytes()),
    };
    let body = offer::manifest_body(&signed);
    let mut out = body.to_vec();
    out.extend_from_slice(&key.sign(&body).to_bytes());
    out
}

fn manifest(version: u32, payload: &[u8]) -> Vec<u8> {
    manifest_for(&bench(), version, payload, 0x22, ImageKind::Replacement)
}

/// Bank 0 holding a reviewed build's layout.
fn bank0_image() -> Vec<u8> {
    let mut out = Vec::new();
    let part = |out: &mut Vec<u8>, address: u32, body: u32| {
        out.extend_from_slice(&MARKER);
        out.extend_from_slice(&body.to_le_bytes());
        out.extend_from_slice(&address.to_le_bytes());
        out.extend_from_slice(&[0xff; 16]);
        out.extend(std::iter::repeat_n(0x3c, body as usize));
    };
    part(&mut out, 0x0C00_0020, 3000);
    part(&mut out, 0x0008_3000, 200);
    out.resize(out.len().next_multiple_of(SECTOR), 0xff);
    part(&mut out, 0x0E00_0020, 40_000);
    part(&mut out, 0x1000_5000, 900);
    part(&mut out, 0x0200_0020, 0);
    out.resize(out.len().next_multiple_of(SECTOR), 0xff);
    out
}

fn start(bank: u8) -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    mailbox::reset();
    keys::reset();
    ARMED.with(|a| a.set(true));
    STORE_FAILS.with(|f| f.set(false));
    INTERLOCK.reset();
    fake::with(|h| {
        h.interrupts = 1;
        h.running_bank = Some(bank);
        h.bank0 = bank0_image();
        if bank == 1 {
            h.bank1[..8].copy_from_slice(&MARKER);
        }
    });
    assert!(stopped::initialize().verified());
    guard
}

/// The stage and a clock.
struct Rig {
    stage: Box<Update<StageSlots>>,
    flash: TestFlash,
    now: u32,
    records: Vec<Record>,
    restarted: bool,
}

impl Rig {
    fn new(bank: u8, key: Option<[u8; 32]>) -> Self {
        Self {
            stage: Box::new(Update::new(bank, RUNNING, key, StageSlots(Slots::new()))),
            flash: TestFlash::default(),
            now: 1_000,
            records: Vec::new(),
            restarted: false,
        }
    }

    fn bench(bank: u8) -> Self {
        Self::new(bank, Some(bench().verifying_key().to_bytes()))
    }

    fn step(&mut self) {
        self.now += 1;
        let step = self.stage.step(&mut self.flash, self.now);
        self.records.extend(step.record);
        self.restarted |= step.restart;
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Queue `request` and step until it is answered.
    fn ask(&mut self, request: Request) -> Result<Answer, Refused> {
        let ticket = mailbox::request(request)?;
        for _ in 0..500 {
            self.step();
            if let Some(answer) = mailbox::outcome(ticket) {
                return answer;
            }
        }
        panic!("no answer");
    }

    fn offer(&mut self, manifest: &[u8], downgrade: Option<&[u8]>) -> Result<Answer, Refused> {
        self.ask(Request::offer(manifest, downgrade, DEVICE)?)
    }

    /// Push every wanted chunk as the device API would, stepping between;
    /// the progress once nothing more is wanted or the install ended.
    fn push(&mut self, payload: &[u8], stop_at: Option<u32>) -> Progress {
        for _ in 0..20_000 {
            let progress = mailbox::progress().unwrap();
            if progress.phase != Phase::Receiving {
                return progress;
            }
            for offset in progress.wanted.into_iter().flatten() {
                if stop_at.is_some_and(|at| offset >= at) {
                    return progress;
                }
                let start = offset as usize;
                let end = (start + MAX_CHUNK as usize).min(payload.len());
                if mailbox::chunk(offset, &payload[start..end]).is_err() {
                    break;
                }
            }
            self.step();
        }
        panic!("no progress");
    }

    fn marker_never_written(&self) -> bool {
        fake::with(|h| h.bank1[..8] != MARKER)
            && self.flash.marker_seen.iter().all(|m| *m == [0xff; 8])
    }
}

fn progress(answer: Result<Answer, Refused>) -> Progress {
    match answer {
        Ok(Answer::Progress(progress)) => progress,
        other => panic!("{other:?}"),
    }
}

#[test]
fn bank_0_installs_an_offer_with_the_activation_recorded_before_the_marker() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    let signed = manifest(3, &payload);

    let accepted = progress(rig.offer(&signed, None));
    assert_eq!(accepted.phase, Phase::Receiving);
    assert_eq!(accepted.digest, Some(offer::manifest_digest(&signed)));
    assert!(mailbox::installing(), "the lease is held");
    assert!(arming::update_attempted());

    let done = rig.push(&payload, None);
    assert_eq!(done.phase, Phase::Activated);
    let bank1 = fake::with(|h| h.bank1[..LEN].to_vec());
    assert_eq!(bank1, payload);
    // Every program before the marker saw it erased; the store said
    // `Activated` when it went in.
    let (last, before) = rig.flash.marker_seen.split_last().unwrap();
    assert!(before.iter().all(|m| *m == [0xff; 8]));
    assert_eq!(*last, [0xff; 8]);
    assert!(matches!(rig.flash.state_at_marker, Some(State::Activated(c)) if c.version == 3));
    assert!(!arming::update_attempted());

    rig.steps(1);
    assert!(!rig.restarted, "the answer leaves first");
    rig.now += LEAVE_MAX_MS;
    rig.steps(1);
    assert!(rig.restarted);
    assert!(
        rig.records
            .iter()
            .any(|r| matches!(r, Record::Activated { version: 3, .. }))
    );
}

#[test]
fn a_flipped_byte_stops_at_the_hash_and_never_writes_the_marker() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    rig.offer(&manifest(3, &payload), None).unwrap();
    let mut flipped = payload.clone();
    flipped[SECTOR + 40] ^= 0x10;

    let stopped = rig.push(&flipped, None);
    assert_eq!(
        (stopped.phase, stopped.stopped),
        (Phase::Idle, Some(Stop::Hash))
    );
    assert!(rig.marker_never_written());
    assert!(!mailbox::installing(), "the lease is released");
    assert!(!arming::update_attempted());
    assert!(rig.records.contains(&Record::Stopped {
        stop: Stop::Hash,
        write: None
    }));
}

#[test]
fn a_failed_program_or_readback_stops_the_install() {
    for (fail, corrupt) in [(Some(3), None), (None, Some(20))] {
        let _guard = start(0);
        let mut rig = Rig::bench(0);
        rig.flash.fail_program = fail;
        rig.flash.corrupt_program = corrupt;
        let payload = image(LEN);
        rig.offer(&manifest(3, &payload), None).unwrap();
        let stopped = rig.push(&payload, None);
        assert_eq!(stopped.stopped, Some(Stop::Write), "{fail:?} {corrupt:?}");
        assert!(rig.marker_never_written());

        // The stop record names the sector, the step and, for a readback,
        // the word: its first bit flipped, and still flipped on a second read,
        // so the cells hold it rather than the read path.
        let write = rig.records.iter().find_map(|record| match record {
            Record::Stopped { write, .. } => *write,
            _ => None,
        });
        let write = write.expect("a write fault in the stop record");
        if fail.is_some() {
            assert_eq!(
                (write.sector, write.step, write.offset),
                (0, SectorStep::Program, 768)
            );
        } else {
            assert_eq!(
                (write.sector, write.step, write.offset),
                (1, SectorStep::Readback, 1024)
            );
            assert_eq!(write.expected ^ write.read, 1 << 24, "{write:?}");
            assert_eq!(write.reread, write.read);
        }
        let line = record_text(Record::Stopped {
            stop: Stop::Write,
            write: Some(write),
        });
        assert!(line.starts_with("error stage=write sector="), "{line}");
    }
}

#[test]
fn a_marker_that_fails_stops_the_install_and_keeps_the_offer_pending() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(SECTOR);
    // 16 page programs for the one sector, then the marker.
    rig.flash.fail_program = Some(16);
    rig.offer(&manifest(3, &payload), None).unwrap();
    let stopped = rig.push(&payload, None);
    assert_eq!(stopped.stopped, Some(Stop::Activate));
    assert!(fake::with(|h| h.bank1[..8] != MARKER));
    assert!(matches!(store().record().state, State::Pending(c) if c.version == 3));
    assert!(!rig.restarted);
}

#[test]
fn a_failed_activation_record_stops_before_the_marker() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(SECTOR);
    rig.offer(&manifest(3, &payload), None).unwrap();
    assert!(arming::update_attempted());

    // The store takes nothing more: the activation cannot be recorded.
    STORE_FAILS.with(|f| f.set(true));
    let stopped = rig.push(&payload, None);
    assert_eq!(stopped.stopped, Some(Stop::Store));
    assert!(rig.marker_never_written());
    assert!(!mailbox::installing(), "the lease is released");
    assert!(!arming::update_attempted(), "the attempt bit is cleared");
    assert!(!rig.restarted);
}

#[test]
fn every_request_holds_feeds_off_until_it_is_answered() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    rig.steps(20);

    // A statement (or a cancel) takes the lease as it is queued.
    let ticket = mailbox::request(Request::statement(&[0; 88]).unwrap()).unwrap();
    assert!(mailbox::installing());
    rig.steps(5);
    assert!(mailbox::outcome(ticket).is_some());
    assert!(!mailbox::installing(), "answered: the lease is gone");

    // During an install only a cancel goes in; the install keeps its lease
    // until the cancel stops it.
    let payload = image(SECTOR);
    rig.offer(&manifest(3, &payload), None).unwrap();
    assert_eq!(
        mailbox::request(Request::statement(&[0; 88]).unwrap()),
        Err(Refused::Installing)
    );
    let cancelled = progress(rig.ask(Request::Cancel));
    assert_eq!(cancelled.stopped, Some(Stop::Cancelled));
    assert!(!mailbox::installing());
    assert_eq!(store().record().state, State::None);
}

#[test]
fn nothing_is_written_while_a_feed_holds_the_outputs() {
    let _guard = start(0);
    INTERLOCK.claim_feed(60_000).unwrap();
    let mut rig = Rig::bench(0);

    // Bank 0's boot record waits for the write turn.
    rig.steps(40);
    assert_eq!(store().record().bank0, None);
    assert!(!INTERLOCK.writing());

    // A request waits in the mailbox, unanswered, never forced past the
    // claim.
    let ticket = mailbox::request(Request::statement(&[0; 88]).unwrap()).unwrap();
    rig.now += 600_000;
    rig.steps(40);
    assert_eq!(mailbox::outcome(ticket), None);
    assert!(!INTERLOCK.writing());

    // Once the outputs are free both go in, and the turn is given back.
    INTERLOCK.release_feed();
    rig.steps(40);
    assert!(mailbox::outcome(ticket).is_some());
    assert!(store().record().bank0.is_some());
    assert!(!INTERLOCK.writing(), "the turn is given back");
    assert!(!mailbox::installing());
}

#[test]
fn an_install_holds_the_write_turn_until_it_stops() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    rig.steps(20);
    let payload = image(SECTOR);
    rig.offer(&manifest(3, &payload), None).unwrap();

    // Every feed meanwhile is refused as a write's.
    assert!(INTERLOCK.writing());
    assert_eq!(INTERLOCK.claim_feed(u32::MAX), Err(Held::Writing));
    rig.push(&payload, Some(SECTOR as u32 / 2));
    assert!(INTERLOCK.writing());

    let cancelled = progress(rig.ask(Request::Cancel));
    assert_eq!(cancelled.stopped, Some(Stop::Cancelled));
    assert!(!INTERLOCK.writing(), "a stop gives the turn back");
    assert_eq!(INTERLOCK.claim_feed(u32::MAX), Ok(()));
}

#[test]
fn the_give_way_waits_for_a_feed_and_keeps_the_turn_until_the_restart() {
    let _guard = start(1);
    store().set_bank0(recorded_bank0()).unwrap();
    INTERLOCK.claim_feed(u32::MAX).unwrap();
    let mut rig = Rig::bench(1);
    let signed = manifest(3, &image(LEN));

    let ticket = mailbox::request(Request::offer(&signed, None, DEVICE).unwrap()).unwrap();
    rig.steps(100);
    assert_eq!(
        mailbox::outcome(ticket),
        None,
        "the offer waits for the turn"
    );
    assert_eq!(store().record().state, State::None);
    assert_eq!(fake::with(|h| h.bank1[0]), MARKER[0]);

    INTERLOCK.release_feed();
    for _ in 0..100 {
        rig.step();
        if mailbox::outcome(ticket).is_some() {
            break;
        }
    }
    assert_eq!(
        progress(mailbox::outcome(ticket).unwrap()).phase,
        Phase::GivingWay
    );
    rig.now += LEAVE_MS;
    rig.steps(2);
    assert!(rig.restarted);
    assert_eq!(fake::with(|h| h.bank1[0]), 0, "given way");
    // No feed starts between the give-way and the restart.
    assert!(INTERLOCK.writing());
    assert_eq!(INTERLOCK.claim_feed(u32::MAX), Err(Held::Writing));
}

#[test]
fn a_cancel_that_cannot_forget_the_candidate_says_busy() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(SECTOR);
    rig.offer(&manifest(3, &payload), None).unwrap();
    ARMED.with(|a| a.set(false));
    assert_eq!(rig.ask(Request::Cancel), Err(Refused::Busy));
    assert!(matches!(store().record().state, State::Pending(_)));

    ARMED.with(|a| a.set(true));
    assert!(rig.ask(Request::Cancel).is_ok());
    assert_eq!(store().record().state, State::None);
}

#[test]
fn an_unwanted_chunk_stops_the_install_in_order() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    rig.offer(&manifest(3, &payload), None).unwrap();
    mailbox::chunk(2 * SECTOR as u32, &payload[2 * SECTOR..2 * SECTOR + 300]).unwrap();
    rig.steps(5);
    let progress = mailbox::progress().unwrap();
    assert_eq!(progress.stopped, Some(Stop::Order));
    assert!(rig.marker_never_written());

    // The stopped offer is still pending, so it may go again; a chunk
    // already held is answered and changes nothing.
    let mut rig = Rig::bench(0);
    rig.offer(&manifest(3, &payload), None).unwrap();
    mailbox::chunk(0, &payload[..512]).unwrap();
    rig.steps(3);
    mailbox::chunk(0, &payload[..512]).unwrap();
    rig.steps(3);
    assert_eq!(mailbox::progress().unwrap().phase, Phase::Receiving);
}

#[test]
fn a_quiet_install_times_out() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    rig.offer(&manifest(3, &payload), None).unwrap();
    let partial = rig.push(&payload, Some(SECTOR as u32));
    assert_eq!(partial.written, SECTOR as u32);
    rig.now += IDLE_MS;
    rig.steps(2);
    assert_eq!(mailbox::progress().unwrap().stopped, Some(Stop::Timeout));
    assert!(rig.marker_never_written());
    assert!(!mailbox::installing());
}

#[test]
fn without_a_built_in_key_or_with_another_signer_nothing_installs() {
    let _guard = start(0);
    let payload = image(LEN);
    let mut rig = Rig::new(0, None);
    assert_eq!(
        rig.offer(&manifest(3, &payload), None),
        Err(Refused::Offer(OfferError::Key(KeyError::Unknown)))
    );
    assert!(!mailbox::installing());

    let mut rig = Rig::bench(0);
    let stranger = SigningKey::from_bytes(&[9; 32]);
    let foreign = manifest_for(&stranger, 3, &payload, 1, ImageKind::Replacement);
    assert_eq!(
        rig.offer(&foreign, None),
        Err(Refused::Offer(OfferError::Key(KeyError::Unknown)))
    );

    // A version 1 manifest is the wrong length for the mailbox, and a
    // version 1 magic is refused by the check.
    let mut v1 = vec![0; 148];
    v1[..8].copy_from_slice(b"PLOTA1\0\0");
    assert_eq!(
        Request::offer(&v1, None, DEVICE),
        Err(Refused::Offer(OfferError::Length))
    );
    let mut padded = v1.clone();
    padded.resize(offer::MANIFEST_LEN, 0);
    assert_eq!(
        rig.offer(&padded, None),
        Err(Refused::Offer(OfferError::Version))
    );
    assert!(fake::with(|h| h.bank1.iter().all(|&b| b == 0xff)));
}

#[test]
fn an_activation_restarts_with_both_attempt_bits_clear() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    rig.offer(&manifest(3, &payload), None).unwrap();
    assert_eq!(rig.push(&payload, None).phase, Phase::Activated);
    rig.now += LEAVE_MAX_MS;
    rig.steps(2);
    assert!(rig.restarted);

    // A restart on purpose is not a crash: nothing is left for the next
    // boot to skip on, so the build bank 1 falls back to still arms.
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert!(matches!(arm(), Poll::Armed(_)));
}

#[test]
fn a_reset_inside_a_sector_write_skips_arming_once() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    rig.offer(&manifest(3, &payload), None).unwrap();
    rig.push(&payload, Some(SECTOR as u32));

    // Every bank-1 program ran under both bits.
    let seen = rig.flash.register_seen.clone();
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|&r| r == UPDATE_BIT | ATTEMPT_BIT),
        "{seen:x?}"
    );

    // A reset inside one: the next boot skips arming (and refuses
    // offers); the boot after that arms again.
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = seen[0]);
    assert!(matches!(arm(), Poll::Failed(Event::Skip { .. })));
    assert!(arming::update_attempted());
    assert!(matches!(arm(), Poll::Armed(_)));
}

#[test]
fn a_reset_mid_install_refuses_offers_but_still_arms_the_flash() {
    let _guard = start(0);
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = UPDATE_BIT);
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    assert_eq!(
        rig.offer(&manifest(3, &payload), None),
        Err(Refused::Attempt)
    );
    assert!(
        rig.records
            .iter()
            .any(|r| matches!(r, Record::Opened { attempt: true, .. }))
    );
    assert!(fake::with(|h| h.bank1.iter().all(|&b| b == 0xff)));

    // Arming skips only for its own bit: the feeder still saves and feeds,
    // and the update bit stays for the next boot.
    assert!(matches!(arm(), Poll::Armed(_)));
    let marker = fake::with(|h| h.backup[MARKER_REGISTER as usize]);
    assert_eq!(marker, UPDATE_BIT | ATTEMPT_BIT);
    arming::clear_attempt();
    assert_eq!(
        fake::with(|h| h.backup[MARKER_REGISTER as usize]),
        UPDATE_BIT
    );

    // A flash attempt that reset still skips arming, and keeps the bit.
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = UPDATE_BIT | ATTEMPT_BIT);
    assert!(matches!(arm(), Poll::Failed(Event::Skip { .. })));
    assert_eq!(
        fake::with(|h| h.backup[MARKER_REGISTER as usize]),
        UPDATE_BIT
    );
}

thread_local! {
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
}

struct Core<const KM0: bool>;

impl<const KM0: bool> Mmio for Core<KM0> {
    const KM0: bool = KM0;
    fn load(address: u32) -> u32 {
        let (c, i) = locate(address);
        REGS.with(|r| r.borrow()[c][i])
    }
    fn store(address: u32, value: u32) {
        let (c, i) = locate(address);
        REGS.with(|r| r.borrow_mut()[c][i] = value);
    }
    fn barrier() {}
    fn mask() -> u32 {
        0
    }
    fn halt() -> ! {
        panic!("halt")
    }
}

/// USR words per core: index 0 KM0 (0x4800_6000), 1 KM4 (0x4000_6000).
fn locate(address: u32) -> (usize, usize) {
    let core = usize::from(address & 0xffff_f000 != 0x4800_6000);
    (core, (((address & 0xfff) - 0x20) / 4) as usize)
}

/// Arm the hooks with the KM0 peer answering through the real
/// `park::answer`; the final poll.
fn arm() -> Poll {
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));
    let mut provider = Provider::default();
    let mut arming = Arming::default();
    let mut answered = 0;
    for _ in 0..100 {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        // SAFETY: `provider` outlives the arming.
        let poll = unsafe { arming.step::<Core<false>, FakeHooks>(&mut provider) };
        if !matches!(poll, Poll::Pending(_)) {
            return poll;
        }
        fake::advance(arming::POLL_MS * 32, false);
    }
    panic!("arming never finished");
}

#[test]
fn bank_0_installs_only_the_pending_manifest_and_never_a_failed_one() {
    let _guard = start(0);
    let payload = image(LEN);
    let first = manifest(3, &payload);
    let digest = offer::manifest_digest(&first);
    let candidate = Candidate {
        digest,
        version: 3,
        build_id: [0x22; 32],
        kind: ImageKind::Replacement,
    };
    store().set_pending(candidate).unwrap();

    let mut rig = Rig::bench(0);
    let swapped = manifest(4, &payload);
    assert_eq!(rig.offer(&swapped, None), Err(Refused::Pending));
    assert!(progress(rig.offer(&first, None)).phase == Phase::Receiving);
    rig.ask(Request::Cancel).unwrap();
    assert_eq!(store().record().state, State::None);

    // Activated and back on bank 0: the trial failed.
    store().set_activated(candidate).unwrap();
    let mut rig = Rig::bench(0);
    assert_eq!(rig.offer(&first, None), Err(Refused::Failed));
}

#[test]
fn a_replayed_version_needs_a_downgrade_statement_for_this_feeder_and_floor() {
    let _guard = start(0);
    let mut s = store();
    let old = Candidate {
        digest: [1; 32],
        version: 5,
        build_id: [0x55; 32],
        kind: ImageKind::Replacement,
    };
    s.set_activated(old).unwrap();
    s.confirm(&[0x55; 32]).unwrap();

    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    let replay = manifest(5, &payload);
    assert_eq!(
        rig.offer(&replay, None),
        Err(Refused::Offer(OfferError::Downgrade))
    );
    let statement = |device: &[u8; 32], floor| {
        let body = offer::downgrade_body(
            &key_id(&bench().verifying_key().to_bytes()),
            device,
            &offer::manifest_digest(&replay),
            floor,
        );
        let mut out = body.to_vec();
        out.extend_from_slice(&bench().sign(&body).to_bytes());
        out
    };
    assert_eq!(
        rig.offer(&replay, Some(&statement(&[2; 32], 5))),
        Err(Refused::Offer(OfferError::Statement(
            StatementError::Device
        )))
    );
    assert_eq!(
        rig.offer(&replay, Some(&statement(&DEVICE, 4))),
        Err(Refused::Offer(OfferError::Statement(StatementError::Floor)))
    );
    assert!(progress(rig.offer(&replay, Some(&statement(&DEVICE, 5)))).phase == Phase::Receiving);
}

#[test]
fn a_retired_key_signs_nothing_more() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    let bench_id = key_id(&bench().verifying_key().to_bytes());
    let body = offer::revoke_body(&bench_id, &bench_id);
    let mut statement = body.to_vec();
    statement.extend_from_slice(&bench().sign(&body).to_bytes());

    let answer = rig.ask(Request::statement(&statement).unwrap());
    assert_eq!(answer, Ok(Answer::Key(KeyStatement::Revoke(bench_id))));
    assert_eq!(store().record().revoked(), &[bench_id]);
    assert_eq!(
        rig.offer(&manifest(3, &image(LEN)), None),
        Err(Refused::Offer(OfferError::Key(KeyError::Revoked)))
    );
}

#[test]
fn an_owner_s_enrolled_key_signs_an_offer() {
    let _guard = start(0);
    let owner = SigningKey::from_bytes(&[0x0e; 32]);
    keys::publish_clients(
        [wellfed_feeder::store::clients::Client {
            key: owner.verifying_key().to_bytes(),
            label: wellfed_feeder::store::clients::Label::new(b"laptop").unwrap(),
            role: wellfed_feeder::store::clients::Role::UpdateSigner,
            enrolled: 1,
        }]
        .into_iter(),
    );
    let mut rig = Rig::bench(0);
    let payload = image(LEN);
    let signed = manifest_for(&owner, 3, &payload, 0x22, ImageKind::Replacement);
    assert!(progress(rig.offer(&signed, None)).phase == Phase::Receiving);
}

#[test]
fn bank_0_installs_its_own_build_but_bank_1_refuses_to_rewrite_itself() {
    // Returning a feeder to bank 0's build means bank 0 installing that
    // build into bank 1; bank 1 offered its own build has nothing to do.
    let payload = image(LEN);
    let own = manifest_for(&bench(), 3, &payload, RUNNING[0], ImageKind::Replacement);

    {
        let _guard = start(1);
        let mut rig = Rig::bench(1);
        assert_eq!(
            rig.offer(&own, None).map(|_| ()),
            Err(Refused::Offer(OfferError::Same))
        );
    }

    let _guard = start(0);
    let mut rig = Rig::bench(0);
    assert_eq!(progress(rig.offer(&own, None)).phase, Phase::Receiving);
    assert_eq!(rig.push(&payload, None).phase, Phase::Activated);
}

#[test]
fn running_from_bank_1_never_writes_bank_1() {
    let _guard = start(1);
    let mut flash = SdkFlash::<FakeHooks>::new();
    assert!(!flash.erase(1));
    assert!(!flash.program(1, 0, &[0; 4]));
    assert!(fake::with(|h| h.bank1[8..].iter().all(|&b| b == 0xff)));
}

#[test]
fn bank_0_records_its_image_once() {
    let _guard = start(0);
    let mut rig = Rig::bench(0);
    rig.steps(20);
    let expected = Bank0 {
        build_id: RUNNING,
        len: u32::try_from(bank0_image().len()).unwrap(),
        sha256: Sha256::digest(bank0_image()).into(),
        key: Some(key_id(&bench().verifying_key().to_bytes())),
    };
    assert_eq!(store().record().bank0, Some(expected));
    assert!(rig.records.contains(&Record::Bank0 {
        len: expected.len,
        recorded: true
    }));

    let erases = fake::with(|h| h.storage_erases);
    let mut again = Rig::bench(0);
    again.steps(20);
    assert_eq!(
        fake::with(|h| h.storage_erases),
        erases,
        "no write for the same image"
    );
}

/// Bank 0's image as bank 0 records it at boot.
fn recorded_bank0() -> Bank0 {
    Bank0 {
        build_id: [0x0b; 32],
        len: u32::try_from(bank0_image().len()).unwrap(),
        sha256: Sha256::digest(bank0_image()).into(),
        key: Some(key_id(&bench().verifying_key().to_bytes())),
    }
}

#[test]
fn bank_1_records_the_offer_then_gives_way_once_the_answer_is_out() {
    let _guard = start(1);
    store().set_bank0(recorded_bank0()).unwrap();
    let mut rig = Rig::bench(1);
    let payload = image(LEN);
    let signed = manifest(3, &payload);

    let ticket = mailbox::request(Request::offer(&signed, None, DEVICE).unwrap()).unwrap();
    let mut answer = None;
    for _ in 0..50 {
        rig.step();
        let progress = mailbox::progress().unwrap();
        if answer.is_none() && progress.phase == Phase::Checking {
            assert_eq!(
                fake::with(|h| h.bank1[0]),
                MARKER[0],
                "no give-way while checking"
            );
        }
        if let Some(outcome) = mailbox::outcome(ticket) {
            answer = Some(outcome);
            break;
        }
    }
    let answer = progress(answer.unwrap());
    assert_eq!(answer.phase, Phase::GivingWay);
    assert_eq!(answer.digest, Some(offer::manifest_digest(&signed)));
    assert!(matches!(store().record().state, State::Pending(c) if c.version == 3));
    assert!(mailbox::installing(), "no feed starts before the restart");
    assert_eq!(fake::with(|h| h.bank1[0]), MARKER[0]);

    rig.now += LEAVE_MS;
    rig.steps(1);
    assert!(rig.restarted);
    assert_eq!(
        fake::with(|h| h.bank1[0]),
        0,
        "bank 1's first marker byte cleared"
    );
    assert!(rig.records.contains(&Record::GaveWay { version: 3 }));
    assert!(fake::with(|h| h.bank1[1..]
        .iter()
        .take(7)
        .eq(MARKER[1..].iter())));
}

#[test]
fn bank_1_never_gives_way_to_a_bank_0_that_cannot_take_the_offer() {
    let payload = image(LEN);
    let signed = manifest(3, &payload);
    let cases: [(&str, fn()); 5] = [
        ("no record", || {}),
        ("another built-in key", || {
            let bank0 = Bank0 {
                key: Some([0x99; 8]),
                ..recorded_bank0()
            };
            store().set_bank0(bank0).unwrap();
        }),
        ("no built-in key", || {
            let bank0 = Bank0 {
                key: None,
                ..recorded_bank0()
            };
            store().set_bank0(bank0).unwrap();
        }),
        ("changed image", || {
            store().set_bank0(recorded_bank0()).unwrap();
            fake::with(|h| h.bank0[SECTOR + 100] ^= 1);
        }),
        ("invalid marker", || {
            store().set_bank0(recorded_bank0()).unwrap();
            fake::with(|h| h.bank0_marker[0] = 0);
        }),
    ];
    for (name, setup) in cases {
        let _guard = start(1);
        setup();
        let mut rig = Rig::bench(1);
        assert_eq!(rig.offer(&signed, None), Err(Refused::Fallback), "{name}");
        assert_eq!(
            store().record().state,
            State::None,
            "{name}: nothing pending"
        );
        assert!(!mailbox::installing(), "{name}: the lease is released");
        rig.now += LEAVE_MAX_MS;
        rig.steps(2);
        assert!(!rig.restarted, "{name}");
        assert!(fake::with(|h| h.bank1[..8] == MARKER), "{name}");
    }
}

#[test]
fn bank_1_never_gives_way_for_a_downgrade_bank_0_could_not_check() {
    // An owner's key signs the manifest, which bank 0 finds in the shared
    // client table; the downgrade statement is the built-in key's, which
    // bank 0's record does not name.
    let _guard = start(1);
    let owner = SigningKey::from_bytes(&[0x0e; 32]);
    keys::publish_clients(
        [wellfed_feeder::store::clients::Client {
            key: owner.verifying_key().to_bytes(),
            label: wellfed_feeder::store::clients::Label::new(b"laptop").unwrap(),
            role: wellfed_feeder::store::clients::Role::UpdateSigner,
            enrolled: 1,
        }]
        .into_iter(),
    );
    let bank0 = Bank0 {
        key: Some([0x99; 8]),
        ..recorded_bank0()
    };
    store().set_bank0(bank0).unwrap();
    let payload = image(LEN);
    let stock = manifest_for(&owner, 3, &payload, 0x22, ImageKind::Stock);
    let body = offer::downgrade_body(
        &key_id(&bench().verifying_key().to_bytes()),
        &DEVICE,
        &offer::manifest_digest(&stock),
        0,
    );
    let mut statement = body.to_vec();
    statement.extend_from_slice(&bench().sign(&body).to_bytes());

    let mut rig = Rig::bench(1);
    assert_eq!(rig.offer(&stock, Some(&statement)), Err(Refused::Fallback));
    assert_eq!(store().record().state, State::None);
    assert!(fake::with(|h| h.bank1[..8] == MARKER));
}

#[test]
fn a_build_in_trial_or_with_an_unreadable_store_takes_no_offer() {
    let _guard = start(1);
    store().set_bank0(recorded_bank0()).unwrap();
    ARMED.with(|a| a.set(false));
    let mut rig = Rig::bench(1);
    let signed = manifest(3, &image(LEN));
    assert_eq!(rig.offer(&signed, None), Err(Refused::Busy));

    // A store a newer schema wrote has no floor this build can trust.
    ARMED.with(|a| a.set(true));
    let at = 25 * SECTOR;
    fake::with(|h| {
        let record = &mut h.storage[at..at + 256];
        record[8] = 2;
        let crc = wellfed_core::store::slot::crc32(&record[..248]);
        record[248..252].copy_from_slice(&crc.to_le_bytes());
    });
    let mut rig = Rig::bench(1);
    assert_eq!(rig.offer(&signed, None), Err(Refused::Unreadable));
    assert_eq!(fake::with(|h| h.bank1[0]), MARKER[0]);
}

#[test]
fn confirming_raises_the_floor_only_for_the_activated_build() {
    let _guard = start(1);
    let candidate = |build| Candidate {
        digest: [4; 32],
        version: 9,
        build_id: build,
        kind: ImageKind::Replacement,
    };
    store().set_activated(candidate([0x44; 32])).unwrap();
    let mut rig = Rig::bench(1);
    assert_eq!(rig.stage.raise_floor(), Floor::Unchanged);
    assert_eq!(store().record().min_version, 0);

    store().set_activated(candidate(RUNNING)).unwrap();
    let mut rig = Rig::bench(1);
    assert_eq!(rig.stage.raise_floor(), Floor::Raised { version: 9 });
    let record = *store().record();
    assert_eq!((record.min_version, record.state), (9, State::None));
    assert_eq!(rig.stage.raise_floor(), Floor::Unchanged);
}

fn record_text(record: Record) -> String {
    let mut text = String::new();
    describe(record, &mut text).unwrap();
    text
}
