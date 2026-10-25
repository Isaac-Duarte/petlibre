//! An update end to end on the host: the owner's client
//! (`wellfed_client::update` over a `Connection`) against the feeder's
//! `Server`, with a fake feeder that keeps two banks, the update store and
//! the attempt flag, installs through the real `ota::install::Install` and
//! checks offers with the real `ota::offer::admit_offer`. Restarts drop the
//! server's RAM, as the feeder's do; a power cut also clears the attempt
//! flag, as it clears the backup register.
//!
//! Every failure before activation must leave bank 1's marker clear, so the
//! feeder keeps booting bank 0, which still answers.

use std::time::{Duration, Instant};

use ed25519_dalek::{Signer, SigningKey};
use wellfed_client::connection::{Connection, Event};
use wellfed_client::update::{Image, InstallError, Installer, Next, Sent};
use wellfed_core::ota::install::{Install, Phase, Progress, Refused, Stop, Taken};
use wellfed_core::ota::offer::{
    self, Context, ImageKind, KeyRing, Signed, admit_offer, key_id, manifest_digest,
};
use wellfed_core::ota::{self, BANK1_OFFSET, MARKER, Manifest, SECTOR};
use wellfed_core::store::clients::{CLIENTS, Client, Label, Role};
use wellfed_core::store::net::Credentials;
use wellfed_core::store::update::{Candidate, State, UpdateStore};
use wellfed_core::tone::Tune;
use wellfed_core::transport::{
    LockResult, PlayResult, SettingsFields, SettingsResult, SettingsView, TimeFields, TimeResult,
    WifiResult, WifiSource, WifiView,
};
use wellfed_feeder::api::{
    Device, EventRead, KeyChange, KeyOutcome, Pass, Server, UpdateAnswer, UpdateRequest,
};
use wellfed_feeder::pairing::{Config, Pairing};
use wellfed_feeder::update as wire;
use wellfed_model::{self as model, Answer, Op};
use wellfed_proto::api::{
    self as proto, Capabilities, EventBody, Refusal, RequestId, Status, UpdateView,
};
use wellfed_session::noise::Keypair;
use wellfed_testkit::journal_flash::{FakeJournalFlash, FlashIo};
use sha2::{Digest, Sha256};

const FEEDER: [u8; 32] = [0x10; 32];
const OWNER: [u8; 32] = [0x20; 32];
const BENCH: [u8; 32] = [0x05; 32];
const OLD_BUILD: [u8; 32] = [0x0a; 32];
/// Room for the largest image a test offers.
const BANK1: usize = 8 * SECTOR;

fn public(secret: [u8; 32]) -> [u8; 32] {
    *Keypair::from_secret(secret).public()
}

fn bench() -> SigningKey {
    SigningKey::from_bytes(&BENCH)
}

fn payload(len: usize, seed: u8) -> Vec<u8> {
    let mut out: Vec<u8> = (0..len)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect();
    out[..8].copy_from_slice(&MARKER);
    out
}

