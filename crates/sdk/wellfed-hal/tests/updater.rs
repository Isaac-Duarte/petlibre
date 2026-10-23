//! FW10 recovery-role updater against the fake SDK: a scripted broker that
//! answers `ota/get` from a signed offer, the fake bank-1 NOR model (running
//! from bank 0), and the KM0 peer answering HELLO through the real
//! `park::answer`. Flash leases are not modelled here; `flash_probe` covers
//! the provider.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, MutexGuard};

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MAGIC, MANIFEST_BODY, MARKER, Part, Status};
use wellfed_hal::flash::park::{self, Mmio};
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::provider::{ATTEMPT_BIT, Provider};
use wellfed_hal::sdk::WifiEvent;
use wellfed_hal::sdk::fake::api::FAKE_TRANSPORT;
use wellfed_hal::sdk::fake::{self, TcpRecvScript};
use wellfed_hal::updater::*;
use wellfed_hal::{net_status, stopped};
use sha2::{Digest, Sha256};

/// The fake hardware is per thread but `net_status` is process-global, so
/// tests that join Wi-Fi run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static WATCHDOG: Cell<bool> = const { Cell::new(true) };
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
    let core = match address & 0xffff_f000 {
        0x4800_6000 => 0,
        0x4000_6000 => 1,
        _ => panic!("unexpected address {address:#x}"),
    };
    (core, (((address & 0xfff) - 0x20) / 4) as usize)
}

struct FakeHooks;

impl Hooks for FakeHooks {
    fn watchdog() -> bool {
        WATCHDOG.with(Cell::get)
    }
    fn install(provider: *mut Provider) -> bool {
        // SAFETY: the test's provider outlives the run.
        let ready = unsafe { (*provider).ready() };
        ARMED.with(|a| a.set(ready));
        ready
    }
    fn armed() -> bool {
        ARMED.with(Cell::get)
    }
}

/// Bank 1 through the real `Hooked` wrapper, with optional failures.
#[derive(Default)]
struct Flash {
    inner: Hooked<FakeHooks>,
    /// Fail the program call with this index (counted from zero).
    fail_program: Option<usize>,
    programs: usize,
    erases: usize,
    /// Bank 1's first 8 bytes before each program call.
    marker_seen: Vec<[u8; 8]>,
}

impl BankFlash for Flash {
    fn erase(&mut self, sector: u32) -> bool {
        self.erases += 1;
        self.inner.erase(sector)
    }
    fn program(&mut self, sector: u32, offset: u32, data: &[u8]) -> bool {
        self.marker_seen
            .push(fake::with(|h| h.bank1[..8].try_into().unwrap()));
        let n = self.programs;
        self.programs += 1;
        if self.fail_program == Some(n) {
            return false;
        }
        self.inner.program(sector, offset, data)
    }
    fn read(&mut self, sector: u32, offset: u32, bytes: &mut [u8]) -> bool {
        self.inner.read(sector, offset, bytes)
    }
}

const SEED: [u8; 32] = [7; 32];

fn image(len: usize) -> Vec<u8> {
    let mut out: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
    out[..8].copy_from_slice(&MARKER);
    out
}

fn manifest(signing: &SigningKey, version: u32, hashed: &[u8], len: usize) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&MAGIC);
    body.extend_from_slice(&version.to_le_bytes());
    body.extend_from_slice(&(len as u32).to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&Sha256::digest(hashed));
    body.extend_from_slice(&[0xab; 32]);
    assert_eq!(body.len(), MANIFEST_BODY);

    let signature = signing.sign(&body).to_bytes();
    body.extend_from_slice(&signature);
    body
}

fn wifi(kind: u32, result: i32, value: u32) {
    net_status::observe(&WifiEvent {
        kind,
        result,
        value,
        attempt: 1,
        rssi: -20,
        ..WifiEvent::default()
    });
}

fn key() -> [u8; 32] {
    SigningKey::from_bytes(&SEED).verifying_key().to_bytes()
}

/// Running from bank 0, joined with working DNS, outputs stopped.
fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    net_status::reset();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));
    WATCHDOG.with(|w| w.set(true));

    fake::with(|h| {
        h.interrupts = 1;
        h.running_bank = Some(0);
    });
    assert!(stopped::initialize().verified());

    // Scan found, DHCP assigned, DNS ok.
    wifi(2, 0, 1);
    wifi(4, 2, 0x0a2a_6705);
    wifi(7, 0, 0x5db8_d822);
    guard
}

/// The broker and the server's offer, answering everything the updater sent.
struct Broker {
    seen: usize,
    manifest: Option<Vec<u8>>,
    image: Vec<u8>,
    /// Ignore this many chunk asks (a lossy link).
    drop_chunks: usize,
    /// Close the connection after answering this many chunks.
    close_after: Option<usize>,
    chunks: usize,
    gets: Vec<(Part, u32, u32)>,
    /// A socket the broker closed: what it sends later is never answered.
    dead: Option<i32>,
}

