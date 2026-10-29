//! The app's Rust half against the simulated feeder, which runs the real
//! feeder's pairing, panel and feeding owner:
//! - the BLE setup driver proves its code and enrolls, carried over the
//!   recovery window's TCP framing (the same frames as the characteristic);
//! - LAN recovery reports the code window, then the feeder's identity;
//! - a session shows the card, runs a feed to its `Fed` end, reports a
//!   refused feed, and reports a feed whose answer never came as
//!   unanswered, without sending it again.

use std::future::Future;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc};
use tokio::task::LocalSet;
use wellfed_app_bridge::api::keys::ClientKey;
use wellfed_app_bridge::api::model::{ClientRole, FeedOutcome};
use wellfed_app_bridge::api::session::{FeedRefusal, SessionUpdate};
use wellfed_app_bridge::api::setup::{
    SetupDriver, SetupStep, recover, setup_control, setup_service,
};
use wellfed_app_bridge::api::store::{AppStore, KnownFeeder, load_store, save_store};
use wellfed_app_bridge::session::{self, Command};
use wellfed_client::setup::{Ask, Session, panel_position};
use wellfed_connect::Connection;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::{Relay, Running};
use wellfed_session::noise::Keypair;
use wellfed_session::setup_code::Code;

const OWNER: [u8; 32] = [0x21; 32];
const WAIT: Duration = Duration::from_secs(20);

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

/// Unlock the panel and open the recovery window, as the owner holding
/// Lock, then Lock and Dispense.
fn open_window(sim: &Running) {
    sim.type_line("hold lock").unwrap();
    sim.type_line("hold lock+dispense").unwrap();
}

fn press(sim: &Running, positions: &[u8]) {
    let presses: Vec<String> = positions.iter().map(u8::to_string).collect();
    sim.type_line(&format!("press {}", presses.join(" ")))
        .unwrap();
}

#[test]
fn the_setup_driver_proves_its_code_and_enrolls_as_owner() {
    local(async {
        let sim = Running::start(&Flash::memory(), None).await;
        open_window(&sim);
        let key = ClientKey::restore(vec![0x31; 32]).unwrap();
        let mut driver = SetupDriver::new(&key, "phone".into(), None).unwrap();
        let code = driver.code();
        assert_eq!(code.len(), 6);
        assert!(code.iter().all(|position| (1..=4).contains(position)));

        // The recovery session opens with a clear byte naming it; after
        // that the frames are the characteristic's.
        let mut stream = tokio::net::TcpStream::connect(sim.address).await.unwrap();
        let mut hello = vec![1, 0, 2];
        hello.extend(driver.start().unwrap().concat());
        stream.write_all(&hello).await.unwrap();

        let mut buffer = [0u8; 1024];
        let done = 'exchange: loop {
            let n = tokio::time::timeout(WAIT, stream.read(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_ne!(n, 0, "the feeder closed the session");
            // Split each read, as notifications split frames.
            for piece in buffer[..n].chunks(7) {
                for step in driver.notified(piece.to_vec()).unwrap() {
                    match step {
                        SetupStep::Write { pieces } => {
                            assert!(pieces.iter().all(|piece| piece.len() <= 20));
                            stream.write_all(&pieces.concat()).await.unwrap();
                        }
                        SetupStep::EnterCode { seconds } => {
                            assert_eq!(seconds, 60);
                            press(&sim, &code);
                        }
                        done @ SetupStep::Done { .. } => break 'exchange done,
                        SetupStep::Refused { refusal } => panic!("refused: {refusal:?}"),
                    }
                }
            }
        };
        let SetupStep::Done { feeder, .. } = done else {
            unreachable!()
        };
        assert_eq!(feeder, sim.identity.to_vec());

        let keys = Keypair::from_secret([0x31; 32]);
        let owner = Connection::open(&keys, sim.identity, sim.address, [0xa1; 32])
            .await
            .unwrap();
        assert_eq!(owner.role(), wellfed_model::ClientRole::Owner);
        sim.stop().await;
    });
}

#[test]
fn recovery_reports_the_code_window_then_the_feeders_identity() {
    local(async {
        let sim = Running::start(&Flash::memory(), None).await;
        open_window(&sim);
        let code = Code::from_bits(0x0bad);
        let ask = Ask {
            label: "phone".into(),
            wifi: None,
        };
        let keys = Keypair::from_secret([0x32; 32]);
        let session = Session::new(&keys, [0x42; 32], ask, code, [0x43; 32]).unwrap();
        let positions: Vec<u8> = code.presses().iter().map(|&b| panel_position(b)).collect();

        let mut steps = Vec::new();
        recover(session, &sim.address.to_string(), &Notify::new(), |step| {
            if let SetupStep::EnterCode { .. } = step {
                press(&sim, &positions);
            }
            steps.push(step);
            true
        })
        .await
        .unwrap();

        assert!(matches!(steps[0], SetupStep::EnterCode { seconds: 60 }));
        let Some(SetupStep::Done { feeder, .. }) = steps.last() else {
            panic!("no Done: {} steps", steps.len());
        };
        assert_eq!(feeder, &sim.identity.to_vec());
        sim.stop().await;
    });
}

#[test]
fn closing_a_recovery_hangs_up_while_the_code_window_waits() {
    local(async {
        let sim = Running::start(&Flash::memory(), None).await;
        open_window(&sim);
        let ask = Ask {
            label: "phone".into(),
            wifi: None,
        };
        let keys = Keypair::from_secret([0x33; 32]);
        let session =
            Session::new(&keys, [0x44; 32], ask, Code::from_bits(0x0123), [0x45; 32]).unwrap();
        let closed = Notify::new();

        // Nobody keys the code in: only the close can end this in time.
        let mut steps = 0;
        tokio::time::timeout(
            Duration::from_secs(5),
            recover(session, &sim.address.to_string(), &closed, |step| {
                if let SetupStep::EnterCode { .. } = step {
                    closed.notify_one();
                }
                steps += 1;
                true
            }),
        )
        .await
        .expect("the close ended the exchange")
        .unwrap();
        assert_eq!(steps, 1);
        sim.stop().await;
    });
}

/// A session against `address`, run on the local set; its updates and
/// the sender of its commands.
fn start_session(
    feeder: [u8; 32],
    address: std::net::SocketAddr,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<SessionUpdate>,
) {
    let (commands, receiver) = mpsc::unbounded_channel();
    let (updates, heard) = mpsc::unbounded_channel();
    tokio::task::spawn_local(session::run(
        Keypair::from_secret(OWNER),
        feeder,
        address,
        receiver,
        move |update| updates.send(update).is_ok(),
    ));
    (commands, heard)
}

/// The next update `pick` accepts, skipping status refreshes.
async fn next<T>(
    heard: &mut mpsc::UnboundedReceiver<SessionUpdate>,
    mut pick: impl FnMut(SessionUpdate) -> Option<T>,
) -> T {
    tokio::time::timeout(WAIT, async {
        loop {
            let update = heard.recv().await.expect("the session ended");
            if let Some(found) = pick(update) {
                return found;
            }
        }
    })
    .await
    .expect("no such update in time")
}

#[test]
fn a_session_shows_the_card_and_runs_a_feed_to_its_end() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = start_session(sim.identity, sim.address);

        let role = next(&mut heard, |update| match update {
            SessionUpdate::Opened { role, .. } => Some(role),
            _ => None,
        })
        .await;
        assert_eq!(role, ClientRole::Owner);
        let card = next(&mut heard, |update| match update {
            SessionUpdate::Status { card } => Some(card),
            _ => None,
        })
        .await;
        assert!(!card.build.is_empty());
        // The panel boots locked.
        assert_eq!(card.locked, Some(true));

        commands.send(Command::Feed { portions: 2 }).unwrap();
        let (feed, portions) = next(&mut heard, |update| match update {
            SessionUpdate::FeedStarted { feed, portions } => Some((feed, portions)),
            _ => None,
        })
        .await;
        assert_eq!(portions, 2);
        let result = next(&mut heard, |update| match update {
            SessionUpdate::FeedEnded { result } => Some(result),
            _ => None,
        })
        .await;
        assert_eq!(result.feed, feed);
        assert_eq!(result.outcome, FeedOutcome::Done);
        assert_eq!((result.confirmed, result.at_most), (2, 2));
        assert!(!result.uncertain);
        assert_eq!(result.words, "2 of 2 portions, done");

        commands.send(Command::Feed { portions: 0 }).unwrap();
        let refusal = next(&mut heard, |update| match update {
            SessionUpdate::FeedRefused { refusal } => Some(refusal),
            _ => None,
        })
        .await;
        assert_eq!(refusal, FeedRefusal::Portions);

        commands.send(Command::Close).unwrap();
        let error = next(&mut heard, |update| match update {
            SessionUpdate::Closed { error } => Some(error),
            _ => None,
        })
        .await;
        assert_eq!(error, None);
        sim.stop().await;
    });
}