/// A version 2 manifest for `payload` as build `build`.
fn manifest(key: &SigningKey, version: u32, payload: &[u8], build: u8, kind: ImageKind) -> Vec<u8> {
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

fn downgrade(key: &SigningKey, manifest: &[u8], floor: u32) -> Vec<u8> {
    let body = offer::downgrade_body(
        &key_id(&key.verifying_key().to_bytes()),
        &public(FEEDER),
        &manifest_digest(manifest),
        floor,
    );
    let mut out = body.to_vec();
    out.extend_from_slice(&key.sign(&body).to_bytes());
    out
}

/// Faults the fake injects into chunks as they arrive.
#[derive(Default)]
struct Faults {
    /// Flip one bit of the image byte at this offset.
    flip: Option<u32>,
}

/// The feeder across restarts: its banks, its update store and the
/// attempt flag survive; everything else is RAM, gone at each restart.
struct Feeder {
    bank0_valid: bool,
    bank1: Vec<u8>,
    /// The build each bank holds.
    bank1_build: [u8; 32],
    bank0_build: [u8; 32],
    store_flash: Box<FakeJournalFlash>,
    store: UpdateStore<FlashIo>,
    /// The bank running now.
    bank: u8,
    /// Set while an install writes; a watchdog reset keeps it, a power cut
    /// clears it.
    attempt: bool,
    install: Option<Install>,
    /// The last install's stop, for the state.
    stopped: Option<Stop>,
    /// Restart into the loader's choice once the answers are out.
    restart: bool,
    /// Restart once the pending outcome has been answered, giving way.
    restart_on_answer: bool,
    /// The restart gives way: bank 1's first marker byte is cleared.
    give_way: bool,
    ticket: u32,
    outcome: Option<(u32, Result<UpdateAnswer, Refusal>)>,
    faults: Faults,
    /// A candidate that never gets healthy, as a failed trial.
    never_confirm: bool,
    trials: u32,
    feeds: u32,
}

impl Feeder {
    fn new() -> Self {
        let mut bank1 = vec![0xff; BANK1];
        bank1[..8].copy_from_slice(&MARKER);
        let mut feeder = Self {
            bank0_valid: true,
            bank1,
            bank1_build: OLD_BUILD,
            bank0_build: [0x0b; 32],
            store_flash: Box::new(FakeJournalFlash::with_unit(512)),
            store: UpdateStore::default(),
            bank: 1,
            attempt: false,
            install: None,
            stopped: None,
            restart: false,
            restart_on_answer: false,
            give_way: false,
            ticket: 0,
            outcome: None,
            faults: Faults::default(),
            never_confirm: false,
            trials: 0,
            feeds: 0,
        };
        feeder.boot();
        feeder
    }

    fn marked(&self) -> bool {
        self.bank1[..8] == MARKER
    }

    /// What the loader and KM0's trial pick, then the build's own start:
    /// a new build confirms (raising the floor) unless it never gets healthy.
    fn boot(&mut self) {
        self.install = None;
        self.stopped = None;
        self.restart = false;
        self.restart_on_answer = false;
        self.give_way = false;
        self.outcome = None;
        // SAFETY: the flash is boxed and outlives the store.
        unsafe { self.store.open(self.store_flash.io()) };

        self.bank = if self.marked() { 1 } else { 0 };
        if self.bank == 1
            && let State::Activated(candidate) = self.store.record().state
        {
            self.bank1_build = candidate.build_id;
            if self.never_confirm {
                self.trials += 1;
                if self.trials >= 3 && self.bank0_valid {
                    // KM0 gives way: bank 1's first marker byte cleared.
                    self.bank1[0] = 0;
                    self.trials = 0;
                    self.bank = 0;
                }
            } else {
                self.store.confirm(&candidate.build_id).unwrap();
            }
        }
    }

    fn power_cut(&mut self) {
        self.attempt = false;
        self.boot();
    }

    fn running(&self) -> [u8; 32] {
        if self.bank == 1 {
            self.bank1_build
        } else {
            self.bank0_build
        }
    }

    fn min(&self) -> u32 {
        self.store.record().min_version
    }

    fn view(&self) -> UpdateView {
        wire::view(&match &self.install {
            Some(install) => install.progress(self.bank, self.min()),
            None => Progress {
                stopped: self.stopped,
                ..Progress::idle(self.bank, self.min())
            },
        })
    }

    fn offer(
        &mut self,
        manifest: &[u8],
        downgrade: Option<&[u8]>,
        device: [u8; 32],
    ) -> Result<UpdateAnswer, Refusal> {
        let built_in = [bench().verifying_key().to_bytes()];
        let record = *self.store.record();
        let context = Context {
            ring: KeyRing {
                built_in: &built_in,
                added: record.added_key(),
                enrolled: &[],
                revoked: record.revoked(),
            },
            floor: record.min_version,
            device,
            running: self.running(),
        };
        let refuse = |refused| wire::refusal(refused);
        let admitted = admit_offer(manifest, downgrade, &context)
            .map_err(|error| refuse(Refused::Offer(error)))?;
        let candidate = Candidate {
            digest: admitted.digest,
            version: admitted.signed.manifest.version,
            build_id: admitted.signed.manifest.build_id,
            kind: admitted.signed.kind,
        };

        if self.bank == 1 {
            if !self.bank0_valid {
                return Err(refuse(Refused::Fallback));
            }
            self.store
                .set_pending(candidate)
                .map_err(|error| refuse(Refused::Store(error)))?;
            self.restart_on_answer = true;
            let progress = Progress::idle(1, self.min()).of(Phase::GivingWay, &admitted);
            return Ok(UpdateAnswer::View(wire::view(&progress)));
        }

        if self.attempt {
            return Err(refuse(Refused::Attempt));
        }
        match record.state {
            State::Pending(c) if c.digest != candidate.digest => {
                return Err(refuse(Refused::Pending));
            }
            State::Activated(c) if c.digest == candidate.digest => {
                return Err(refuse(Refused::Failed));
            }
            State::Pending(_) => {}
            State::None | State::Activated(_) => {
                self.store
                    .set_pending(candidate)
                    .map_err(|error| refuse(Refused::Store(error)))?;
            }
        }
        self.attempt = true;
        self.install = Install::new(admitted, 0);
        Ok(UpdateAnswer::View(self.view()))
    }

    fn take(&mut self, offset: u32, data: &[u8], now: u32) -> Result<UpdateView, Refusal> {
        let Some(install) = self.install.as_mut() else {
            return Err(wire::refusal(Refused::NotInstalling));
        };
        let mut data = data.to_vec();
        if let Some(at) = self.faults.flip
            && (offset..offset + 512).contains(&at)
        {
            data[(at - offset) as usize] ^= 1;
        }

        let result = match install.chunk(offset, &data, now) {
            Ok(Taken::Program(program)) => {
                let at = (program.offset - BANK1_OFFSET) as usize;
                self.bank1[at..at + program.data.len()].copy_from_slice(program.data);
                install.programmed(true)
            }
            Ok(_) => Ok(()),
            Err(stop) => Err(stop),
        };
        if result.is_ok() && install.ready() {
            let admitted = *install.admitted();
            let candidate = Candidate {
                digest: admitted.digest,
                version: admitted.signed.manifest.version,
                build_id: admitted.signed.manifest.build_id,
                kind: admitted.signed.kind,
            };
            // The activation is recorded before the marker, which goes last.
            self.store.set_activated(candidate).unwrap();
            let (at, marker) = install.commit().unwrap();
            let at = (at - BANK1_OFFSET) as usize;
            self.bank1[at..at + 8].copy_from_slice(&marker);
            install.marked(true).unwrap();
            self.attempt = false;
            self.restart = true;
        }
        let view = self.view();
        if let Err(stop) = result {
            self.end(stop);
        }
        Ok(view)
    }

    /// The install stopped in order: bank 1 stays invalid, the flag clears.
    fn end(&mut self, stop: Stop) {
        self.install = None;
        self.stopped = Some(stop);
        self.attempt = false;
    }

    fn tick(&mut self, now: u32) {
        if let Some(stop) = self.install.as_mut().and_then(|i| i.tick(now)) {
            self.end(stop);
        }
    }
}

impl Device for Feeder {
    fn random(&mut self, out: &mut [u8; 32]) -> bool {
        self.ticket = self.ticket.wrapping_add(1);
        *out = [u8::try_from(self.ticket % 251).unwrap(); 32];
        true
    }

    fn build(&self) -> &'static str {
        "update-test"
    }

    fn status(&mut self, now_ms: u32) -> Status<'static> {
        Status {
            build: "update-test",
            bank: self.bank,
            uptime_ms: now_ms,
            boot_count: 0,
            reset_flags: 0,
            faults: 0,
            rssi: -40,
            wifi: proto::WifiSource::Stored,
            link_losses: 0,
            dns_losses: 0,
            clock_ms: None,
            power: None,
            locked: Some(false),
            settings_generation: 1,
            wifi_generation: 1,
            last_event: None,
            sessions: 0,
        }
    }

    fn settings(&mut self, _change: SettingsFields) -> SettingsResult {
        SettingsResult::Current {
            view: SettingsView::default(),
            pending: false,
        }
    }

    fn wifi(&mut self, _change: Option<Option<Credentials>>) -> WifiResult {
        WifiResult::Current {
            view: WifiView {
                stored_ssid_crc: None,
                generation: 1,
                writable: true,
                source: WifiSource::Stored,
            },
            pending: false,
        }
    }

    fn lock(&mut self, locked: bool) -> LockResult {
        LockResult::Current {
            locked,
            pending: false,
        }
    }

    fn play(&mut self, _tune: Tune) -> Option<PlayResult> {
        None
    }

    fn time_sync(&mut self, _fields: TimeFields) -> TimeResult {
        TimeResult::Synced { jump_ms: None }
    }

    fn keys(&mut self, _change: KeyChange) -> Option<u32> {
        None
    }

    fn keys_outcome(&mut self, _ticket: u32) -> Option<KeyOutcome> {
        None
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::KNOWN
    }

    fn feed(&mut self, _portions: u8, _by: proto::Source) -> Result<u32, Refusal> {
        self.feeds += 1;
        Ok(self.feeds)
    }

    fn feed_reserved(&mut self, _feed: u32) -> Option<Result<(), Refusal>> {
        Some(Ok(()))
    }

    fn cancel_feed(&mut self, _feed: u32) -> bool {
        false
    }

    fn record(&mut self, _now_ms: u32, _body: EventBody) {}

    fn event_after(&mut self, _after: Option<u64>) -> EventRead {
        EventRead::None
    }

    fn epoch(&self) -> Option<u32> {
        Some(1)
    }

    fn reboot(&mut self) {}

    fn update(&mut self, request: UpdateRequest<'_>) -> Result<u32, Refusal> {
        let answer = match request {
            UpdateRequest::Offer {
                manifest,
                downgrade,
                device,
            } => self.offer(manifest, downgrade, device),
            UpdateRequest::Cancel => {
                if self.install.is_some() {
                    self.end(Stop::Cancelled);
                }
                self.store
                    .clear()
                    .map_err(|error| wire::refusal(Refused::Store(error)))?;
                Ok(UpdateAnswer::View(self.view()))
            }
            UpdateRequest::Statement { .. } => Err(Refusal::Unavailable),
        };
        self.ticket += 1;
        self.outcome = Some((self.ticket, answer));
        Ok(self.ticket)
    }

    fn update_outcome(&mut self, ticket: u32) -> Option<Result<UpdateAnswer, Refusal>> {
        let outcome = self
            .outcome
            .filter(|&(done, _)| done == ticket)
            .map(|(_, answer)| answer);
        // The server seals the answer this pass; the give-way follows it.
        if outcome.is_some() && core::mem::take(&mut self.restart_on_answer) {
            self.restart = true;
            self.give_way = true;
        }
        outcome
    }

    fn update_chunk(&mut self, offset: u32, data: &[u8]) -> Result<UpdateView, Refusal> {
        self.take(offset, data, 0)
    }

    fn update_view(&mut self) -> Option<UpdateView> {
        Some(self.view())
    }

    fn installing(&self) -> bool {
        self.install.as_ref().is_some_and(Install::receiving)
            || self.restart
            || self.restart_on_answer
    }
}

