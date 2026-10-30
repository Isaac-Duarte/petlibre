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
use wellfed_app_bridge::api::error::ErrorKind;
use wellfed_app_bridge::api::feeder::{
    FeederRefusal, FeederReply, FeederRequest, NotAsked, PlanEdit, PlanView, PortionChange,
    SettingsChanges,
};
use wellfed_app_bridge::api::join::{JoinAnswer, invite_feeder, join_feeder};
use wellfed_app_bridge::api::keys::ClientKey;
use wellfed_app_bridge::api::model::{CallClip, ClientRole, FeedOutcome, StatsSection};
use wellfed_app_bridge::api::session::{FedBy, FeedRefusal, HistoryBody, SessionUpdate};
use wellfed_app_bridge::api::setup::{
    SetupDriver, SetupStep, recover, setup_control, setup_service,
};
use wellfed_app_bridge::api::store::{AppStore, KnownFeeder, load_store, save_store};
use wellfed_app_bridge::requests;
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
                        SetupStep::Failed { error } => panic!("failed: {error}"),
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
    sync_clock: bool,
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
        sync_clock,
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
        let (commands, mut heard) = start_session(sim.identity, sim.address, false);

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
        let (commands, mut heard) = start_session(sim.identity, relay.address, false);
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

/// After pairing the session gives the feeder this phone's clock, as
/// `wellfed time` does; the feeder's own event records it, and the next
/// card shows the clock set.
#[test]
fn a_session_after_pairing_syncs_the_feeders_clock() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = start_session(sim.identity, sim.address, true);

        next(&mut heard, |update| match update {
            SessionUpdate::ClockSynced => Some(()),
            SessionUpdate::ClockNotSynced => panic!("the feeder refused the clock"),
            _ => None,
        })
        .await;
        let card = next(&mut heard, |update| match update {
            SessionUpdate::Status { card } if card.clock_set => Some(card),
            _ => None,
        })
        .await;
        assert!(card.clock_set);

        commands.send(Command::Close).unwrap();
        sim.stop().await;
    });
}

/// A feeder whose window is shut closes the connection, which is the first
/// thing most owners meet: the failure stays typed, so the screen can say
/// to open the window. On the bridge it travels as a stream value.
#[test]
fn a_recovery_with_the_window_shut_fails_with_its_kind() {
    local(async {
        let sim = Running::start(&Flash::memory(), None).await;
        let ask = Ask {
            label: "phone".into(),
            wifi: None,
        };
        let keys = Keypair::from_secret([0x34; 32]);
        let session =
            Session::new(&keys, [0x46; 32], ask, Code::from_bits(0x0123), [0x47; 32]).unwrap();

        let error = tokio::time::timeout(
            WAIT,
            recover(session, &sim.address.to_string(), &Notify::new(), |_| true),
        )
        .await
        .expect("the feeder answered")
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Closed, "{error}");
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
        appearance: wellfed_app_bridge::api::store::Appearance::Dark,
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

/// A session on the local set, opened and caught up with the history.
async fn opened(
    feeder: [u8; 32],
    address: std::net::SocketAddr,
) -> (
    mpsc::UnboundedSender<Command>,
    mpsc::UnboundedReceiver<SessionUpdate>,
) {
    let (commands, mut heard) = start_session(feeder, address, false);
    next(&mut heard, |update| match update {
        SessionUpdate::History { caught_up, .. } => caught_up.then_some(()),
        _ => None,
    })
    .await;
    (commands, heard)
}

#[test]
fn requests_read_and_change_the_feeder_through_the_session() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = opened(sim.identity, sim.address).await;
        let ask = |request| requests::run(&commands, sim.identity, request);

        let FeederReply::Settings { settings } = ask(FeederRequest::GetSettings).await else {
            panic!("no settings");
        };
        let changes = SettingsChanges {
            volume: Some(30),
            ..SettingsChanges::default()
        };
        let reply = ask(FeederRequest::PutSettings {
            base: settings.generation,
            changes,
        })
        .await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");
        let reply = ask(FeederRequest::PutSettings {
            base: settings.generation,
            changes,
        })
        .await;
        assert!(
            matches!(
                reply,
                FeederReply::Refused {
                    refusal: FeederRefusal::Stale { .. }
                }
            ),
            "{reply:?}"
        );

        // What the feeder would refuse anyway goes no further than Rust.
        let reply = ask(FeederRequest::PutWifi {
            base: 0,
            ssid: "Home".into(),
            passphrase: "short".into(),
        })
        .await;
        assert_eq!(
            reply,
            FeederReply::NotAsked {
                reason: NotAsked::Credentials
            }
        );

        let FeederReply::Invite { invite } = ask(FeederRequest::CreateInvite {
            role: ClientRole::Member,
        })
        .await
        else {
            panic!("no invite");
        };
        assert_eq!(invite.role, ClientRole::Member);
        assert_eq!(
            wellfed_client::api::Invite::parse(&invite.text)
                .unwrap()
                .feeder,
            sim.identity
        );

        // The invite's reader names the feeder, and the invite works once.
        assert_eq!(invite_feeder(&invite.text), Some(sim.identity.to_vec()));
        assert_eq!(invite_feeder("not an invite"), None);
        let joining = ClientKey::generate().unwrap();
        let answer = join_feeder(
            &joining,
            invite.text.clone(),
            "Ana".into(),
            sim.address.to_string(),
        )
        .await
        .unwrap();
        assert_eq!(
            answer,
            JoinAnswer::Joined {
                role: ClientRole::Member
            }
        );
        let again = join_feeder(&joining, invite.text, "Ana".into(), sim.address.to_string()).await;
        assert!(again.is_err(), "{again:?}");

        let FeederReply::Clients { clients } = ask(FeederRequest::ListClients).await else {
            panic!("no clients");
        };
        assert!(clients.iter().any(|client| client.you));

        let FeederReply::Time { time } = ask(FeederRequest::GetTime).await else {
            panic!("no time");
        };
        let reply = ask(FeederRequest::PutTime {
            base: time.generation,
            zone: None,
            ntp: Some("pool.ntp.org".into()),
        })
        .await;
        assert_eq!(
            reply,
            FeederReply::NotAsked {
                reason: NotAsked::BadNtp
            }
        );

        let FeederReply::Calibration { calibration } = ask(FeederRequest::GetCalibration).await
        else {
            panic!("no calibration");
        };
        let reply = ask(FeederRequest::PutCalibration {
            base: calibration.generation,
            portion: PortionChange::Weighed {
                grams: "98.4".into(),
                portions: 10,
            },
            bowl: calibration.bowl,
        })
        .await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");
        let FeederReply::Calibration { calibration } = ask(FeederRequest::GetCalibration).await
        else {
            panic!("no calibration");
        };
        assert_eq!(calibration.portion_mg, Some(9_840));

        let reply = ask(FeederRequest::Stats {
            section: StatsSection::Counters,
        })
        .await;
        assert!(matches!(reply, FeederReply::Stats { .. }), "{reply:?}");

        let reply = ask(FeederRequest::TailLog { after: None }).await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");
        next(&mut heard, |update| match update {
            SessionUpdate::Log { lines, .. } => (!lines.is_empty()).then_some(()),
            _ => None,
        })
        .await;

        let reply = ask(FeederRequest::SetLock { locked: false }).await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");
        let card = next(&mut heard, |update| match update {
            SessionUpdate::Status { card } if card.locked == Some(false) => Some(card),
            _ => None,
        })
        .await;
        assert!(!card.build.is_empty());

        commands.send(Command::Close).unwrap();
        sim.stop().await;
    });
}