#[test]
fn a_feed_whose_answer_never_comes_is_reported_once_and_not_resent() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let flash = Flash::memory();
        let sim = Running::start(&flash, Some(owner)).await;
        let relay = Relay::start(sim.address).await;
        let (commands, mut heard) = start_session(sim.identity, relay.address);
        next(&mut heard, |update| match update {
            SessionUpdate::Status { .. } => Some(()),
            _ => None,
        })
        .await;

        // The request reaches the feeder; its answer is lost on the way
        // back, then the connection with it.
        relay.hold_answers();
        commands.send(Command::Feed { portions: 1 }).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        relay.cut();

        let mut unanswered = 0;
        let error = next(&mut heard, |update| match update {
            SessionUpdate::FeedUnanswered => {
                unanswered += 1;
                None
            }
            SessionUpdate::FeedStarted { .. } => panic!("an answer got through"),
            SessionUpdate::Closed { error } => Some(error),
            _ => None,
        })
        .await;
        assert_eq!(unanswered, 1);
        assert!(error.is_some());

        // The feeder ran it exactly once: one Fed event in its history.
        let keys = Keypair::from_secret(OWNER);
        let mut reader = Connection::open(&keys, sim.identity, sim.address, [0xa2; 32])
            .await
            .unwrap();
        reader.subscribe(None, false).await.unwrap();
        let mut fed = 0;
        while let Ok(pushed) = reader.next(Duration::from_secs(3)).await {
            if let wellfed_model::Pushed::Event(event) = pushed
                && matches!(event.body, wellfed_model::EventBody::Fed { .. })
            {
                fed += 1;
            }
        }
        assert_eq!(fed, 1);
        sim.stop().await;
    });
}

#[test]
fn the_setup_uuids_are_the_feeders() {
    assert_eq!(setup_service(), "504c5345-5455-5000-8000-000000000000");
    assert_eq!(setup_control(), "504c5345-5455-5000-8000-000000000001");
}

#[test]
fn the_store_round_trips_and_starts_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.json");
    let path = path.to_str().unwrap();

    assert_eq!(load_store(path).unwrap(), AppStore::default());
    let store = AppStore {
        label: "Pixel 8".into(),
        feeders: vec![KnownFeeder {
            identity: "ab".repeat(32),
            name: "Kitchen".into(),
            address: Some("192.168.1.20:4747".into()),
        }],
    };
    save_store(path, &store).unwrap();
    assert_eq!(load_store(path).unwrap(), store);
}