/// The feeder's server, the fake under it, and the owner's connection.
struct Rig {
    server: Box<Server>,
    feeder: Feeder,
    pairing: Pairing,
    identity: Keypair,
    now: u32,
    base: Instant,
    slot: Option<usize>,
    connection: Option<Connection>,
    outbox: Vec<u8>,
    ids: u64,
    /// What each request in flight was.
    sent: Vec<(RequestId, Sent)>,
}

impl Rig {
    fn new() -> Self {
        let mut rig = Self {
            server: Box::new(Server::new()),
            feeder: Feeder::new(),
            pairing: Pairing::new(Config::DEFAULT),
            identity: Keypair::from_secret(FEEDER),
            now: 1_000,
            base: Instant::now(),
            slot: None,
            connection: None,
            outbox: Vec::new(),
            ids: 0,
            sent: Vec::new(),
        };
        rig.restart_server();
        rig
    }

    fn instant(&self) -> Instant {
        self.base + Duration::from_millis(u64::from(self.now))
    }

    /// A fresh server: the feeder restarted and its RAM is gone.
    fn restart_server(&mut self) {
        *self.server = Server::new();
        let mut table: [Option<Client>; CLIENTS] = [None; CLIENTS];
        table[0] = Some(Client {
            key: public(OWNER),
            label: Label::new(b"owner").unwrap(),
            role: Role::Owner,
            enrolled: 1,
        });
        *self.server.clients_table().0 = table;
        self.server.clients_changed(1, |_| {});
        self.slot = None;
        self.connection = None;
        self.outbox.clear();
        self.sent.clear();
    }