#[test]
fn the_plans_change_through_their_edit_and_show_on_the_card() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = opened(sim.identity, sim.address).await;
        let ask = |request| requests::run(&commands, sim.identity, request);
        // The card says no meal without a clock.
        let reply = ask(FeederRequest::SyncClock).await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");

        let FeederReply::Plans { plans } = ask(FeederRequest::GetPlans).await else {
            panic!("no plans");
        };
        let plan = PlanView {
            minute: 7 * 60 + 30,
            portions: 2,
            days: vec![wellfed_model::Weekday::Mon, wellfed_model::Weekday::Thu],
            enabled: true,
            call: 0,
        };
        let reply = ask(FeederRequest::EditPlans {
            plans: plans.clone(),
            edit: PlanEdit::Add { plan: plan.clone() },
        })
        .await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");

        let FeederReply::Plans { plans: stored } = ask(FeederRequest::GetPlans).await else {
            panic!("no plans");
        };
        assert_eq!(stored.plans[0], Some(plan));

        // Its `Plans` event moves the card without being asked.
        let next_meal = next(&mut heard, |update| match update {
            SessionUpdate::Status { card } => card.next_meal,
            _ => None,
        })
        .await;
        assert_eq!(next_meal.minute, 7 * 60 + 30);
        assert_eq!(next_meal.portions, 2);

        // An edit of the table as it was is refused rather than merged.
        let reply = ask(FeederRequest::EditPlans {
            plans,
            edit: PlanEdit::Add {
                plan: PlanView {
                    minute: 9 * 60,
                    portions: 1,
                    days: vec![],
                    enabled: true,
                    call: 0,
                },
            },
        })
        .await;
        assert!(
            matches!(
                reply,
                FeederReply::Refused {
                    refusal: FeederRefusal::Stale { .. }
                }
            ),
            "{reply:?}"
        );

        let reply = ask(FeederRequest::EditPlans {
            plans: stored,
            edit: PlanEdit::Delete { slot: 5 },
        })
        .await;
        assert_eq!(
            reply,
            FeederReply::NotAsked {
                reason: NotAsked::EmptySlot
            }
        );

        commands.send(Command::Close).unwrap();
        sim.stop().await;
    });
}

