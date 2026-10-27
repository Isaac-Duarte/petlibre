//! Updates end to end over real TCP: `wellfed_client::update` through
//! `wellfed-connect` against the simulator, whose firmware half is the
//! real update stage (`wellfed_hal::ota::update`) on two simulated banks,
//! with the loader's choice and KM0's trial around them.
//!
//! Every fault before activation leaves bank 1 invalid, so the next boot
//! is bank 0's, which still answers; a build that never confirms gives way
//! to bank 0 after its trial.

use std::cell::RefCell;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use wellfed_client::update::{Image, InstallError, Installer, Next, Sent};
use wellfed_connect::Connection;
use wellfed_core::ota::install::IDLE_MS;
use wellfed_core::ota::offer::{self, ImageKind, Signed, key_id};
use wellfed_core::ota::{MARKER, Manifest, SECTOR};
use wellfed_feeder_sim::banks::{Banks, Trial};
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness;
use wellfed_feeder_sim::serve::Control;
use wellfed_model::{Answer, Op, Refusal, UpdatePhase, UpdateRefusal, UpdateStop, UpdateView};
use wellfed_proto::api::RequestId;
use wellfed_session::noise::Keypair;
use sha2::{Digest, Sha256};
use tokio::task::LocalSet;

const OWNER: [u8; 32] = [0x22; 32];
const BENCH: [u8; 32] = [0x0b; 32];

fn bench() -> SigningKey {
    SigningKey::from_bytes(&BENCH)
}