    /// Open the owner's session.
    fn connect(&mut self) {
        let slot = self.server.accept(self.now, |_| {}).unwrap();
        let connection = Connection::new(
            &Keypair::from_secret(OWNER),
            public(FEEDER),
            [0x77; 32],
            self.instant(),
        )
        .unwrap();
        self.slot = Some(slot);
        self.connection = Some(connection);
        for _ in 0..50 {
            self.pass();
            if self.connection.as_ref().unwrap().opened().is_some() {
                return;
            }
        }
        panic!("no welcome");
    }

    /// Drop the connection from the client's side.
    fn disconnect(&mut self) {
        if let Some(slot) = self.slot.take() {
            self.server.closed(slot, |_| {});
        }
        self.connection = None;
        self.outbox.clear();
        self.sent.clear();
    }

    /// One pass of the feeder, moving bytes both ways; the answers.
    fn pass(&mut self) -> Vec<(Sent, Answer)> {
        self.now += 5;
        let mut answers = Vec::new();
        let (Some(slot), Some(connection)) = (self.slot, self.connection.as_mut()) else {
            return answers;
        };
        while let Some(bytes) = connection.poll_transmit() {
            self.outbox.extend(bytes);
        }
        let room = self.server.rx(slot);
        let n = room.len().min(self.outbox.len());
        room[..n].copy_from_slice(&self.outbox[..n]);
        self.server.received(slot, n);
        self.outbox.drain(..n);

        self.feeder.tick(self.now);
        self.server.step(
            Pass {
                now_ms: self.now,
                device: &mut self.feeder,
                pairing: &mut self.pairing,
                identity: Some(&self.identity),
                feeding: false,
                recovery: None,
                signer: None,
            },
            |_| {},
        );
        let out = self.server.tx(slot).to_vec();
        self.server.sent(slot, out.len());
        let now = self.base + Duration::from_millis(u64::from(self.now));
        if !out.is_empty() {
            connection.handle_input(now, &out).unwrap();
        }
        while let Some(event) = connection.poll_event() {
            if let Event::Answer { id, answer } = event
                && let Some(index) = self.sent.iter().position(|(sent, _)| *sent == id)
            {
                let (_, sent) = self.sent.swap_remove(index);
                answers.push((sent, answer));
            }
        }
        answers
    }

