//! FW10 recovery-role updater on KM4. Runs only when the image booted from
//! bank 0 (the factory loader's fallback), in place of the FW08 transport.
//!
//! Arming follows the FW15 probe: bind the KM0 peer, set the attempt marker,
//! open the watchdog and install the lease provider into the SDK flash-lock
//! hooks. Every bank-1 erase and page program then parks KM0 under one FW06
//! lease. A boot that finds the attempt marker set skips the update, so a
//! hook fault or hang resets at most once per power cycle.
//!
//! The updater then pulls a signed manifest and the image over its own MQTT
//! session (`ota/get` out, `ota/data` in) and verifies the manifest's Ed25519
//! signature against the build's key. The `wellfed-core::ota` writer stages
//! each sector, and every page is read back. The writer withholds bank 1's
//! marker until the whole image hashed correctly; the marker goes in last and
//! is read back. The SDK wrappers refuse bank 0, the bank this runs from.
//!
//! A dropped connection resumes at the next chunk the writer wants. A write,
//! readback, hash or manifest failure stops the update with bank 1 invalid,
//! so the loader keeps booting this recovery image.

use core::fmt;

use wellfed_core::named_enum;

use wellfed_core::flash_coord::FlashReply;
use wellfed_core::ota::{self, Manifest, Part, SECTOR, SECTOR_BYTES, Status, Writer};
use wellfed_core::transport::{self as mqtt, Event, Session, Tick};

pub use crate::arming::Hooks;
use crate::arming::{self, Arming, Poll};
use crate::flash::park::Mmio;
use crate::net_status;
use crate::provider::Provider;
use crate::sdk::{self, TcpRecv, TransportConfig};

// The SDK crate cannot depend on wellfed-core, so it keeps its own copies
// of the bank layout the updater writes; they must agree.
const _: () = assert!(
    sdk::BANK1_OFFSET == ota::BANK1_OFFSET
        && sdk::BANK1_SECTORS * sdk::SECTOR_BYTES == ota::BANK1_BYTES
        && sdk::SECTOR_BYTES == SECTOR_BYTES
        && u64::from_le_bytes(sdk::MARKER) == u64::from_le_bytes(ota::MARKER)
);

pub const CONNECT_TIMEOUT_MS: u32 = 3000;
pub const POLL_MS: u32 = 50;
pub const IDLE_MS: u32 = 500;
/// Ask again for an answer not received within this.
pub const ANSWER_MS: u32 = 3000;
/// Asks per part before the update stops.
pub const MAX_TRIES: u32 = 5;
/// Wait between manifest asks while the server has no offer.
pub const NO_OFFER_MS: u32 = 10_000;
/// Log progress every this many sectors.
pub const PROGRESS_SECTORS: u32 = 16;
/// One page program (one lease) at a time, each read back.
const PIECE: usize = 256;

/// Bank-1 flash access by sector index, as `sdk::bank1_*` take it.
pub trait BankFlash {
    fn erase(&mut self, sector: u32) -> bool;
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool;
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool;
}

/// Bank 1 through the SDK wrappers, refused unless the hooks are armed (an
/// unarmed lock would fall through to the SDK's own KM0 handshake).
pub struct Hooked<H>(core::marker::PhantomData<H>);

impl<H> Hooked<H> {
    pub const fn new() -> Self {
        Self(core::marker::PhantomData)
    }
}

impl<H> Default for Hooked<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Hooks> BankFlash for Hooked<H> {
    fn erase(&mut self, sector: u32) -> bool {
        H::armed() && sdk::bank1_erase(sector)
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        H::armed() && sdk::bank1_program(sector, offset, data)
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        sdk::bank1_read(sector, offset, bytes)
    }
}

/// The build's update-signing public key, from `WELLFED_OTA_PUBLIC_KEY`
/// (64 lowercase hex characters). Without one the updater does not run.
pub fn public_key() -> Option<[u8; 32]> {
    parse_key(option_env!("WELLFED_OTA_PUBLIC_KEY")?)
}

pub fn parse_key(text: &str) -> Option<[u8; 32]> {
    wellfed_core::hex::decode_array(text.as_bytes())
}

named_enum! {
    /// Where an update stopped (`error stage=`).
    pub enum ErrorStage {
        Clock = "clock",
        Timeout = "timeout",
        Topic = "topic",
        Range = "range",
        Order = "order",
        Hash = "hash",
        Write = "write",
        State = "state",
        Key = "key",
        Manifest = "manifest",
        Marker = "marker",
    }
}