impl Broker {
    fn new(manifest: Option<Vec<u8>>, image: Vec<u8>) -> Self {
        Self {
            seen: 0,
            manifest,
            image,
            drop_chunks: 0,
            close_after: None,
            chunks: 0,
            gets: Vec::new(),
            dead: None,
        }
    }

    fn reply(&mut self) {
        let sent: Vec<(i32, Vec<u8>)> = fake::with(|h| h.tcp_tx[self.seen..].to_vec());
        self.seen += sent.len();

        for (socket, packet) in sent {
            if self.dead == Some(socket) {
                continue;
            }

            match packet[0] {
                0x10 => rx(&[0x20, 2, 0, 0]),
                0x82 => {
                    let at = if packet[1] & 0x80 != 0 { 3 } else { 2 };
                    rx(&[0x90, 3, packet[at], packet[at + 1], 0]);
                }
                0xc0 => rx(&[0xd0, 0]),
                0x30 => self.publish(socket, &packet),
                other => panic!("unexpected packet {other:#x}"),
            }
        }
    }

    fn publish(&mut self, socket: i32, packet: &[u8]) {
        let (length, at) = if packet[1] & 0x80 != 0 {
            ((packet[1] & 0x7f) as usize + 128 * packet[2] as usize, 3)
        } else {
            (packet[1] as usize, 2)
        };

        assert_eq!(packet.len(), at + length);
        let n = u16::from_be_bytes([packet[at], packet[at + 1]]) as usize;
        let topic = std::str::from_utf8(&packet[at + 2..at + 2 + n]).unwrap();
        assert_eq!(topic, format!("{}ota/get", FAKE_TRANSPORT.namespace));
        let (part, offset, len) = ota::decode_get(&packet[at + 2 + n..]).unwrap();
        self.gets.push((part, offset, len));

        let mut answer = Vec::new();
        match (part, &self.manifest) {
            (_, None) => answer.extend(ota::encode_data_header(part, Status::NoOffer, offset, 0)),
            (Part::Manifest, Some(m)) => {
                answer.extend(ota::encode_data_header(part, Status::Ok, 0, m.len() as u32));
                answer.extend(m);
            }
            (Part::Chunk, Some(_)) => {
                if self.drop_chunks > 0 {
                    self.drop_chunks -= 1;
                    return;
                }

                let end = offset as usize + len as usize;
                if end > self.image.len() || len > ota::MAX_CHUNK {
                    answer.extend(ota::encode_data_header(part, Status::Range, offset, 0));
                } else {
                    answer.extend(ota::encode_data_header(part, Status::Ok, offset, len));
                    answer.extend(&self.image[offset as usize..end]);
                }

                self.chunks += 1;
            }
        }

        rx(&publish_packet(
            &format!("{}ota/data", FAKE_TRANSPORT.namespace),
            &answer,
        ));

        if self.close_after == Some(self.chunks) {
            self.close_after = None;
            self.dead = Some(socket);
            fake::with(|h| h.tcp_rx.push_back(TcpRecvScript::Closed));
        }
    }
}

fn rx(data: &[u8]) {
    fake::with(|h| h.tcp_rx.push_back(TcpRecvScript::Data(data.to_vec())));
}

fn publish_packet(topic: &str, payload: &[u8]) -> Vec<u8> {
    let length = 2 + topic.len() + payload.len();
    let mut out = vec![0x30];
    if length < 128 {
        out.push(length as u8);
    } else {
        out.extend([(length % 128) as u8 | 0x80, (length / 128) as u8]);
    }
    out.extend((topic.len() as u16).to_be_bytes());
    out.extend(topic.as_bytes());
    out.extend(payload);
    out
}

struct Run {
    records: Vec<Record>,
    restart: bool,
    ended: bool,
}

fn run(updater: &mut Updater, broker: &mut Broker, flash: &mut Flash, steps: usize) -> Run {
    let mut provider = Provider::default();
    let mut answered = 0;
    let mut records = Vec::new();

    for _ in 0..steps {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        // SAFETY: `provider` outlives the run and is not otherwise borrowed.
        let step = unsafe { updater.step::<Core<false>, FakeHooks>(&mut provider, flash) };
        records.extend(step.record);
        if step.delay_ms == 0 {
            return Run {
                records,
                restart: step.restart,
                ended: true,
            };
        }

        broker.reply();
        fake::advance(step.delay_ms * 32, false);
        fake::with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(step.delay_ms));
    }
    Run {
        records,
        restart: false,
        ended: false,
    }
}