    fn request(&mut self, sent: Sent, op: Op) {
        self.ids += 1;
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.ids.to_le_bytes());
        let now = self.instant();
        self.connection
            .as_mut()
            .unwrap()
            .request(now, id, &op)
            .unwrap();
        self.sent.push((id, sent));
    }

    /// The feeder restarts once its answer is out.
    fn settle_restart(&mut self, installer: &mut Installer<'_>) -> bool {
        if !self.feeder.restart {
            return false;
        }
        self.disconnect();
        installer.lost();
        if self.feeder.give_way {
            assert!(self.feeder.bank0_valid, "never give way without a bank 0");
            self.feeder.bank1[0] = 0;
        }
        self.feeder.boot();
        self.restart_server();
        true
    }
}

/// Hooks a test runs as the install goes.
#[derive(Default)]
struct Script {
    /// After this chunk is sent, drop the connection once.
    disconnect_after: Option<u32>,
    /// After this chunk is sent, cut the power once.
    power_cut_after: Option<u32>,
    /// After this chunk is sent, reset as a watchdog would, once.
    crash_after: Option<u32>,
    /// Stop sending chunks from this offset on (a stalled client).
    stall_at: Option<u32>,
}

/// Run the installer against the rig until it activates or fails.
fn install(rig: &mut Rig, image: Image<'_>, script: &mut Script) -> Result<(), InstallError> {
    let mut installer = Installer::new(image);
    if rig.connection.is_none() {
        rig.connect();
    }
    installer.opened(rig.feeder.bank);

    for _ in 0..20_000 {
        loop {
            match installer.poll() {
                Next::Send(Sent::Chunk(offset), _)
                    if script.stall_at.is_some_and(|at| offset >= at) =>
                {
                    installer.lost();
                    // Nothing more is sent: let the feeder's clock run out.
                    for _ in 0..=wellfed_core::ota::install::IDLE_MS / 1000 {
                        rig.now += 1000;
                        rig.feeder.tick(rig.now);
                    }
                    script.stall_at = None;
                    rig.disconnect();
                    rig.connect();
                    installer.opened(rig.feeder.bank);
                    break;
                }
                Next::Send(sent, op) => {
                    rig.request(sent, op);
                    if let Sent::Chunk(offset) = sent {
                        if script
                            .disconnect_after
                            .take_if(|at| *at == offset)
                            .is_some()
                        {
                            rig.disconnect();
                            installer.lost();
                            rig.connect();
                            installer.opened(rig.feeder.bank);
                            break;
                        }
                        if script.power_cut_after.take_if(|at| *at == offset).is_some() {
                            rig.disconnect();
                            installer.lost();
                            rig.feeder.power_cut();
                            rig.restart_server();
                            rig.connect();
                            installer.opened(rig.feeder.bank);
                            break;
                        }
                        if script.crash_after.take_if(|at| *at == offset).is_some() {
                            rig.disconnect();
                            installer.lost();
                            rig.feeder.boot();
                            rig.restart_server();
                            rig.connect();
                            installer.opened(rig.feeder.bank);
                            break;
                        }
                    }
                }
                Next::Wait => break,
                Next::Reconnect => {
                    rig.connect();
                    installer.opened(rig.feeder.bank);
                }
                Next::Activated => {
                    // The feeder restarts into the new build.
                    rig.settle_restart(&mut installer);
                    return Ok(());
                }
            }
        }
        for (sent, outcome) in rig.pass() {
            installer.answer(sent, &outcome)?;
        }
        rig.settle_restart(&mut installer);
    }
    panic!("no progress");
}