named_enum! {
    /// Why the update's connection dropped (`closed reason=`); the next one
    /// resumes.
    pub enum CloseReason {
        Wifi = "wifi",
        Closed = "closed",
        Protocol = "protocol",
        Send = "send",
        Timeout = "timeout",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Start {
        key: bool,
    },
    Ready {
        reply: FlashReply,
        inactive: bool,
    },
    Timeout,
    /// The attempt marker was set: an earlier attempt this power cycle reset.
    Skip {
        marker: u32,
    },
    Armed {
        watchdog: bool,
        installed: bool,
    },

    Connect {
        ok: bool,
    },
    Closed {
        reason: CloseReason,
    },
    NoOffer,

    Manifest {
        version: u32,
        bytes: u32,
    },
    Progress {
        sectors: u32,
        total: u32,
        leases: u64,
    },
    Activated {
        version: u32,
        bytes: u32,
        sectors: u32,
    },
    Error {
        stage: ErrorStage,
    },
}

impl From<arming::Event> for Record {
    fn from(event: arming::Event) -> Self {
        match event {
            arming::Event::Ready { reply, inactive } => Self::Ready { reply, inactive },
            arming::Event::Timeout => Self::Timeout,
            arming::Event::Skip { marker } => Self::Skip { marker },
            arming::Event::Armed {
                watchdog,
                installed,
            } => Self::Armed {
                watchdog,
                installed,
            },
            arming::Event::Clock => Self::Error {
                stage: ErrorStage::Clock,
            },
        }
    }
}

/// The body of a `PLOTA1` line. No record carries a credential.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Start { key } => write!(f, "start role=recovery key={}", u8::from(key)),
        Record::Ready { reply, inactive } => {
            write!(f, "ready reply={} inactive={}", reply.0, u8::from(inactive))
        }
        Record::Timeout => f.write_str("timeout"),
        Record::Skip { marker } => write!(f, "skip marker={marker:#x}"),
        Record::Armed {
            watchdog,
            installed,
        } => write!(
            f,
            "armed watchdog={} installed={}",
            u8::from(watchdog),
            u8::from(installed)
        ),

        Record::Connect { ok } => write!(f, "connect ok={}", u8::from(ok)),
        Record::Closed { reason } => write!(f, "closed reason={reason}"),
        Record::NoOffer => f.write_str("no_offer"),
        Record::Manifest { version, bytes } => {
            write!(f, "manifest version={version} bytes={bytes}")
        }

        Record::Progress {
            sectors,
            total,
            leases,
        } => write!(f, "progress sectors={sectors}/{total} leases={leases}"),
        Record::Activated {
            version,
            bytes,
            sectors,
        } => write!(
            f,
            "activated version={version} bytes={bytes} sectors={sectors}"
        ),
        Record::Error { stage } => write!(f, "error stage={stage}"),
    }
}

/// `delay_ms == 0` ends the task. `restart` asks the glue to let the watchdog
/// reset the platform so the loader boots the new bank 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    pub restart: bool,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        restart: false,
    }
}

pub(crate) struct Topic {
    buf: [u8; ota::TOPIC_BUDGET],
    len: usize,
}

impl Topic {
    pub(crate) fn new(namespace: &str, suffix: &str) -> Option<Self> {
        let mut buf = [0; ota::TOPIC_BUDGET];
        let len = mqtt::topic(&mut buf, namespace, suffix).ok()?.len();
        Some(Self { buf, len })
    }

    pub(crate) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

/// The outstanding ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wanted {
    part: Part,
    offset: u32,
    len: u32,
    asked_at: u32,
    tries: u32,
}

#[derive(Clone, Copy, Debug)]
struct Open {
    socket: i32,
    session: Session,
    generation: u32,
    data_ready: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Stage {
    #[default]
    Start,
    Arm,
    Net,
    Done,
}

/// Keep at a fixed address once started: the provider is reached by pointer.
pub struct Updater {
    config: TransportConfig,
    key: Option<[u8; 32]>,
    stage: Stage,
    arming: Arming,
    open: Option<Open>,
    rx: [u8; 2 * mqtt::MAX_PACKET],
    rx_len: usize,
    wanted: Option<Wanted>,
    writer: Option<Writer>,
    sectors: u32,
    leases: u64,
    no_offer_at: Option<u32>,
}

impl Updater {
    pub fn new(config: TransportConfig, key: Option<[u8; 32]>) -> Self {
        Self {
            config,
            key,
            stage: Stage::Start,
            arming: Arming::default(),
            open: None,
            rx: [0; 2 * mqtt::MAX_PACKET],
            rx_len: 0,
            wanted: None,
            writer: None,
            sectors: 0,
            leases: 0,
            no_offer_at: None,
        }
    }