fn connect_ok() {
    fake::with(|h| h.tcp_connects.extend([Some(5), Some(6), Some(7)]));
}

const LEN: usize = 3 * 4096 + 1002;

#[test]
fn installs_bank_1_with_its_marker_last_and_asks_for_a_restart() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload.clone());
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 500);
    assert!(result.ended && result.restart, "{:?}", result.records);
    assert_eq!(
        result.records.last(),
        Some(&Record::Activated {
            version: 3,
            bytes: LEN as u32,
            sectors: 4
        })
    );
    assert!(result.records.contains(&Record::Armed {
        watchdog: true,
        installed: true
    }));

    let bank1 = fake::with(|h| h.bank1.clone());
    assert_eq!(&bank1[..LEN], &payload[..]);
    assert!(bank1[LEN..].iter().all(|&b| b == 0xff));

    // The marker was erased before every program but the last one.
    let (last, earlier) = flash.marker_seen.split_last().unwrap();
    assert!(earlier[1..].iter().all(|m| *m == [0xff; 8]));
    assert_eq!(*last, [0xff; 8]);
    assert_eq!(flash.erases, 4);

    // 16 pages per full sector, 4 for the last, then the marker.
    assert_eq!(flash.programs, 3 * 16 + 4 + 1);

    // Chunks were asked in order, each once.
    let chunks: Vec<_> = broker
        .gets
        .iter()
        .filter(|g| g.0 == Part::Chunk)
        .map(|g| g.1)
        .collect();
    assert_eq!(chunks, (0..LEN as u32).step_by(512).collect::<Vec<_>>());
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
}

#[test]
fn a_sectors_chunks_are_asked_together_and_answers_poll_again_at_once() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload);
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));
    let mut provider = Provider::default();
    let mut answered = 0;

    // Step until the manifest is pinned; that step asks for the whole first
    // sector before any of it arrives.
    let mut pinned = None;
    for _ in 0..100 {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        // SAFETY: `provider` outlives the loop and is not otherwise borrowed.
        let step = unsafe { updater.step::<Core<false>, FakeHooks>(&mut provider, &mut flash) };
        broker.reply();
        if matches!(step.record, Some(Record::Manifest { .. })) {
            pinned = Some(step);
            break;
        }
        fake::advance(step.delay_ms * 32, false);
        fake::with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(step.delay_ms));
    }
    let Some(pinned) = pinned else {
        panic!("the manifest was never pinned");
    };

    let chunks: Vec<_> = broker
        .gets
        .iter()
        .filter(|g| g.0 == Part::Chunk)
        .map(|g| g.1)
        .collect();
    assert_eq!(chunks, (0..4096).step_by(512).collect::<Vec<_>>());
    assert_eq!(pinned.delay_ms, BUSY_MS);
}

#[test]
fn a_manifest_signed_by_another_key_writes_nothing() {
    let _guard = start();
    connect_ok();
    let other = SigningKey::from_bytes(&[9; 32]);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&other, 3, &payload, LEN)), payload);
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 200);
    assert!(result.ended && !result.restart);

    assert_eq!(
        result.records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Manifest
        })
    );
    assert_eq!(flash.erases + flash.programs, 0);
    assert!(broker.gets.iter().all(|g| g.0 == Part::Manifest));
}

#[test]
fn a_hash_mismatch_leaves_bank_1_without_its_marker() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut tampered = payload.clone();
    tampered[9000] ^= 1;
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), tampered);
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 500);
    assert!(result.ended && !result.restart);

    assert_eq!(
        result.records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Hash
        })
    );
    assert_eq!(fake::with(|h| h.bank1[..8].to_vec()), vec![0xff; 8]);
    assert_eq!(flash.programs, 3 * 16 + 4);
}

#[test]
fn a_failed_page_program_stops_the_update() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload);
    let mut flash = Flash {
        fail_program: Some(20),
        ..Flash::default()
    };
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 500);

    assert_eq!(
        result.records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Write
        })
    );
    assert_eq!(flash.programs, 21);
    assert_eq!(fake::with(|h| h.bank1[..8].to_vec()), vec![0xff; 8]);
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
}

#[test]
fn a_dropped_connection_resumes_at_the_next_chunk() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload.clone());
    broker.close_after = Some(10);
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 500);
    assert!(result.restart, "{:?}", result.records);

    assert!(result.records.contains(&Record::Closed {
        reason: CloseReason::Closed
    }));
    assert_eq!(
        result
            .records
            .iter()
            .filter(|r| **r == Record::Connect { ok: true })
            .count(),
        2
    );

    let chunks: Vec<_> = broker
        .gets
        .iter()
        .filter(|g| g.0 == Part::Chunk)
        .map(|g| g.1)
        .collect();
    assert_eq!(chunks, (0..LEN as u32).step_by(512).collect::<Vec<_>>());

    // The manifest was asked once: the pinned writer kept its place.
    assert_eq!(
        broker.gets.iter().filter(|g| g.0 == Part::Manifest).count(),
        1
    );
    assert_eq!(&fake::with(|h| h.bank1.clone())[..LEN], &payload[..]);
}