#[test]
fn the_card_and_the_history_show_a_feed_read_again_by_a_new_session() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = opened(sim.identity, sim.address).await;
        commands.send(Command::Feed { portions: 1 }).unwrap();
        let ended = next(&mut heard, |update| match update {
            SessionUpdate::FeedEnded { result } => Some(result),
            _ => None,
        })
        .await;
        assert_eq!(
            ended.by,
            FedBy::Client {
                fingerprint: ended_by(owner)
            }
        );
        commands.send(Command::Close).unwrap();

        // The next session has it from the history, as no news: no FeedEnded.
        let (commands, mut heard) = start_session(sim.identity, sim.address, false);
        let mut ends = 0;
        let entries = next(&mut heard, |update| match update {
            SessionUpdate::History { entries, caught_up } => caught_up.then_some(entries),
            SessionUpdate::FeedEnded { .. } => {
                ends += 1;
                None
            }
            _ => None,
        })
        .await;
        assert_eq!(ends, 0);
        assert!(entries.iter().any(
            |entry| matches!(&entry.body, HistoryBody::Fed { result } if result.feed == ended.feed)
        ));
        let card = next(&mut heard, |update| match update {
            SessionUpdate::Status { card } => Some(card),
            _ => None,
        })
        .await;
        assert_eq!(card.last_fed.map(|fed| fed.feed), Some(ended.feed));

        commands.send(Command::Close).unwrap();
        sim.stop().await;
    });
}

/// A client's name in events: its key's first 4 bytes.
fn ended_by(key: [u8; 32]) -> String {
    wellfed_session::hex::Lower(&key[..4]).to_string()
}

#[test]
fn a_write_whose_answer_never_comes_says_so_and_a_request_after_the_end_went_nowhere() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let relay = Relay::start(sim.address).await;
        let (commands, mut heard) = opened(sim.identity, relay.address).await;

        relay.hold_answers();
        let lock = requests::run(
            &commands,
            sim.identity,
            FeederRequest::SetLock { locked: false },
        );
        let cut = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            relay.cut();
        };
        let (reply, ()) = tokio::join!(lock, cut);
        assert_eq!(reply, FeederReply::Unanswered);

        next(&mut heard, |update| match update {
            SessionUpdate::Closed { error } => error,
            _ => None,
        })
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let reply = requests::run(&commands, sim.identity, FeederRequest::Reboot).await;
        assert_eq!(reply, FeederReply::NotSent);
        sim.stop().await;
    });
}

/// 8 kHz, 16-bit mono PCM of `samples` samples.
fn wav(samples: usize) -> Vec<u8> {
    let data = samples * 2;
    let mut file = Vec::new();
    file.extend(b"RIFF");
    file.extend(u32::try_from(36 + data).unwrap().to_le_bytes());
    file.extend(b"WAVEfmt ");
    file.extend(16u32.to_le_bytes());
    file.extend(1u16.to_le_bytes());
    file.extend(1u16.to_le_bytes());
    file.extend(8_000u32.to_le_bytes());
    file.extend(16_000u32.to_le_bytes());
    file.extend(2u16.to_le_bytes());
    file.extend(16u16.to_le_bytes());
    file.extend(b"data");
    file.extend(u32::try_from(data).unwrap().to_le_bytes());
    for sample in 0..samples {
        let tone = if sample % 16 < 8 { 8_000i16 } else { -8_000 };
        file.extend(tone.to_le_bytes());
    }
    file
}

#[test]
fn a_recording_uploads_as_the_meal_call_and_a_trace_refused_says_so() {
    local(async {
        let owner = *Keypair::from_secret(OWNER).public();
        let sim = Running::start(&Flash::memory(), Some(owner)).await;
        let (commands, mut heard) = opened(sim.identity, sim.address).await;
        let ask = |request| requests::run(&commands, sim.identity, request);

        let reply = ask(FeederRequest::UploadCall { wav: wav(4_000) }).await;
        let FeederReply::Call { call } = reply else {
            panic!("no call: {reply:?}");
        };
        assert!(matches!(call.clip, CallClip::Stored { .. }), "{call:?}");
        assert!(call.ms.is_some());

        let reply = ask(FeederRequest::UploadCall {
            wav: b"not a recording".to_vec(),
        })
        .await;
        assert_eq!(
            reply,
            FeederReply::NotAsked {
                reason: NotAsked::BadRecording
            }
        );

        let reply = ask(FeederRequest::DeleteCall).await;
        assert!(matches!(reply, FeederReply::Done { .. }), "{reply:?}");

        commands.send(Command::Feed { portions: 1 }).unwrap();
        let feed = next(&mut heard, |update| match update {
            SessionUpdate::FeedEnded { result } => Some(result.feed),
            _ => None,
        })
        .await;
        // The simulated feeder keeps no traces, so this is a refusal, by name.
        let reply = ask(FeederRequest::ReadTrace { feed }).await;
        assert_eq!(
            reply,
            FeederReply::Refused {
                refusal: FeederRefusal::NotFound
            }
        );

        commands.send(Command::Close).unwrap();
        sim.stop().await;
    });
}