    /// The hooks are armed and the update is still running.
    pub fn networking(&self) -> bool {
        self.stage == Stage::Net
    }

    fn end(&mut self, record: Record) -> Step {
        self.stage = Stage::Done;
        next(0, Some(record))
    }

    /// Stop for good with bank 1 invalid. The stop is orderly, so the
    /// attempt marker is cleared.
    fn fail(&mut self, stage: ErrorStage) -> Step {
        self.close();
        arming::clear_attempt();
        self.end(Record::Error { stage })
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            sdk::tcp_close(open.socket);
        }
        self.rx_len = 0;
        // The writer keeps its place: the next connection resumes there.
        self.wanted = None;
    }

    fn drop_link(&mut self, reason: CloseReason) -> Step {
        self.close();
        next(IDLE_MS, Some(Record::Closed { reason }))
    }

    /// One task iteration in task context.
    ///
    /// # Safety
    /// `provider` is valid for the updater's lifetime, stays at a fixed
    /// address and is not otherwise borrowed.
    pub unsafe fn step<M: Mmio, H: Hooks>(
        &mut self,
        provider: *mut Provider,
        flash: &mut impl BankFlash,
    ) -> Step {
        match self.stage {
            Stage::Start => {
                if self.key.is_none() {
                    return self.end(Record::Start { key: false });
                }
                self.stage = Stage::Arm;
                next(POLL_MS, Some(Record::Start { key: true }))
            }

            // SAFETY: the caller's contract.
            Stage::Arm => match unsafe { self.arming.step::<M, H>(provider) } {
                Poll::Pending(event) => next(POLL_MS, event.map(Record::from)),
                Poll::Armed(event) => {
                    self.stage = Stage::Net;
                    next(POLL_MS, Some(event.into()))
                }
                Poll::Failed(event) => self.end(event.into()),
            },

            Stage::Net => {
                let step = self.net(flash);
                // SAFETY: the caller's contract; no hook call is in progress.
                self.leases = unsafe { (*provider).leases };
                step
            }

            Stage::Done => next(0, None),
        }
    }

    fn net(&mut self, flash: &mut impl BankFlash) -> Step {
        let wifi = net_status::read();
        let Some(open) = self.open else {
            return self.connect(wifi.dns_ok, wifi.generation);
        };
        if open.generation != wifi.generation {
            return self.drop_link(CloseReason::Wifi);
        }

        match sdk::tcp_recv(open.socket, &mut self.rx[self.rx_len..], POLL_MS) {
            TcpRecv::Data(n) => self.rx_len += n,
            TcpRecv::Timeout => {}
            TcpRecv::Closed => return self.drop_link(CloseReason::Closed),
        }

        let now = sdk::uptime_ms();
        loop {
            let used = match mqtt::decode(&self.rx[..self.rx_len]) {
                Ok(None) => break,
                Ok(Some((_, used))) => used,
                Err(_) => return self.drop_link(CloseReason::Protocol),
            };
            let mut frame = [0u8; mqtt::MAX_PACKET];
            frame[..used].copy_from_slice(&self.rx[..used]);
            self.rx.copy_within(used..self.rx_len, 0);
            self.rx_len -= used;
            if let Some(step) = self.handle(now, &frame[..used], flash) {
                return step;
            }
        }

        let tick = match self.open.as_mut() {
            Some(open) => open.session.tick(now),
            None => return next(IDLE_MS, None),
        };

        match tick {
            Tick::Idle => {}
            Tick::Ping => {
                let mut out = [0; 2];
                if !mqtt::encode_pingreq(&mut out).is_ok_and(|n| self.send(&out[..n])) {
                    return self.drop_link(CloseReason::Send);
                }
            }
            Tick::Closed(_) => return self.drop_link(CloseReason::Timeout),
        }

        self.retry(now)
    }

    fn connect(&mut self, dns_ok: bool, generation: u32) -> Step {
        if !dns_ok {
            return next(IDLE_MS, None);
        }

        let config = self.config;
        let Some(socket) = sdk::tcp_connect(config.broker, config.port, CONNECT_TIMEOUT_MS) else {
            return next(IDLE_MS * 4, Some(Record::Connect { ok: false }));
        };

        let mut out = [0; mqtt::MAX_PACKET];
        let connect = mqtt::Connect {
            client_id: config.client_id,
            username: config.username,
            password: config.password,
            will: None,
        };
        let sent = mqtt::encode_connect(&mut out, &connect)
            .is_ok_and(|n| sdk::tcp_send(socket, &out[..n]));
        if !sent {
            sdk::tcp_close(socket);
            return next(IDLE_MS * 4, Some(Record::Connect { ok: false }));
        }

        self.open = Some(Open {
            socket,
            session: Session::connecting(sdk::uptime_ms()),
            generation,
            data_ready: false,
        });
        self.rx_len = 0;
        next(POLL_MS, Some(Record::Connect { ok: true }))
    }