#[test]
fn lost_answers_are_asked_again_then_the_update_stops() {
    let _guard = start();
    connect_ok();
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload.clone());
    broker.drop_chunks = 2;
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    assert!(run(&mut updater, &mut broker, &mut flash, 1000).restart);

    // The first sector's chunks go out together; the two lost answers are
    // asked for again, each once.
    let asks = |offset| {
        broker
            .gets
            .iter()
            .filter(|g| **g == (Part::Chunk, offset, 512))
            .count()
    };
    assert_eq!([asks(0), asks(512), asks(1024)], [2, 2, 1]);

    let _guard2 = (drop(_guard), start()).1;
    connect_ok();
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload);
    broker.drop_chunks = usize::MAX;
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 2000);

    assert_eq!(
        result.records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Timeout
        })
    );
    // Every chunk of the first sector, each asked `MAX_TRIES` times.
    assert_eq!(
        broker.gets.iter().filter(|g| g.0 == Part::Chunk).count(),
        8 * MAX_TRIES as usize
    );
    assert_eq!(flash.erases, 0);
}

#[test]
fn no_offer_is_asked_again_later() {
    let _guard = start();
    connect_ok();
    let mut broker = Broker::new(None, Vec::new());
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 600);
    assert!(!result.ended);

    let asks = broker.gets.len();
    // Each idle step is 100 ms (its delay plus the receive timeout): 60 s.
    assert!((5..=7).contains(&asks), "{asks}");
    assert!(result.records.contains(&Record::NoOffer));
    assert_eq!(flash.erases, 0);
}

#[test]
fn a_set_attempt_marker_skips_without_arming() {
    let _guard = start();
    connect_ok();
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = ATTEMPT_BIT);
    let mut broker = Broker::new(None, Vec::new());
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 50);
    assert!(result.ended);

    assert_eq!(
        result.records.last(),
        Some(&Record::Skip {
            marker: ATTEMPT_BIT
        })
    );
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert!(fake::with(|h| h.tcp_tx.is_empty()));
}

#[test]
fn without_a_key_or_a_watchdog_nothing_runs() {
    let _guard = start();
    let mut broker = Broker::new(None, Vec::new());
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, None);
    let result = run(&mut updater, &mut broker, &mut flash, 5);
    assert_eq!(result.records, vec![Record::Start { key: false }]);

    WATCHDOG.with(|w| w.set(false));
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));
    let result = run(&mut updater, &mut broker, &mut flash, 50);

    assert_eq!(
        result.records.last(),
        Some(&Record::Armed {
            watchdog: false,
            installed: false
        })
    );
    assert!(!FakeHooks::armed());
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
}

#[test]
fn running_from_bank_1_refuses_every_bank_1_write() {
    let _guard = start();
    connect_ok();
    fake::with(|h| h.running_bank = Some(1));
    let signing = SigningKey::from_bytes(&SEED);
    let payload = image(LEN);
    let mut broker = Broker::new(Some(manifest(&signing, 3, &payload, LEN)), payload);
    let mut flash = Flash::default();
    let mut updater = Updater::new(FAKE_TRANSPORT, Some(key()));

    let result = run(&mut updater, &mut broker, &mut flash, 500);

    assert_eq!(
        result.records.last(),
        Some(&Record::Error {
            stage: ErrorStage::Write
        })
    );
    assert!(fake::with(|h| h.bank1.iter().all(|&b| b == 0xff)));
}

#[test]
fn keys_parse_only_as_64_lowercase_hex_digits() {
    let text = "00".repeat(31) + "a5";
    assert_eq!(parse_key(&text).unwrap()[31], 0xa5);
    assert!(parse_key(&text.to_uppercase()).is_none() || text == text.to_uppercase());
    assert!(parse_key(&("A5".to_string() + &"00".repeat(31))).is_none());
    assert!(parse_key(&text[..62]).is_none());
    assert!(parse_key(&(text.clone() + "00")).is_none());
}

#[test]
fn records_describe_without_credentials() {
    let mut line = String::new();
    describe(
        Record::Progress {
            sectors: 16,
            total: 145,
            leases: 272,
        },
        &mut line,
    )
    .unwrap();
    assert_eq!(line, "progress sectors=16/145 leases=272");

    let mut line = String::new();
    describe(Record::Start { key: true }, &mut line).unwrap();
    assert_eq!(line, "start role=recovery key=1");
}
