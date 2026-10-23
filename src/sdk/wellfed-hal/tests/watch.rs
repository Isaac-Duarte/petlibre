//! FW10 normal-role offer check against the fake SDK: a scripted broker
//! answering `ota/check`, and the fake bank model for the give-way.

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, MutexGuard};

use ed25519_dalek::{Signer, SigningKey};
use wellfed_core::ota::{self, MAGIC, MARKER, Status};
use wellfed_hal::arming::Hooks;
use wellfed_hal::flash::park::{self, Mmio};
use wellfed_hal::flash::peer::MARKER_REGISTER;
use wellfed_hal::provider::{ATTEMPT_BIT, Provider};
use wellfed_hal::sdk::fake::api::FAKE_TRANSPORT;
use wellfed_hal::sdk::fake::{self, TcpRecvScript};
use wellfed_hal::sdk::{self, WifiEvent};
use wellfed_hal::trial::identity;
use wellfed_hal::watch::*;
use wellfed_hal::{net_status, stopped};
use sha2::{Digest, Sha256};

/// The fake hardware is per thread but `net_status` is process-global, so
/// tests that join Wi-Fi run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    static REGS: RefCell<[[u32; 12]; 2]> = const { RefCell::new([[0; 12]; 2]) };
    static ARMED: Cell<bool> = const { Cell::new(false) };
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
    let core = if address & 0xffff_f000 == 0x4800_6000 {
        0
    } else {
        1
    };
    (core, (((address & 0xfff) - 0x20) / 4) as usize)
}

struct FakeHooks;