    fn send(&mut self, bytes: &[u8]) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        let ok = sdk::tcp_send(open.socket, bytes);
        if ok {
            open.session.sent(sdk::uptime_ms());
        }
        ok
    }

    fn ask(&mut self, now: u32, part: Part, offset: u32, len: u32, tries: u32) -> bool {
        let Some(topic) = Topic::new(self.config.namespace, "ota/get") else {
            return false;
        };

        let mut out = [0; ota::TOPIC_BUDGET + ota::GET_LEN + 8];
        let get = ota::encode_get(part, offset, len);
        let ok = mqtt::encode_publish(&mut out, topic.as_str(), &get, false)
            .is_ok_and(|n| self.send(&out[..n]));
        if ok {
            self.wanted = Some(Wanted {
                part,
                offset,
                len,
                asked_at: now,
                tries,
            });
        }
        ok
    }

    /// Ask for the manifest, or for the writer's next chunk.
    fn ask_next(&mut self, now: u32) -> bool {
        match self.writer.as_ref().map(Writer::next_request) {
            None => self.ask(now, Part::Manifest, 0, 0, 1),
            Some(Some((offset, len))) => self.ask(now, Part::Chunk, offset, len, 1),
            Some(None) => true,
        }
    }

    fn retry(&mut self, now: u32) -> Step {
        if !self.open.is_some_and(|o| o.data_ready) {
            return next(POLL_MS, None);
        }

        if let Some(at) = self.no_offer_at {
            if now.wrapping_sub(at) >= NO_OFFER_MS {
                self.no_offer_at = None;
                if !self.ask_next(now) {
                    return self.drop_link(CloseReason::Send);
                }
            }
            return next(POLL_MS, None);
        }

        let Some(w) = self.wanted else {
            return next(POLL_MS, None);
        };
        if now.wrapping_sub(w.asked_at) < ANSWER_MS {
            return next(POLL_MS, None);
        }

        if w.tries >= MAX_TRIES {
            return self.fail(ErrorStage::Timeout);
        }
        if !self.ask(now, w.part, w.offset, w.len, w.tries + 1) {
            return self.drop_link(CloseReason::Send);
        }
        next(POLL_MS, None)
    }

    /// One received packet; `Some` ends this iteration.
    fn handle(&mut self, now: u32, frame: &[u8], flash: &mut impl BankFlash) -> Option<Step> {
        let Ok(Some((packet, _))) = mqtt::decode(frame) else {
            return Some(self.drop_link(CloseReason::Protocol));
        };
        let (Some(request), Some(data)) = (
            Topic::new(self.config.namespace, "request"),
            Topic::new(self.config.namespace, "ota/data"),
        ) else {
            return Some(self.fail(ErrorStage::Topic));
        };

        let open = self.open.as_mut()?;
        let event = open
            .session
            .on_packet_with(now, packet, request.as_str(), Some(data.as_str()));

        match event {
            Event::Connected => {
                let mut out = [0; 128];
                let ok = open
                    .session
                    .subscribe(now, &mut out, request.as_str())
                    .is_ok_and(|n| sdk::tcp_send(open.socket, &out[..n]));
                (!ok).then(|| self.drop_link(CloseReason::Send))
            }

            Event::Ready => {
                let mut out = [0; 128];
                let ok = open
                    .session
                    .subscribe_data(now, &mut out, data.as_str())
                    .is_ok_and(|n| sdk::tcp_send(open.socket, &out[..n]));
                (!ok).then(|| self.drop_link(CloseReason::Send))
            }

            Event::DataReady => {
                open.data_ready = true;
                self.no_offer_at = None;
                (!self.ask_next(now)).then(|| self.drop_link(CloseReason::Send))
            }

            // Diagnostic requests are not served in the recovery role.
            Event::Pong | Event::Request(_) => None,
            Event::Data(payload) => self.data(now, payload, flash),
            Event::Closed(_) => Some(self.drop_link(CloseReason::Protocol)),
        }
    }

    fn data(&mut self, now: u32, payload: &[u8], flash: &mut impl BankFlash) -> Option<Step> {
        let answer = ota::decode_data(payload)?;
        let wanted = self.wanted?;
        if answer.part != wanted.part || answer.offset != wanted.offset {
            return None; // A late answer to an earlier ask.
        }

        self.wanted = None;
        match answer.status {
            Status::Ok => {}
            Status::NoOffer => {
                self.no_offer_at = Some(now);
                return Some(next(POLL_MS, Some(Record::NoOffer)));
            }
            Status::Range => return Some(self.fail(ErrorStage::Range)),
        }

        if answer.part == Part::Manifest {
            return Some(self.pin_manifest(now, answer.bytes));
        }

        let writer = self.writer.as_mut()?;
        let written = match writer.accept(answer.offset, answer.bytes) {
            Err(_) => return Some(self.fail(ErrorStage::Order)),
            Ok(None) => None,
            Ok(Some(program)) => {
                let sector = (program.offset - ota::BANK1_OFFSET) / SECTOR_BYTES;
                Some(write_sector(flash, sector, program.data))
            }
        };

        let Some(ok) = written else {
            return (!self.ask_next(now)).then(|| self.drop_link(CloseReason::Send));
        };

        if let Err(error) = writer.programmed(ok) {
            return Some(self.fail(match error {
                ota::WriteError::Hash => ErrorStage::Hash,
                ota::WriteError::Readback => ErrorStage::Write,
                _ => ErrorStage::State,
            }));
        }

        self.sectors += 1;
        let manifest = *writer.manifest();
        if writer.next_request().is_none() {
            return Some(self.activate(flash, manifest));
        }

        if !self.ask_next(now) {
            return Some(self.drop_link(CloseReason::Send));
        }

        self.sectors.is_multiple_of(PROGRESS_SECTORS).then(|| {
            let record = Record::Progress {
                sectors: self.sectors,
                total: manifest.image_len.div_ceil(SECTOR_BYTES),
                leases: self.leases,
            };
            next(POLL_MS, Some(record))
        })
    }

    /// Verify the first manifest and pin a writer for it. Kept out of line:
    /// the signature check and the writer's sector buffer need about 12 KiB
    /// of stack, which must not stack on the chunk path (observed: a
    /// HardFault at 10 KiB on hardware).
    #[inline(never)]
    fn pin_manifest(&mut self, now: u32, bytes: &[u8]) -> Step {
        if self.writer.is_some() {
            return next(POLL_MS, None); // Already pinned; only the first manifest counts.
        }

        let Some(key) = self.key else {
            return self.fail(ErrorStage::Key);
        };
        let Ok(manifest) = ota::parse_manifest(bytes, &key, 0) else {
            return self.fail(ErrorStage::Manifest);
        };

        self.writer = Writer::new(manifest, ota::MAX_CHUNK as usize);
        if self.writer.is_none() || !self.ask_next(now) {
            return self.drop_link(CloseReason::Send);
        }

        next(
            POLL_MS,
            Some(Record::Manifest {
                version: manifest.version,
                bytes: manifest.image_len,
            }),
        )
    }

    /// Program the withheld marker last and read it back: bank 1 is valid.
    fn activate(&mut self, flash: &mut impl BankFlash, manifest: Manifest) -> Step {
        let Some(Ok((offset, marker))) = self.writer.as_mut().map(Writer::commit) else {
            return self.fail(ErrorStage::State);
        };

        let mut readback = [0u8; 8];
        let ok = offset == ota::BANK1_OFFSET
            && flash.program(0, 0, &marker)
            && flash.read(0, 0, &mut readback)
            && readback == marker;
        if !ok {
            return self.fail(ErrorStage::Marker);
        }

        self.close();
        arming::clear_attempt();
        self.stage = Stage::Done;

        Step {
            delay_ms: 0,
            record: Some(Record::Activated {
                version: manifest.version,
                bytes: manifest.image_len,
                sectors: self.sectors,
            }),
            restart: true,
        }
    }
}

/// Erase one sector, program it page by page and read every page back.
/// A short last page is padded with erased bytes to the 4-byte program unit.
fn write_sector(flash: &mut impl BankFlash, sector: u32, data: &[u8]) -> bool {
    if data.len() > SECTOR || !flash.erase(sector) {
        return false;
    }

    let mut page = [u8::MAX; PIECE];
    let mut readback = [0u8; PIECE];
    for (n, piece) in data.chunks(PIECE).enumerate() {
        let offset = (n * PIECE) as u32;
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
