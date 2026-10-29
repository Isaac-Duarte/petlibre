//! The feeder session against the simulator, the UART capture beside the
//! jobs, and the bank ledger, over the socket in a temporary checkout.

use wellfed_feeder_sim::harness::Relay;
use wellfed_feeder_sim::harness::Threaded;
use wellfed_feeder_sim::serve::Control;
use wellfed_formats::bench::banks::{Bank, Check, Content, Source, Witness};
use wellfed_formats::bench::feeder::{Asked, Stream};
use wellfed_formats::bench::live::{DEVICE_CURSOR, DeviceCursor, Pause, Session, Unavailable};
use wellfed_formats::bench::{notes, rendezvous};
use wellfed_model::{Answer, Op, Pushed};
use wellfed_rig::device::{Ask as RigAsk, DeviceLink};

use super::*;
use crate::api::{Seed, SeedError, SeededFrom};
use crate::feeder::{ClockSync, DeviceOptions};

const OWNER: [u8; 32] = [0x22; 32];
/// An update key, as `wellfed update-key enroll` sends it.
const KEY: [u8; 32] = [0x5a; 32];
const BUILT: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const OTHER: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";

/// A checkout whose stand-in flash client receives `banner` on every
/// observe (until the root's `banner` file changes) and holds the port for
/// the observe's own `--seconds`; its writes complete at once. Relay 1
/// starts on, so the server captures and opens its session.
fn checkout() -> Checkout {
    let checkout = Checkout::new(0.0);
    let root = checkout.root();
    // Arguments: --root R read observe --port P --output O --seconds S
    let script = r#"#!/bin/sh
case "$3 $4" in
"read observe")
  out="$8"
  mkdir -p "$out"
  [ -f "$2/banner" ] && cat "$2/banner" > "$out/serial-rx.bin"
  echo '{"status":"running"}' > "$out/report.json"
  sleep "${10}" & wait $!
  echo '{"status":"complete","result":{"bytes":7}}' > "$out/report.json"
  ;;
*" execute")
  [ -f "$2/stop-writes" ] && exit 1
  ;;
esac
exit 0
"#;
    fs::write(root.join("wellfed-flash"), script).unwrap();
    fs::write(root.join("private/server.json"), "{}").unwrap();
    checkout.board.lock().on[1] = true;
    checkout
}

impl Checkout {
    /// Enroll the owner key with `feeder`'s identity.
    fn enroll(&self, feeder: &Threaded) {
        let file = serde_json::json!({
            "version": 1,
            "secret": wellfed_formats::hex::encode(&OWNER),
            "label": "bench",
            "feeders": [{ "identity": wellfed_formats::hex::encode(&feeder.identity) }],
        });
        fs::write(
            self.root().join("private/wellfed-owner.json"),
            file.to_string(),
        )
        .unwrap();
    }

    fn banner(&self, text: &str) {
        fs::write(self.root().join("banner"), text).unwrap();
    }

    fn open_at(&self, feeder: Option<&Threaded>) -> Server {
        self.open_device(DeviceOptions {
            address: feeder.map(|feeder| feeder.address),
            ..DeviceOptions::default()
        })
        .unwrap()
    }

    fn open_device(&self, device: DeviceOptions) -> Result<Server, ServeError> {
        let board = self.board.clone();
        Server::open_with(self.options(), &device, move |_| {
            Ok(Box::new(AtBoard::new(Box::new(board))))
        })
    }
}

fn simulator() -> Threaded {
    Threaded::start(*wellfed_session::noise::Keypair::from_secret(OWNER).public())
}

fn open(state: &BenchState) -> bool {
    matches!(state.feeder.session, Session::Open { .. })
}

/// The events the server's ring holds, by number.
async fn events(server: &Running) -> Vec<u64> {
    let page = server
        .client
        .pushed(Stream::Events, 0, Duration::from_millis(10))
        .await
        .unwrap();
    page.messages
        .iter()
        .filter_map(|message| match message {
            Pushed::Event(event) => Some(event.seq),
            _ => None,
        })
        .collect()
}