/// An image, its manifest and no statement.
struct Offer {
    payload: Vec<u8>,
    manifest: Vec<u8>,
    downgrade: Option<Vec<u8>>,
}

impl Offer {
    fn new(version: u32, build: u8, len: usize) -> Self {
        let payload = payload(len, build);
        let manifest = manifest(&bench(), version, &payload, build, ImageKind::Replacement);
        Self {
            payload,
            manifest,
            downgrade: None,
        }
    }

    fn image(&self) -> Image<'_> {
        Image {
            manifest: &self.manifest,
            downgrade: self.downgrade.as_deref(),
            payload: &self.payload,
        }
    }
}

/// Bank 1 is back with `build`, confirmed: the floor is its version.
fn assert_running(rig: &Rig, build: u8, floor: u32) {
    assert_eq!(rig.feeder.bank, 1);
    assert_eq!(rig.feeder.bank1_build, [build; 32]);
    assert_eq!(rig.feeder.min(), floor);
    assert_eq!(rig.feeder.store.record().state, State::None);
}

/// A failure left bank 1 invalid: the feeder still boots bank 0, which
/// still answers and feeds.
fn assert_still_on_bank_0(rig: &mut Rig) {
    assert!(
        !rig.feeder.marked(),
        "bank 1's marker must never be written"
    );
    rig.feeder.boot();
    assert_eq!(rig.feeder.bank, 0);
    rig.restart_server();
    rig.connect();
    assert!(!rig.feeder.installing());
    // The request is tagged as a state read only to reach the answer.
    rig.request(Sent::State, Op::Feed { portions: 1 });
    let fed = (0..20).flat_map(|_| rig.pass()).next().map(|(_, o)| o);
    assert_eq!(
        fed,
        Some(Answer::Feeding {
            feed: rig.feeder.feeds
        })
    );
}