fn paused(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

/// A signed image for build `build`.
struct Offer {
    payload: Vec<u8>,
    manifest: Vec<u8>,
}

impl Offer {
    fn new(version: u32, build: u8, len: usize) -> Self {
        let mut payload: Vec<u8> = (0..len).map(|i| (i % 249) as u8 ^ build).collect();
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

/// The simulator, its banks, and the owner's request ids.
struct Rig {
    sim: harness::Running,
    banks: Rc<RefCell<Banks>>,
    ids: u64,
}

impl Rig {
    async fn start() -> Self {
        let banks = Banks::new();
        let sim = harness::Running::start_with(
            &Flash::memory(),
            &banks,
            Some(bench().verifying_key().to_bytes()),
            Some(*Keypair::from_secret(OWNER).public()),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        )
        .await;
        Self { sim, banks, ids: 0 }
    }

    fn id(&mut self) -> RequestId {
        self.ids += 1;
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.ids.to_le_bytes());
        id
    }

    /// A session, once the feeder answers again after a restart.
    async fn open(&mut self) -> Connection {
        for attempt in 0u8..200 {
            let keys = Keypair::from_secret(OWNER);
            let ephemeral = [attempt ^ 0x5c; 32];
            if let Ok(connection) =
                Connection::open(&keys, self.sim.identity, self.sim.address, ephemeral).await
            {
                return connection;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the feeder never answered");
    }

    async fn state(&mut self, connection: &mut Connection) -> UpdateView {
        let answer = connection.ask(self.id(), &Op::UpdateState).await.unwrap();
        let Answer::Update(view) = answer else {
            panic!("{answer:?}");
        };
        view
    }

    /// The bank the feeder answers from now.
    async fn bank(&mut self) -> u8 {
        let mut connection = self.open().await;
        self.state(&mut connection).await.bank
    }
}

/// What a test does to the transfer.
#[derive(Default)]
struct Faults {
    /// Drop the connection once this chunk is answered.
    drop_after: Option<u32>,
    /// Cut the power once this chunk is answered.
    cut_after: Option<u32>,
    /// Send nothing from this offset on, for longer than the install waits.
    stall_at: Option<u32>,
}

/// Run an update until it activates (and the feeder restarted into it) or
/// stops.
async fn update(rig: &mut Rig, image: Image<'_>, faults: &mut Faults) -> Result<(), InstallError> {
    let mut installer = Installer::new(image);
    let mut connection = rig.open().await;
    installer.opened(rig.state(&mut connection).await.bank);

    for _ in 0..10_000 {
        match installer.poll() {
            Next::Send(Sent::Chunk(offset), _)
                if faults.stall_at.is_some_and(|at| offset >= at) =>
            {
                faults.stall_at = None;
                installer.lost();
                tokio::time::sleep(Duration::from_millis(u64::from(IDLE_MS) + 2_000)).await;
                connection = rig.open().await;
                installer.opened(rig.state(&mut connection).await.bank);
            }
            Next::Send(sent, op) => match connection.ask(rig.id(), &op).await {
                Ok(answer) => {
                    installer.answer(sent, &answer)?;
                    let Sent::Chunk(offset) = sent else { continue };
                    if faults.drop_after.take_if(|at| *at == offset).is_some() {
                        installer.lost();
                        connection = rig.open().await;
                        installer.opened(rig.state(&mut connection).await.bank);
                    } else if faults.cut_after.take_if(|at| *at == offset).is_some() {
                        installer.lost();
                        rig.sim.control(Control::Reboot);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        connection = rig.open().await;
                        installer.opened(rig.state(&mut connection).await.bank);
                    }
                }
                Err(_) => {
                    installer.lost();
                    connection = rig.open().await;
                    installer.opened(rig.state(&mut connection).await.bank);
                }
            },
            Next::Wait => tokio::time::sleep(Duration::from_millis(5)).await,
            Next::Reconnect => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                connection = rig.open().await;
                installer.opened(rig.state(&mut connection).await.bank);
            }
            Next::Failed(error) => return Err(error),
            Next::Cancelled => unreachable!("no test cancels here"),
            Next::Activated => {
                // The feeder restarts into the new build.
                tokio::time::sleep(Duration::from_secs(3)).await;
                return Ok(());
            }
        }
    }
    panic!("no progress");
}

/// After a stop: bank 1 still invalid, bank 0 answering, nothing installing.
async fn assert_on_bank_0(rig: &mut Rig, stop: Option<UpdateStop>) {
    assert!(
        !rig.banks.borrow().bank1_valid(),
        "bank 1's marker was written"
    );
    let mut connection = rig.open().await;
    let view = rig.state(&mut connection).await;
    assert_eq!(view.bank, 0);
    if stop.is_some() {
        assert_eq!(view.stopped, stop);
    }
    assert_ne!(view.phase, UpdatePhase::Receiving);
}

#[test]
fn an_update_gives_way_installs_from_bank_0_and_confirms() {
    paused(async {
        let mut rig = Rig::start().await;
        let offer = Offer::new(7, 0x77, 3 * SECTOR + 123);
        update(&mut rig, offer.image(), &mut Faults::default())
            .await
            .unwrap();

        assert_eq!(rig.bank().await, 1);
        let mut connection = rig.open().await;
        assert_eq!(connection.build(), "77".repeat(32));
        let view = rig.state(&mut connection).await;
        assert_eq!(view.min_version, 7, "the confirmed build raised the floor");
        assert_eq!(
            &rig.banks.borrow().bank1()[..offer.payload.len()],
            &offer.payload[..]
        );
        rig.sim.stop().await;
    });
}

/// The stock backup's source: once bank 1 is left as stock leaves it after
/// installing a build into bank 0, the bank-0 role streams it whole, byte
/// for byte, the zeroed marker byte included. Bank 1 never serves its own
/// image, and after a promotion there is nothing left to read.
#[test]
fn bank_0_streams_bank_1_whole_and_bank_1_serves_none() {
    use wellfed_client::install::Bank1Reader;

    paused(async {
        let mut rig = Rig::start().await;
        let mut connection = rig.open().await;
        let answer = connection
            .ask(rig.id(), &Op::ReadBank1 { offset: 0 })
            .await
            .unwrap();
        assert_eq!(answer, Answer::Refused(Refusal::Unavailable));
        drop(connection);

        rig.banks.borrow_mut().stock_gave_way();
        rig.sim.control(Control::Reboot);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(rig.bank().await, 0);

        let mut connection = rig.open().await;
        let mut reader = Bank1Reader::new();
        let bank = loop {
            let op = reader.next_op();
            let answer = connection.ask(rig.id(), &op).await.unwrap();
            if let Some(bank) = reader.take(answer).unwrap() {
                break bank;
            }
        };
        assert_eq!(bank[0], 0, "the marker byte as stock left it");
        assert_eq!(bank, rig.banks.borrow().bank1());
        rig.sim.stop().await;
    });
}

#[test]
fn a_flipped_byte_stops_at_the_hash() {
    paused(async {
        let mut rig = Rig::start().await;
        let mut offer = Offer::new(7, 0x77, 2 * SECTOR);
        offer.payload[SECTOR + 3] ^= 0x40;
        let stopped = update(&mut rig, offer.image(), &mut Faults::default()).await;
        assert_eq!(stopped, Err(InstallError::Stopped(UpdateStop::Hash)));
        assert_on_bank_0(&mut rig, Some(UpdateStop::Hash)).await;
        rig.sim.stop().await;
    });
}

#[test]
fn a_truncated_or_stalled_transfer_times_out() {
    paused(async {
        let mut rig = Rig::start().await;
        // The manifest names more bytes than the client has.
        let mut offer = Offer::new(7, 0x77, 3 * SECTOR);
        offer.payload.truncate(SECTOR);
        let stopped = update(&mut rig, offer.image(), &mut Faults::default()).await;
        assert_eq!(stopped, Err(InstallError::Lost));
        tokio::time::sleep(Duration::from_millis(u64::from(IDLE_MS) + 2_000)).await;
        assert_on_bank_0(&mut rig, Some(UpdateStop::Timeout)).await;

        let offer = Offer::new(7, 0x77, 3 * SECTOR);
        let mut faults = Faults {
            stall_at: Some(2 * SECTOR as u32),
            ..Faults::default()
        };
        let stopped = update(&mut rig, offer.image(), &mut faults).await;
        assert_eq!(stopped, Err(InstallError::Stopped(UpdateStop::Timeout)));
        assert_on_bank_0(&mut rig, Some(UpdateStop::Timeout)).await;
        rig.sim.stop().await;
    });
}

#[test]
fn a_dropped_connection_and_a_power_cut_both_end_installed() {
    paused(async {
        let mut rig = Rig::start().await;
        let offer = Offer::new(7, 0x77, 3 * SECTOR);
        let mut faults = Faults {
            drop_after: Some(SECTOR as u32 + 512),
            ..Faults::default()
        };
        update(&mut rig, offer.image(), &mut faults).await.unwrap();
        assert_eq!(rig.bank().await, 1);

        let offer = Offer::new(8, 0x88, 3 * SECTOR);
        let mut faults = Faults {
            cut_after: Some(2 * SECTOR as u32),
            ..Faults::default()
        };
        update(&mut rig, offer.image(), &mut faults).await.unwrap();
        let mut connection = rig.open().await;
        assert_eq!(connection.build(), "88".repeat(32));
        assert_eq!(rig.state(&mut connection).await.min_version, 8);
        rig.sim.stop().await;
    });
}

#[test]
fn a_confirmed_build_that_keeps_resetting_gives_way_to_a_bank_0_that_answers() {
    paused(async {
        let mut rig = Rig::start().await;
        rig.banks.borrow_mut().trial = Trial::Storms;
        let offer = Offer::new(7, 0x77, 2 * SECTOR);
        update(&mut rig, offer.image(), &mut Faults::default())
            .await
            .unwrap();

        // It confirmed on its trial boot, then reset soon after that boot
        // and each one after it, until the resets made a storm.
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(rig.bank().await, 0);
        assert!(!rig.banks.borrow().bank1_valid());
        let mut connection = rig.open().await;
        assert_eq!(
            rig.state(&mut connection).await.min_version,
            7,
            "it had confirmed: the floor stays where it raised it"
        );

        // A fixed build installs and stays.
        rig.banks.borrow_mut().trial = Trial::Confirms;
        let next = Offer::new(8, 0x88, 2 * SECTOR);
        update(&mut rig, next.image(), &mut Faults::default())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(rig.bank().await, 1);
        rig.sim.stop().await;
    });
}

#[test]
fn a_build_that_never_confirms_gives_way_to_a_bank_0_that_answers() {
    paused(async {
        let mut rig = Rig::start().await;
        rig.banks.borrow_mut().trial = Trial::NeverConfirms;
        let offer = Offer::new(7, 0x77, 2 * SECTOR);
        update(&mut rig, offer.image(), &mut Faults::default())
            .await
            .unwrap();

        // Three trial boots, each reset at its deadline, then KM0 gives way.
        tokio::time::sleep(Duration::from_secs(20)).await;
        assert_eq!(rig.bank().await, 0);
        assert!(!rig.banks.borrow().bank1_valid());
        let mut connection = rig.open().await;
        assert_eq!(rig.state(&mut connection).await.min_version, 0);

        // The failed candidate is not installed again; another one is.
        let again = update(&mut rig, offer.image(), &mut Faults::default()).await;
        assert_eq!(
            again,
            Err(InstallError::Refused(Refusal::Update(
                UpdateRefusal::Failed
            )))
        );
        rig.banks.borrow_mut().trial = Trial::Confirms;
        let next = Offer::new(8, 0x88, 2 * SECTOR);
        update(&mut rig, next.image(), &mut Faults::default())
            .await
            .unwrap();
        assert_eq!(rig.bank().await, 1);
        rig.sim.stop().await;
    });
}