fn cursor(root: &std::path::Path) -> Option<DeviceCursor> {
    wellfed_formats::load(&root.join(DEVICE_CURSOR)).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_session_reads_status_sets_the_clock_and_resumes_after_its_cursor() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let root = checkout.root();
    let server = Running::start(checkout.open_at(Some(&feeder)));

    // The simulator starts with no clock: the first status sets it, and the
    // next reads it back within the 2 s the server allows.
    let state = state_until(&server, "the clock set and read back", |state| {
        state.feeder.clock.synced_at.is_some()
            && state
                .feeder
                .clock
                .skew_ms
                .is_some_and(|skew| skew.abs() <= 2_000)
    })
    .await;
    let Session::Open { build, .. } = &state.feeder.session else {
        panic!("{:?}", state.feeder.session);
    };
    let status = state.feeder.status.clone().unwrap();
    assert_eq!(&status.build, build);
    assert!(state.feeder.status_at.is_some());

    // The subscription replays the history from its start: the boot.
    let first = state_until(&server, "the boot event", |state| {
        state
            .feeder
            .journal
            .is_some_and(|journal| journal.epoch.is_some())
    })
    .await
    .feeder
    .journal
    .unwrap();
    until("the cursor on disk", || cursor(&root).is_some()).await;
    assert_eq!(cursor(&root).unwrap().seq, first.seq);
    assert!(events(&server).await.contains(&first.seq));

    // A restart drops the session; the server reopens it and takes the new
    // boot, and only that, after its cursor.
    feeder.control(Control::Reboot);
    until("the restart's boot on disk", || {
        cursor(&root).is_some_and(|cursor| cursor.seq > first.seq)
    })
    .await;
    state_until(&server, "the session again", open).await;
    let kept = cursor(&root).unwrap();
    server.quit().await;

    // A new server subscribes after the cursor it finds.
    let server = Running::start(checkout.open_at(Some(&feeder)));
    state_until(&server, "a status on the new server", |state| {
        open(state) && state.feeder.status.is_some()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let replayed = events(&server).await;
    assert!(
        replayed.iter().all(|seq| *seq > kept.seq),
        "{replayed:?} after {kept:?}"
    );

    // A request through the socket is answered once.
    let asked = server.client.ask([7; 16], Op::Status).await.unwrap();
    assert!(
        matches!(
            &asked,
            Asked::Answered {
                answer: Answer::Status(_)
            }
        ),
        "{asked:?}"
    );
    let info = server.client.session().await.unwrap();
    assert_eq!(
        info.identity,
        wellfed_formats::hex::encode(&feeder.identity)
    );

    // A job's sequence asks through the same session, from its own thread.
    let (keys, mut link) = server.shared.feeder().link();
    assert_eq!(keys, std::path::Path::new(wellfed_rig::device::OWNER_KEYS));
    let answer = tokio::task::spawn_blocking(move || {
        wellfed_rig::device::DeviceLink::ask(&mut link, &wellfed_rig::device::Ask::Status)
    })
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(&answer, wellfed_rig::device::Answer::Status(status)
            if status.build == info.opened.as_ref().unwrap().build.to_ascii_lowercase()),
        "{answer:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_uart_job_pauses_the_session_and_it_opens_again_after() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let server = Running::start(checkout.open_at(Some(&feeder)));
    state_until(&server, "the session", open).await;

    let id = server
        .submit(&["capture", "c1", "--seconds", "1.5"])
        .await
        .unwrap();
    let state = state_until(&server, "the pause", |state| {
        matches!(state.feeder.session, Session::Paused { .. })
    })
    .await;
    assert_eq!(
        state.feeder.session,
        Session::Paused {
            pause: Pause::UartJob {
                job: id,
                label: "capture c1".into()
            }
        }
    );
    // Nothing is sent while it is paused.
    let refused = server.client.ask([8; 16], Op::Status).await.unwrap_err();
    assert!(
        matches!(&refused, ClientError::Unavailable(unavailable)
            if matches!(unavailable.reason, Unavailable::Paused { .. })),
        "{refused:?}"
    );

    assert_eq!(follow(&server.client, id).await.0.code, 0);
    state_until(&server, "the session again", open).await;

    // Relay 1 off closes it too.
    let off = server.submit(&["off"]).await.unwrap();
    follow(&server.client, off).await;
    state_until(&server, "the power-off pause", |state| {
        state.feeder.session
            == Session::Paused {
                pause: Pause::PowerOff,
            }
    })
    .await;
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_yields_the_port_to_a_job_and_comes_back() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    checkout.banner(&format!(
        "noise\r\nPLFW1 start build={} bank=1 reset=0\r\nPLOTA1 activated version=3\r\n",
        BUILT.to_uppercase()
    ));
    let root = checkout.root();
    let server = Running::start(checkout.open_at(None));

    let state = state_until(&server, "the capture's records", |state| {
        state.capture.start.is_some() && state.capture.update.is_some()
    })
    .await;
    let first = state.capture.segment.clone().unwrap();
    assert_eq!(
        state.uart,
        Uart::Capturing {
            path: Some(first.clone())
        }
    );
    let start = state.capture.start.unwrap();
    assert_eq!((start.build.as_str(), start.bank), (BUILT, Some(1)));
    assert_eq!(
        state.capture.update.unwrap().line,
        "PLOTA1 activated version=3"
    );
    assert!(state.capture.lines.contains(&"noise".to_string()));

    let id = server
        .submit(&["capture", "c1", "--seconds", "1"])
        .await
        .unwrap();
    // While the job holds the port, the server's own observe is gone: it
    // was stopped before it could finish, and only the job's runs.
    let during = state_until(&server, "the job's capture alone", |state| {
        state.capture.segment.is_none() && state.activity.len() == 1
    })
    .await;
    assert_eq!(during.activity[0].label, "read observe");
    let report = fs::read_to_string(root.join(&first).join("report.json")).unwrap();
    assert!(report.contains("running"), "{report}");

    assert_eq!(follow(&server.client, id).await.0.code, 0);
    let after = state_until(&server, "the capture again", |state| {
        state
            .capture
            .segment
            .as_ref()
            .is_some_and(|segment| *segment != first)
    })
    .await;
    assert!(
        matches!(after.uart, Uart::Capturing { .. }),
        "{:?}",
        after.uart
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_results_write_the_ledger_and_the_banner_disputes_them() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    let root = checkout.root();
    checkout.banner(&format!("PLFW1 start build={OTHER} bank=1\r\n"));
    fs::create_dir_all(root.join("t")).unwrap();
    fs::write(
        root.join("t/build-manifest.json"),
        format!(r#"{{"startup_identity": "{}"}}"#, BUILT.to_uppercase()),
    )
    .unwrap();
    fs::write(root.join("private/verification/u1-plan.json"), "{}").unwrap();
    fs::write(root.join("private/verification/g1-plan.json"), "{}").unwrap();
    let server = Running::start(checkout.open_at(None));

    let id = server
        .submit(&[
            "flash-execute",
            "u1",
            "--source",
            "s",
            "--target",
            "t",
            "--expected-plan-sha256",
            "00",
            "--boot-seconds",
            "0.2",
        ])
        .await
        .unwrap();
    let (end, lines) = follow(&server.client, id).await;
    assert_eq!(end.code, 0, "{lines:?}");

    let state = state_until(&server, "the banner's dispute", |state| {
        state
            .banks
            .banks
            .get(&Bank::One)
            .is_some_and(|entry| matches!(entry.check, Check::Disputed { .. }))
    })
    .await;
    let entry = &state.banks.banks[&Bank::One];
    assert_eq!(entry.content, Content::Build { id: BUILT.into() });
    assert_eq!(
        (entry.source, entry.session.as_deref()),
        (Source::FlashExecute, Some("u1"))
    );
    assert!(
        matches!(&entry.check, Check::Disputed { by: Witness::Banner, seen, .. }
            if *seen == Content::Build { id: OTHER.into() }),
        "{entry:?}"
    );
    // Kept on disk, and read back by the next server.
    let kept: wellfed_formats::bench::banks::Ledger =
        wellfed_formats::load(&root.join(wellfed_formats::bench::banks::BANKS)).unwrap();
    assert_eq!(kept.banks[&Bank::One].content, entry.content);

    // A give-way leaves bank 1 without a valid image, which the banner of
    // a build on bank 1 then disputes too.
    let id = server
        .submit(&[
            "give-way-execute",
            "g1",
            "--fallback",
            "f",
            "--expected-plan-sha256",
            "00",
            "--boot-seconds",
            "0.2",
        ])
        .await
        .unwrap();
    assert_eq!(follow(&server.client, id).await.0.code, 0);
    let state = state_until(&server, "the give-way's entry", |state| {
        state.banks.banks.get(&Bank::One).is_some_and(|entry| {
            entry.source == Source::GiveWay && matches!(entry.check, Check::Disputed { .. })
        })
    })
    .await;
    assert_eq!(state.banks.banks[&Bank::One].content, Content::Invalid);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seed_takes_the_operator_s_word_when_the_inspection_names_no_build() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    let server = Running::start(checkout.open_at(None));

    let refused = server
        .client
        .seed(&Seed {
            session: "d1".into(),
            bank: None,
            build: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(
            &refused,
            ClientError::Refused(api::Refusal::Seed {
                error: SeedError::NoBuild { .. }
            })
        ),
        "{refused:?}"
    );

    let seeded = server
        .client
        .seed(&Seed {
            session: "d1".into(),
            bank: Some(0),
            build: Some(BUILT.to_uppercase()),
        })
        .await
        .unwrap();
    assert_eq!(
        seeded.from,
        SeededFrom::Operator {
            inspection: "private/verification/d1-inspect/report.json".into()
        }
    );
    let entry = &server.client.banks().await.unwrap().banks[&Bank::Zero];
    assert_eq!(
        (&entry.content, entry.source),
        (&Content::Build { id: BUILT.into() }, Source::Seed)
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_lost_with_its_session_goes_once_more_and_a_write_comes_back_uncertain() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let relay = Relay::start(feeder.address).await;
    let checkout = checkout();
    checkout.enroll(&feeder);
    let server = Running::start(
        checkout
            .open_device(DeviceOptions {
                address: Some(relay.address),
                clock: ClockSync::Off,
                ..DeviceOptions::default()
            })
            .unwrap(),
    );
    state_until(&server, "the session", |state| {
        open(state) && state.feeder.status.is_some()
    })
    .await;

    // Both go out; the feeder's answers are held back, then the
    // connection is cut under them.
    relay.hold_answers();
    let client = Client::at(server.client_socket());
    let write =
        tokio::spawn(async move { client.ask([1; 16], Op::SetLock { locked: true }).await });
    let client = Client::at(server.client_socket());
    let read = tokio::spawn(async move { client.ask([2; 16], Op::Status).await });
    // The same id again while the first is out is refused, not sent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let twice = server
        .client
        .ask([1; 16], Op::SetLock { locked: false })
        .await
        .unwrap_err();
    assert!(
        matches!(&twice, ClientError::Unavailable(refused)
            if matches!(refused.reason, Unavailable::DuplicateId { .. })),
        "{twice:?}"
    );
    relay.cut();
    relay.resume();

    assert_eq!(write.await.unwrap().unwrap(), Asked::Uncertain);
    let read = read.await.unwrap().unwrap();
    assert!(
        matches!(
            &read,
            Asked::Answered {
                answer: Answer::Status(_)
            }
        ),
        "{read:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_push_closes_the_session_until_it_ends_and_no_clock_sync_leaves_the_clock() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let server = Running::start(
        checkout
            .open_device(DeviceOptions {
                address: Some(feeder.address),
                clock: ClockSync::Off,
                ..DeviceOptions::default()
            })
            .unwrap(),
    );
    let state = state_until(&server, "a status", |state| {
        open(state) && state.feeder.status.is_some()
    })
    .await;
    assert_eq!(state.feeder.clock.synced_at, None);
    assert_eq!(state.feeder.clock.skew_ms, None, "the clock stays unset");

    let (_, mut link) = server.shared.feeder().link();
    let release = link.hold_for_push().unwrap();
    state_until(&server, "the install pause", |state| {
        state.feeder.session
            == Session::Paused {
                pause: Pause::Install,
            }
    })
    .await;
    let refused = tokio::task::spawn_blocking(move || link.ask(&RigAsk::Status))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(
            refused,
            wellfed_rig::device::DeviceError::Unavailable(Unavailable::Paused {
                pause: Pause::Install
            })
        ),
        "{refused:?}"
    );

    release();
    state_until(&server, "the session after the push", open).await;
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_capture_never_starts_under_a_job_however_the_starts_interleave() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    let server = Running::start(checkout.open_at(None));
    state_until(&server, "the capture", |state| {
        state.capture.segment.is_some()
    })
    .await;

    for round in 0..5 {
        let shared = Arc::clone(&server.shared);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        // The relay poll and a job's end both start the capture: race them
        // against the job's start.
        let racing = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                shared.resume_capture();
            }
        });
        let id = server
            .submit(&["capture", &format!("c{round}"), "--seconds", "0.5"])
            .await
            .unwrap();
        // Once the job's thread has stopped the capture, nothing starts it
        // again until the job ends.
        state_until(&server, "the job's own capture", |state| {
            state.activity.len() == 1
        })
        .await;
        for _ in 0..10 {
            let state = server.state().await;
            if state.job.as_ref().is_some_and(|job| job.end.is_none()) {
                assert!(
                    !server.shared.capture().is_running(),
                    "round {round}: the capture runs under job {id}"
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(follow(&server.client, id).await.0.code, 0);
        stop.store(true, Ordering::SeqCst);
        racing.join().unwrap();
    }
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_stops_leaves_its_bank_unknown_and_a_seed_waits_for_no_job() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    let root = checkout.root();
    fs::write(root.join("stop-writes"), "").unwrap();
    fs::write(root.join("private/verification/g1-plan.json"), "{}").unwrap();
    let server = Running::start(checkout.open_at(None));

    let id = server
        .submit(&[
            "give-way-execute",
            "g1",
            "--fallback",
            "f",
            "--expected-plan-sha256",
            "00",
        ])
        .await
        .unwrap();
    assert_eq!(follow(&server.client, id).await.0.code, 1);
    let entry = server.client.banks().await.unwrap().banks[&Bank::One].clone();
    assert_eq!(
        (entry.content, entry.source),
        (Content::Unknown, Source::GiveWay)
    );

    let id = server
        .submit(&["capture", "c1", "--seconds", "1"])
        .await
        .unwrap();
    let refused = server
        .client
        .seed(&Seed {
            session: "d1".into(),
            bank: Some(0),
            build: Some(BUILT.into()),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, ClientError::Refused(api::Refusal::Busy { id: busy, .. }) if *busy == id),
        "{refused:?}"
    );
    follow(&server.client, id).await;
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_points_the_user_s_clients_at_itself_and_withdraws_when_it_quits() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let pointers = tempfile::tempdir().unwrap();
    let (runtime, state) = (pointers.path().join("run"), pointers.path().join("state"));
    let checkout = checkout();
    let device = || DeviceOptions {
        rendezvous: vec![runtime.clone(), state.clone()],
        ..DeviceOptions::default()
    };
    let server = Running::start(checkout.open_device(device()).unwrap());

    // In both: a client with no XDG in its environment looks only in the
    // second.
    for directory in [&runtime, &state] {
        let pointer = rendezvous::live(directory).unwrap();
        assert_eq!(pointer.root, checkout.root());
        assert_eq!(pointer.socket, checkout.root().join(api::SOCKET));
    }
    assert!(rendezvous::live_in(std::slice::from_ref(&state)).is_some());

    // Another checkout's server is refused while this one answers.
    let other = self::checkout();
    let refused = other.open_device(device()).err().unwrap();
    assert!(
        matches!(&refused, ServeError::OtherServer { root, .. } if *root == checkout.root()),
        "{refused}"
    );

    server.quit().await;
    assert!(rendezvous::live_in(&[runtime, state]).is_none());
}

#[test]
fn the_hotspot_is_read_only_while_no_job_and_no_loader_or_write_runs() {
    let mut state = crate::tests::idle_state();
    assert!(crate::server::hotspot_due(&state));

    state.uart = Uart::Writing {
        done: 1,
        total: 2,
        counted: true,
    };
    assert!(!crate::server::hotspot_due(&state));
    state.uart = Uart::Loader;
    assert!(!crate::server::hotspot_due(&state));

    state.uart = Uart::Idle;
    state.job = Some(wellfed_formats::bench::live::JobView {
        id: 1,
        label: "ota o1".into(),
        log: String::new(),
        accepted: 0.0,
        record: None,
        upcoming: Vec::new(),
        cancel: CancelState::None,
        end: None,
    });
    assert!(!crate::server::hotspot_due(&state));
}

impl Running {
    /// The socket, for a second client of its own.
    fn client_socket(&self) -> PathBuf {
        self.root.join(api::SOCKET)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_an_epoch_change_the_history_is_the_new_epoch_s_and_a_feed_reports_its_own_events() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    // The cursor a server before a store reset kept: another epoch, and a
    // sequence far past anything the new one holds.
    let root = checkout.root();
    fs::create_dir_all(root.join("private/bench-status")).unwrap();
    fs::write(root.join(DEVICE_CURSOR), r#"{"epoch": 4242, "seq": 900}"#).unwrap();
    let server = Running::start(checkout.open_at(Some(&feeder)));
    let state = state_until(&server, "the clock set", |state| {
        open(state) && state.feeder.clock.synced_at.is_some()
    })
    .await;
    let last = state.feeder.status.unwrap().last_event;

    // The held history is the new epoch's alone, from its oldest event.
    let page = server
        .client
        .pushed(Stream::Events, 0, Duration::from_millis(100))
        .await
        .unwrap();
    let epoch = page.epoch.unwrap();
    assert_ne!(epoch, 4242);
    assert_eq!(page.floor, None);
    assert!(
        page.messages.iter().all(|message| match message {
            Pushed::Event(event) => event.epoch == epoch,
            Pushed::Gap(gap) => gap.epoch == epoch,
            _ => true,
        }),
        "{:?}",
        page.messages
    );

    // A feed asks once and its result arrives, after the status's newest
    // event, in the same epoch.
    let asked = server
        .client
        .ask([5; 16], Op::Feed { portions: 1 })
        .await
        .unwrap();
    let Asked::Answered {
        answer: Answer::Feeding { feed },
    } = asked
    else {
        panic!("{asked:?}");
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    let fed = loop {
        let page = server
            .client
            .pushed(Stream::Events, 0, Duration::from_millis(500))
            .await
            .unwrap();
        let fed = page.messages.iter().find_map(|message| match message {
            Pushed::Event(event)
                if matches!(event.body, wellfed_model::EventBody::Fed { feed: f, .. } if f == feed) =>
            {
                Some(*event)
            }
            _ => None,
        });
        if let Some(fed) = fed {
            break fed;
        }
        assert!(Instant::now() < deadline, "no Fed event for feed {feed}");
    };
    assert_eq!(fed.epoch, epoch);
    assert!(
        last.is_none_or(|last| fed.seq > last),
        "{fed:?} after {last:?}"
    );
    server.quit().await;
}

/// A timer for `label`, due in a minute, expecting `expect`.
fn timer(label: &str, expect: notes::Expect) -> notes::Body {
    let due = wellfed_formats::bench::now() + 60.0;
    notes::Body::Timer {
        label: label.into(),
        due,
        until: due + notes::WINDOW,
        expect: Some(expect),
    }
}

fn timer_state(state: &BenchState, label: &str) -> Option<notes::TimerState> {
    state
        .operator
        .timers
        .iter()
        .find(|timer| timer.label == label)
        .map(|timer| timer.state.clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notes_fold_into_the_state_and_a_uart_line_or_a_lapsed_window_settles_a_timer() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = checkout();
    checkout.banner("PLFW1 start build=00 bank=0 reset=0\r\n");
    let root = checkout.root();
    let server = Running::start(checkout.open_at(None));
    let segment = state_until(&server, "the capture", |state| {
        state.capture.start.is_some()
    })
    .await
    .capture
    .segment
    .unwrap();

    server
        .client
        .note(&notes::Body::Next {
            steps: vec!["boot".into(), "feed".into()],
        })
        .await
        .unwrap();
    let refused = server
        .client
        .note(&notes::Body::Evidence {
            card: "schedule".into(),
            verdict: notes::Verdict::Pass,
            text: String::new(),
            path: Some("src/main.rs".into()),
        })
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            ClientError::Refused(api::Refusal::Note {
                error: api::NoteError::OutsidePrivate { .. }
            })
        ),
        "{refused:?}"
    );
    server
        .client
        .note(&timer(
            "fire",
            notes::Expect::Line {
                text: "PLSCH1 fire".into(),
            },
        ))
        .await
        .unwrap();
    let now = wellfed_formats::bench::now();
    server
        .client
        .note(&notes::Body::Timer {
            label: "late".into(),
            due: now,
            until: now + 0.5,
            expect: Some(notes::Expect::Line {
                text: "never printed".into(),
            }),
        })
        .await
        .unwrap();

    // The feeder prints the record the timer waits for.
    let mut rx = fs::OpenOptions::new()
        .append(true)
        .open(root.join(&segment).join("serial-rx.bin"))
        .unwrap();
    std::io::Write::write_all(&mut rx, b"PLSCH1 fire slot=0 portions=2\r\n").unwrap();

    let state = state_until(&server, "the timers settled", |state| {
        matches!(
            timer_state(state, "fire"),
            Some(notes::TimerState::Seen { .. })
        ) && matches!(
            timer_state(state, "late"),
            Some(notes::TimerState::Missed { .. })
        )
    })
    .await;
    assert_eq!(state.operator.next, ["boot", "feed"]);
    let Some(notes::TimerState::Seen { line, .. }) = timer_state(&state, "fire") else {
        unreachable!("settled above")
    };
    assert_eq!(line, "PLSCH1 fire slot=0 portions=2");
    server.quit().await;

    // The settlements are notes too: a new server folds the same timers.
    let kept = fs::read_to_string(root.join(notes::NOTES)).unwrap();
    assert!(kept.contains(r#""kind":"seen""#), "{kept}");
    assert!(kept.contains(r#""kind":"missed""#), "{kept}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_event_settles_the_timer_that_expects_its_kind() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let server = Running::start(checkout.open_at(Some(&feeder)));
    state_until(&server, "the first boot taken", |state| {
        state
            .feeder
            .journal
            .is_some_and(|journal| journal.epoch.is_some())
    })
    .await;

    server
        .client
        .note(&timer(
            "reboot",
            notes::Expect::Event {
                kind: "boot".into(),
            },
        ))
        .await
        .unwrap();
    feeder.control(Control::Reboot);

    let state = state_until(&server, "the boot event's timer", |state| {
        matches!(
            timer_state(state, "reboot"),
            Some(notes::TimerState::Seen { .. })
        )
    })
    .await;
    assert!(matches!(
        timer_state(&state, "reboot"),
        Some(notes::TimerState::Seen { line, .. }) if line == "boot event"
    ));
    server.quit().await;
}

/// An update key's enrollment waits for a Dispense press at the feeder,
/// and the owner may take most of the feeder's 60 s window to press it:
/// the server holds the request through its session that long, past the
/// 10 s every other request gets, and answers it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_enrollment_answered_by_a_late_press_comes_back_through_the_server() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let server = Running::start(checkout.open_at(Some(&feeder)));
    state_until(&server, "the session", open).await;
    for control in wellfed_feeder_sim::console::parse("hold lock").unwrap() {
        feeder.control(control);
    }

    let enroll = Op::EnrollUpdateKey {
        key: KEY,
        label: "bench".into(),
    };
    let hands = async {
        tokio::time::sleep(wellfed_connect::ANSWER + Duration::from_secs(3)).await;
        for control in wellfed_feeder_sim::console::parse("press dispense").unwrap() {
            feeder.control(control);
        }
    };
    let (asked, ()) = tokio::join!(server.client.ask([9; 16], enroll), hands);
    assert!(
        matches!(
            asked,
            Ok(Asked::Answered {
                answer: Answer::Done { .. }
            })
        ),
        "{asked:?}"
    );

    let listed = server.client.ask([10; 16], Op::ListClients).await.unwrap();
    let Asked::Answered {
        answer: Answer::Clients(clients),
    } = &listed
    else {
        panic!("{listed:?}");
    };
    assert!(
        clients
            .iter()
            .any(|client| client.key.0 == KEY
                && client.role == wellfed_model::ClientRole::UpdateSigner),
        "{clients:?}"
    );
    server.quit().await;
}

/// `wellfed install` opens sessions of its own only once the server shows
/// its session paused for the job, so the pause holds from the job's start
/// to its end: through the restart's relay off and on, and while a
/// question waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_install_job_keeps_the_session_paused_for_its_whole_run() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let feeder = simulator();
    let checkout = checkout();
    checkout.enroll(&feeder);
    let script = r#"#!/bin/sh
echo '{"kind":"restart-feeder"}'
read a
echo '{"kind":"open-recovery-window"}'
read a
"#;
    let wellfed = checkout.root().join("wellfed");
    fs::write(&wellfed, script).unwrap();
    fs::set_permissions(&wellfed, fs::Permissions::from_mode(0o755)).unwrap();
    let server = Running::start(checkout.open_at(Some(&feeder)));
    state_until(&server, "the session", open).await;

    let words = [
        "install",
        "--review",
        "r",
        "--manifest",
        "m",
        "--stock-config",
        "c",
        "--broker-name",
        "b.example",
        "--restart-seconds",
        "0.3",
    ];
    let id = server.submit(&words).await.unwrap();
    let paused = Session::Paused {
        pause: Pause::StockInstall { job: id },
    };

    let state = state_until(&server, "the question", |state| {
        let running = state
            .job
            .as_ref()
            .is_some_and(|job| job.id == id && job.end.is_none());
        // Once the job runs, nothing opens the session beside it.
        if running {
            assert!(
                !matches!(state.feeder.session, Session::Open { .. }),
                "{:?}",
                state.feeder.session
            );
        }
        state.prompt.is_some()
    })
    .await;
    assert_eq!(state.feeder.session, paused);
    assert_eq!(state.own_sessions_job(), Some(id));
    assert!(checkout.board.log().contains(&(POWER, false)));

    server
        .client
        .answer(id, 1, wellfed_formats::bench::prompt::Answer::Done)
        .await
        .unwrap();
    assert_eq!(follow(&server.client, id).await.0.code, 0);
    state_until(&server, "the session again", open).await;
    server.quit().await;
}