#[test]
fn an_offer_on_bank_1_gives_way_and_bank_0_installs_it() {
    let mut rig = Rig::new();
    let offer = Offer::new(2, 0x22, 3 * SECTOR + 700);
    install(&mut rig, offer.image(), &mut Script::default()).unwrap();
    assert_running(&rig, 0x22, 2);
    assert_eq!(&rig.feeder.bank1[..offer.payload.len()], &offer.payload[..]);
}

#[test]
fn a_dropped_connection_resumes_from_the_state() {
    let mut rig = Rig::new();
    let offer = Offer::new(2, 0x22, 3 * SECTOR);
    let mut script = Script {
        disconnect_after: Some(SECTOR as u32 + 1024),
        ..Script::default()
    };
    install(&mut rig, offer.image(), &mut script).unwrap();
    assert!(script.disconnect_after.is_none(), "the drop happened");
    assert_running(&rig, 0x22, 2);
}

#[test]
fn a_flipped_byte_stops_at_the_hash_before_activation() {
    let mut rig = Rig::new();
    rig.feeder.faults.flip = Some(SECTOR as u32 + 9);
    let offer = Offer::new(2, 0x22, 2 * SECTOR + 10);
    let stopped = install(&mut rig, offer.image(), &mut Script::default());
    assert_eq!(stopped, Err(InstallError::Stopped(model::UpdateStop::Hash)));
    assert_still_on_bank_0(&mut rig);
}

#[test]
fn a_stalled_client_times_out_before_activation() {
    let mut rig = Rig::new();
    let offer = Offer::new(2, 0x22, 3 * SECTOR);
    let mut script = Script {
        stall_at: Some(SECTOR as u32),
        ..Script::default()
    };
    let stopped = install(&mut rig, offer.image(), &mut script);
    assert_eq!(
        stopped,
        Err(InstallError::Stopped(model::UpdateStop::Timeout))
    );
    assert_still_on_bank_0(&mut rig);
}

#[test]
fn a_power_cut_mid_download_starts_over_on_bank_0() {
    let mut rig = Rig::new();
    let offer = Offer::new(2, 0x22, 3 * SECTOR);
    let mut script = Script {
        power_cut_after: Some(2 * SECTOR as u32),
        ..Script::default()
    };
    install(&mut rig, offer.image(), &mut script).unwrap();
    assert!(script.power_cut_after.is_none());
    assert_running(&rig, 0x22, 2);
}

#[test]
fn a_reset_mid_install_refuses_offers_until_a_power_cycle() {
    let mut rig = Rig::new();
    let offer = Offer::new(2, 0x22, 3 * SECTOR);
    let mut script = Script {
        crash_after: Some(SECTOR as u32),
        ..Script::default()
    };
    let refused = install(&mut rig, offer.image(), &mut script);
    assert_eq!(
        refused,
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Attempt
        )))
    );
    assert_still_on_bank_0(&mut rig);

    rig.feeder.power_cut();
    rig.restart_server();
    install(&mut rig, offer.image(), &mut Script::default()).unwrap();
    assert_running(&rig, 0x22, 2);
}

