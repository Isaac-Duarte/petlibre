//! [`Device::install`] and its faults over real TCP against the simulator,
//! whose firmware half is the real update stage on two simulated banks.
//! The bench's own faults must end where the feeder stops them, before
//! activation; a cancel must leave bank 0 free for another manifest.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use ed25519_dalek::{Signer, SigningKey};
use wellfed_client::update::{Image, InstallError};
use wellfed_core::ota::offer::{self, ImageKind, Signed, key_id};
use wellfed_core::ota::{MARKER, Manifest, SECTOR};
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_model::{UpdatePhase, UpdateStop};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use super::{Answer, Ask, Device, DeviceError, Fault};

const OWNER: [u8; 32] = [0x22; 32];
const BENCH: [u8; 32] = [0x0b; 32];

fn bench() -> SigningKey {
    SigningKey::from_bytes(&BENCH)
}

/// One simulator per process at a time: the update stage's mailbox and the
/// interlock are process-wide statics, as on the feeder, and every boot
/// resets them. Two simulators in one process (libtest's threads, or one
/// left running after its test) take each other's requests and progress.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// The simulator on a thread of its own, in real time, holding the process
/// to itself until dropped, which stops it.
struct Simulator {
    address: SocketAddr,
    identity: [u8; 32],
    quit: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    _alone: MutexGuard<'static, ()>,
}

impl Simulator {
    fn start() -> Self {
        // A test that failed while holding it stopped its simulator first.
        let alone = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);

        let (send, started) = std::sync::mpsc::channel();
        let (quit, quitting) = oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(tokio::task::LocalSet::new().run_until(async move {
                let owner = *wellfed_session::noise::Keypair::from_secret(OWNER).public();
                let sim = Running::start_with(
                    &Flash::memory(),
                    &Banks::new(),
                    Some(bench().verifying_key().to_bytes()),
                    Some(owner),
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                )
                .await;
                send.send((sim.address, sim.identity)).unwrap();
                let _ = quitting.await;
                sim.stop().await;
            }));
        });
        let (address, identity) = started.recv().unwrap();

        Self {
            address,
            identity,
            quit: Some(quit),
            thread: Some(thread),
            _alone: alone,
        }
    }

    fn device(&self) -> Device {
        Device::at(OWNER, self.identity, self.address)
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        // Stopped and joined before the lock goes, so the next test's
        // simulator is alone with the statics.
        if let Some(quit) = self.quit.take() {
            let _ = quit.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A signed image for build `build`.
struct Offer {
    payload: Vec<u8>,
    manifest: Vec<u8>,
}

impl Offer {
    fn new(version: u32, build: u8) -> Self {
        let len = 2 * SECTOR + 700;
        let mut payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8 ^ build).collect();
        payload[..8].copy_from_slice(&MARKER);
        let signed = Signed {
            manifest: Manifest {
                version,
                image_len: u32::try_from(len).unwrap(),
                target_bank: 1,
                image_sha256: Sha256::digest(&payload).into(),
                build_id: [build; 32],
            },
            kind: ImageKind::Replacement,
            key_id: key_id(&bench().verifying_key().to_bytes()),
        };
        let body = offer::manifest_body(&signed);
        let mut manifest = body.to_vec();
        manifest.extend_from_slice(&bench().sign(&body).to_bytes());
        Self { payload, manifest }
    }

    fn image(&self) -> Image<'_> {
        Image {
            manifest: &self.manifest,
            downgrade: None,
            payload: &self.payload,
        }
    }
}

fn push(device: &mut Device, offer: &Offer, fault: Fault) -> Result<(), DeviceError> {
    let stop = AtomicBool::new(false);
    let result = device.install(offer.image(), fault, &stop, &mut |note| eprintln!("{note}"));
    eprintln!("push: {result:?}");
    result
}