impl Hooks for FakeHooks {
    fn watchdog() -> bool {
        true
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

const SEED: [u8; 32] = [7; 32];

fn key() -> [u8; 32] {
    SigningKey::from_bytes(&SEED).verifying_key().to_bytes()
}

fn manifest(signing: &SigningKey, build: [u8; 32]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&MAGIC);
    body.extend_from_slice(&5u32.to_le_bytes());
    body.extend_from_slice(&8192u32.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&Sha256::digest([0u8; 8192]));
    body.extend_from_slice(&build);

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

/// Running bank 1, both markers valid, joined with DNS.
fn start() -> MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    fake::reset_hardware();
    net_status::reset();
    REGS.with(|r| *r.borrow_mut() = [[0; 12]; 2]);
    ARMED.with(|a| a.set(false));

    fake::with(|h| {
        h.interrupts = 1;
        h.running_bank = Some(1);
        h.bank1[..8].copy_from_slice(&MARKER);
        h.tcp_connects.extend((0..20).map(|n| Some(10 + n)));
    });
    assert!(stopped::initialize().verified());

    // Scan found, DHCP assigned, DNS ok.
    wifi(2, 0, 1);
    wifi(4, 2, 0x0a2a_6705);
    wifi(7, 0, 0x5db8_d822);
    guard
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

/// Answers CONNECT, SUBSCRIBE and `ota/check` with `offer`.
struct Broker {
    seen: usize,
    offer: Option<Vec<u8>>,
    checks: usize,
    client_ids: Vec<String>,
}

impl Broker {
    fn reply(&mut self) {
        let sent: Vec<Vec<u8>> = fake::with(|h| {
            h.tcp_tx[self.seen..]
                .iter()
                .map(|(_, p)| p.clone())
                .collect()
        });
        self.seen += sent.len();

        for packet in sent {
            match packet[0] {
                0x10 => {
                    // Client ID follows the 10-byte variable header.
                    let at = 2 + 10;
                    let n = u16::from_be_bytes([packet[at], packet[at + 1]]) as usize;
                    self.client_ids
                        .push(String::from_utf8(packet[at + 2..at + 2 + n].to_vec()).unwrap());
                    rx(&[0x20, 2, 0, 0]);
                }

                0x82 => rx(&[0x90, 3, packet[2], packet[3], 0]),

                0x30 => {
                    let n = u16::from_be_bytes([packet[2], packet[3]]) as usize;
                    let topic = std::str::from_utf8(&packet[4..4 + n]).unwrap();
                    assert_eq!(topic, format!("{}ota/check", FAKE_TRANSPORT.namespace));
                    self.checks += 1;

                    let mut answer = Vec::new();
                    match &self.offer {
                        None => answer.extend(ota::encode_data_header(
                            ota::Part::Manifest,
                            Status::NoOffer,
                            0,
                            0,
                        )),
                        Some(m) => {
                            answer.extend(ota::encode_data_header(
                                ota::Part::Manifest,
                                Status::Ok,
                                0,
                                m.len() as u32,
                            ));
                            answer.extend(m);
                        }
                    }

                    rx(&publish_packet(
                        &format!("{}ota/offered", FAKE_TRANSPORT.namespace),
                        &answer,
                    ));
                }

                0xe0 => {}
                other => panic!("unexpected packet {other:#x}"),
            }
        }
    }
}

fn run(watch: &mut Watch, broker: &mut Broker, steps: usize) -> (Vec<Record>, bool) {
    let mut provider = Provider::default();
    let mut bank = SdkGiveWay::<FakeHooks>::new();
    let mut answered = 0;
    let mut records = Vec::new();

    for _ in 0..steps {
        park::answer::<Core<true>>(0x0000_0001_4b30_abcd, &mut answered);
        let now = sdk::uptime_ms();
        // SAFETY: `provider` outlives the run.
        let step = unsafe { watch.step::<Core<false>, FakeHooks>(&mut provider, &mut bank, now) };
        records.extend(step.record);
        if step.delay_ms == 0 {
            return (records, step.restart);
        }

        broker.reply();
        fake::advance(step.delay_ms * 32, false);
        fake::with(|h| h.uptime_ms = h.uptime_ms.wrapping_add(step.delay_ms));
    }
    (records, false)
}

fn broker(offer: Option<Vec<u8>>) -> Broker {
    Broker {
        seen: 0,
        offer,
        checks: 0,
        client_ids: Vec::new(),
    }
}

#[test]
fn a_signed_offer_for_another_build_gives_way_and_asks_for_a_restart() {
    let _guard = start();
    let mut broker = broker(Some(manifest(&SigningKey::from_bytes(&SEED), [9; 32])));
    let mut watch = Watch::new(FAKE_TRANSPORT, Some(key()));

    let (records, restart) = run(&mut watch, &mut broker, 200);
    assert!(restart, "{records:?}");

    assert_eq!(records.last(), Some(&Record::GaveWay { version: 5 }));
    assert!(records.contains(&Record::Offer { version: 5 }));
    assert_eq!(fake::with(|h| h.bank1[0]), 0);
    assert_eq!(fake::with(|h| h.bank1[1..8].to_vec()), MARKER[1..].to_vec());
    assert_eq!(fake::with(|h| h.backup[MARKER_REGISTER as usize]), 0);
    assert_eq!(
        broker.client_ids,
        [format!("{}-ota", FAKE_TRANSPORT.client_id)]
    );
}

#[test]
fn no_offer_this_build_or_a_foreign_signature_never_gives_way() {
    for (offer, found) in [
        (None, Found::NoOffer),
        (
            Some(manifest(&SigningKey::from_bytes(&SEED), identity())),
            Found::Same,
        ),
        (
            Some(manifest(&SigningKey::from_bytes(&[9; 32]), [9; 32])),
            Found::Refused,
        ),
    ] {
        let _guard = start();
        let mut broker = broker(offer);
        let mut watch = Watch::new(FAKE_TRANSPORT, Some(key()));

        // Over three check periods: logged once, checked each period.
        let (records, restart) = run(&mut watch, &mut broker, 400);

        assert!(!restart);
        assert_eq!(records, [Record::Found(found)], "{found:?}");
        assert!(broker.checks >= 2, "{}", broker.checks);
        assert_eq!(fake::with(|h| h.bank1[..8].to_vec()), MARKER.to_vec());
        assert!(!FakeHooks::armed());
    }
}

#[test]
fn without_a_valid_bank_0_the_give_way_is_refused() {
    let _guard = start();
    fake::with(|h| h.bank0_marker[0] = 0);
    let mut broker = broker(Some(manifest(&SigningKey::from_bytes(&SEED), [9; 32])));
    let mut watch = Watch::new(FAKE_TRANSPORT, Some(key()));

    let (records, restart) = run(&mut watch, &mut broker, 200);

    assert!(!restart);
    assert_eq!(
        records.last(),
        Some(&Record::Error {
            stage: ErrorStage::GiveWay
        })
    );
    assert_eq!(fake::with(|h| h.bank1[..8].to_vec()), MARKER.to_vec());
}

#[test]
fn a_set_attempt_marker_stops_before_arming() {
    let _guard = start();
    fake::with(|h| h.backup[MARKER_REGISTER as usize] = ATTEMPT_BIT);
    let mut broker = broker(Some(manifest(&SigningKey::from_bytes(&SEED), [9; 32])));
    let mut watch = Watch::new(FAKE_TRANSPORT, Some(key()));

    let (records, restart) = run(&mut watch, &mut broker, 200);

    assert!(!restart);
    assert!(
        matches!(records.last(), Some(Record::Arming(_))),
        "{records:?}"
    );
    assert_eq!(fake::with(|h| h.bank1[0]), MARKER[0]);
}