#[test]
fn bank_0_installs_only_the_manifest_bank_1_gave_way_for() {
    let mut rig = Rig::new();
    let first = Offer::new(2, 0x22, 2 * SECTOR);
    let mut installer = Installer::new(first.image());
    rig.connect();
    installer.opened(1);
    let Next::Send(sent, op) = installer.poll() else {
        panic!("an offer first");
    };
    rig.request(sent, op);
    for _ in 0..20 {
        for (sent, outcome) in rig.pass() {
            installer.answer(sent, &outcome).unwrap();
        }
    }
    assert!(rig.settle_restart(&mut installer));
    assert_eq!(rig.feeder.bank, 0);

    // Another signed manifest, offered to bank 0 in its place.
    let swapped = Offer::new(3, 0x33, 2 * SECTOR);
    let refused = install(&mut rig, swapped.image(), &mut Script::default());
    assert_eq!(
        refused,
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Pending
        )))
    );
    install(&mut rig, first.image(), &mut Script::default()).unwrap();
    assert_running(&rig, 0x22, 2);
}

#[test]
fn an_older_version_needs_a_statement_and_stock_always_does() {
    let mut rig = Rig::new();
    install(
        &mut rig,
        Offer::new(5, 0x55, SECTOR).image(),
        &mut Script::default(),
    )
    .unwrap();
    assert_running(&rig, 0x55, 5);

    let mut old = Offer::new(4, 0x44, SECTOR);
    let refused = install(&mut rig, old.image(), &mut Script::default());
    assert_eq!(
        refused,
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Downgrade
        )))
    );
    // A statement made at another floor, or for another feeder, is refused.
    old.downgrade = Some(downgrade(&bench(), &old.manifest, 4));
    let refused = install(&mut rig, old.image(), &mut Script::default());
    assert_eq!(
        refused,
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Statement
        )))
    );
    old.downgrade = Some(downgrade(&bench(), &old.manifest, 5));
    install(&mut rig, old.image(), &mut Script::default()).unwrap();
    assert_running(&rig, 0x44, 5);

    let stock_payload = payload(SECTOR, 0x99);
    let stock = manifest(&bench(), 1, &stock_payload, 0x99, ImageKind::Stock);
    let image = Image {
        manifest: &stock,
        downgrade: None,
        payload: &stock_payload,
    };
    let refused = install(&mut rig, image, &mut Script::default());
    assert_eq!(
        refused,
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Downgrade
        )))
    );
}

#[test]
fn an_unknown_key_and_a_version_1_manifest_are_refused() {
    let mut rig = Rig::new();
    let stranger = SigningKey::from_bytes(&[9; 32]);
    let payload = payload(SECTOR, 1);
    let foreign = manifest(&stranger, 9, &payload, 1, ImageKind::Replacement);
    let image = Image {
        manifest: &foreign,
        downgrade: None,
        payload: &payload,
    };
    assert_eq!(
        install(&mut rig, image, &mut Script::default()),
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Key
        )))
    );

    let mut v1 = vec![0; ota::MANIFEST_LEN];
    v1[..8].copy_from_slice(&ota::MAGIC);
    let image = Image {
        manifest: &v1,
        downgrade: None,
        payload: &payload,
    };
    assert_eq!(
        install(&mut rig, image, &mut Script::default()),
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Manifest
        )))
    );
    assert_eq!(rig.feeder.bank, 1);
    assert_eq!(rig.feeder.store.record().state, State::None);
}

#[test]
fn a_candidate_that_fails_its_trial_falls_back_and_is_not_installed_again() {
    let mut rig = Rig::new();
    rig.feeder.never_confirm = true;
    let offer = Offer::new(2, 0x22, SECTOR);
    install(&mut rig, offer.image(), &mut Script::default()).unwrap();
    assert_eq!(rig.feeder.bank, 1, "trial boot 1");
    rig.feeder.boot();
    rig.feeder.boot();
    assert_eq!(rig.feeder.bank, 0, "KM0 gave way after three boots");
    assert_eq!(
        rig.feeder.min(),
        0,
        "a failed candidate never raises the floor"
    );

    rig.restart_server();
    assert_eq!(
        install(&mut rig, offer.image(), &mut Script::default()),
        Err(InstallError::Refused(model::Refusal::Update(
            model::UpdateRefusal::Failed
        )))
    );
    rig.feeder.never_confirm = false;
    install(
        &mut rig,
        Offer::new(3, 0x33, SECTOR).image(),
        &mut Script::default(),
    )
    .unwrap();
    assert_running(&rig, 0x33, 3);
}