/// The feeder's update state once it answers from `bank` (a restart
/// follows an activation or a give-way).
fn state_on(device: &mut Device, bank: u8) -> wellfed_model::UpdateView {
    for _ in 0..60 {
        if let Ok(Answer::Update(view)) = device.ask(&Ask::UpdateState)
            && view.bank == bank
        {
            return view;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    panic!("the feeder never answered from bank {bank}");
}

fn stopped(result: Result<(), DeviceError>) -> Option<UpdateStop> {
    match result {
        Err(DeviceError::Install(InstallError::Stopped(stop))) => Some(stop),
        _ => None,
    }
}

#[test]
fn a_clean_push_and_a_dropped_one_both_activate() {
    let sim = Simulator::start();
    let mut device = sim.device();

    push(&mut device, &Offer::new(2, 0x22), Fault::None).unwrap();
    assert_eq!(state_on(&mut device, 1).min_version, 2);

    push(&mut device, &Offer::new(3, 0x33), Fault::Drop { at: 4100 }).unwrap();
    assert_eq!(state_on(&mut device, 1).min_version, 3);
}

#[test]
fn every_fault_stops_before_activation_on_bank_0() {
    let sim = Simulator::start();
    let mut device = sim.device();

    let corrupt = push(
        &mut device,
        &Offer::new(2, 0x22),
        Fault::Corrupt { at: 600 },
    );
    assert_eq!(stopped(corrupt), Some(UpdateStop::Hash));
    assert_eq!(state_on(&mut device, 0).phase, UpdatePhase::Idle);

    // Bank 0 holds the stopped candidate; a cancel frees it.
    assert!(matches!(
        device.ask(&Ask::CancelUpdate),
        Ok(Answer::Update(_))
    ));
    let unwanted = push(&mut device, &Offer::new(3, 0x33), Fault::Unwanted { at: 0 });
    assert_eq!(stopped(unwanted), Some(UpdateStop::Order));
    state_on(&mut device, 0);
}

#[test]
fn an_unwanted_chunk_after_a_sector_s_last_chunk_still_stops_in_order() {
    // The first chunk at or past 3584 is the last of sector 0: a chunk one
    // sector ahead would be the next one wanted by the time it lands.
    let sim = Simulator::start();
    let mut device = sim.device();
    let last = u32::try_from(SECTOR).unwrap() - 512;

    let unwanted = push(
        &mut device,
        &Offer::new(2, 0x22),
        Fault::Unwanted { at: last },
    );
    assert_eq!(stopped(unwanted), Some(UpdateStop::Order));
    assert_eq!(state_on(&mut device, 0).phase, UpdatePhase::Idle);
}

#[test]
fn a_cancel_after_a_give_way_lets_bank_0_take_another_manifest() {
    let sim = Simulator::start();
    let mut device = sim.device();

    push(&mut device, &Offer::new(2, 0x22), Fault::GiveWayOnly).unwrap();
    state_on(&mut device, 0);
    let swapped = push(&mut device, &Offer::new(3, 0x33), Fault::None);
    assert!(
        matches!(
            swapped,
            Err(DeviceError::Install(InstallError::Refused(
                wellfed_model::Refusal::Update(wellfed_model::UpdateRefusal::Pending)
            )))
        ),
        "{swapped:?}"
    );

    assert!(matches!(
        device.ask(&Ask::CancelUpdate),
        Ok(Answer::Update(_))
    ));
    push(&mut device, &Offer::new(3, 0x33), Fault::None).unwrap();
    assert_eq!(state_on(&mut device, 1).min_version, 3);
}

/// A feeder that answers the install's offer with a garbled message once:
/// one that authenticates but decodes as nothing, as a feeder that sealed
/// wrong bytes would send. Every later session's offer is answered as
/// activated, so a push that resumes on a new session ends `Ok`.
mod garbled {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wellfed_proto::api::{
        self as proto, ClientMessage, FRAME_MAX, FeederMessage, Hello, Op, Outcome, PROLOGUE,
        PROTOCOL_MAJOR, Welcome,
    };
    use wellfed_session::frame::{self, PREFIX};
    use wellfed_session::noise::{Handshake, Keypair, Pattern, Role, Transport};

    const FEEDER: [u8; 32] = [0x7e; 32];

    pub(super) struct Feeder {
        pub(super) address: SocketAddr,
        pub(super) identity: [u8; 32],
        pub(super) sessions: Arc<AtomicUsize>,
    }

    impl Feeder {
        pub(super) fn start() -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let sessions = Arc::new(AtomicUsize::new(0));
            let counted = Arc::clone(&sessions);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let session = counted.fetch_add(1, Ordering::SeqCst);
                    // The client hanging up ends a session; nothing to report.
                    let _ = serve(stream, session == 0);
                }
            });

            Self {
                address,
                identity: *Keypair::from_secret(FEEDER).public(),
                sessions,
            }
        }

        pub(super) fn sessions(&self) -> usize {
            self.sessions.load(Ordering::SeqCst)
        }
    }

    fn read_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
        let mut prefix = [0; PREFIX];
        stream.read_exact(&mut prefix)?;
        let len = frame::parse_prefix(prefix, FRAME_MAX).unwrap();
        let mut message = vec![0; len];
        stream.read_exact(&mut message)?;
        Ok(message)
    }

    fn write_frame(stream: &mut TcpStream, message: &[u8]) -> std::io::Result<()> {
        let prefix = frame::prefix(message.len(), FRAME_MAX).unwrap();
        stream.write_all(&prefix)?;
        stream.write_all(message)
    }

    fn seal(
        stream: &mut TcpStream,
        transport: &mut Transport,
        plain: &[u8],
    ) -> std::io::Result<()> {
        let mut out = [0; FRAME_MAX];
        let len = transport.seal(plain, &mut out).unwrap();
        write_frame(stream, &out[..len])
    }

    fn serve(mut stream: TcpStream, garble: bool) -> std::io::Result<()> {
        let open: proto::Open = wellfed_proto::decode(&read_frame(&mut stream)?).unwrap();
        assert_eq!(open, proto::Open::Session);

        let keys = Keypair::from_secret(FEEDER);
        let mut handshake = Handshake::new(
            Pattern::Ik,
            Role::Responder,
            PROLOGUE,
            &keys,
            None,
            [0x3c; 32],
        )
        .unwrap();
        let mut plain = [0; FRAME_MAX];
        let len = handshake
            .read_message(&read_frame(&mut stream)?, &mut plain)
            .unwrap();
        let hello: Hello = wellfed_proto::decode(&plain[..len]).unwrap();
        let welcome = Welcome::Open {
            major: PROTOCOL_MAJOR,
            capabilities: hello.capabilities,
            build: "garbled",
            role: proto::Role::Owner,
            epoch: 1,
        };
        let mut payload = [0; 128];
        let len = wellfed_proto::encode(&welcome, &mut payload).unwrap();
        let mut out = [0; FRAME_MAX];
        let written = handshake.write_message(&payload[..len], &mut out).unwrap();
        write_frame(&mut stream, &out[..written])?;
        let mut transport = handshake.into_transport().unwrap();

        loop {
            let message = read_frame(&mut stream)?;
            let len = transport.open(&message, &mut plain).unwrap();
            let ClientMessage::Request { id, op } = wellfed_proto::decode(&plain[..len]).unwrap()
            else {
                continue;
            };
            let result = match op {
                Op::Status => Outcome::Status(status()),
                // Authenticated, and no message of the protocol.
                Op::OfferUpdate { .. } if garble => {
                    return seal(&mut stream, &mut transport, &[0xff; 24]);
                }
                Op::OfferUpdate { .. } => Outcome::Update(activated()),
                _ => Outcome::Refused(proto::Refusal::Invalid),
            };

            let mut answer = [0; FRAME_MAX];
            let response = FeederMessage::Response { id, result };
            let len = wellfed_proto::encode(&response, &mut answer).unwrap();
            seal(&mut stream, &mut transport, &answer[..len])?;
        }
    }

    fn status() -> proto::Status<'static> {
        proto::Status {
            build: "garbled",
            bank: 0,
            uptime_ms: 0,
            boot_count: 1,
            reset_flags: 0,
            faults: 0,
            rssi: 0,
            wifi: proto::WifiSource::Build,
            link_losses: 0,
            dns_losses: 0,
            clock_ms: None,
            power: None,
            locked: None,
            settings_generation: 0,
            wifi_generation: 0,
            last_event: None,
            sessions: 1,
        }
    }

    fn activated() -> proto::UpdateView {
        proto::UpdateView {
            phase: proto::UpdatePhase::Activated,
            bank: 0,
            min_version: 1,
            digest: None,
            version: 2,
            image_len: 0,
            written: 0,
            wanted: [None; proto::WANTED_MAX],
            stopped: None,
        }
    }
}

/// A message that authenticates but does not decode ends only its
/// session: the install resumes on a new one, as after a drop.
#[test]
fn a_garbled_feeder_message_resumes_on_a_new_session() {
    let feeder = garbled::Feeder::start();
    let mut device = Device::at(OWNER, feeder.identity, feeder.address);

    push(&mut device, &Offer::new(2, 0x22), Fault::None).unwrap();
    assert_eq!(
        feeder.sessions(),
        2,
        "the garbled answer's session, then the one that resumed"
    );
}
