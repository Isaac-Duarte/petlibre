//! The feeder session against the simulator, the UART capture beside the
//! jobs, and the bank ledger, over the socket in a temporary checkout.

use wellfed_feeder_sim::harness::Threaded;
use wellfed_feeder_sim::serve::Control;
use wellfed_formats::bench::banks::{Bank, Check, Content, Source, Witness};
use wellfed_formats::bench::feeder::Asked;
use wellfed_formats::bench::live::{DEVICE_CURSOR, DeviceCursor, Pause, Session, Unavailable};
use wellfed_model::{Answer, Op, Pushed};

use super::*;
use crate::api::{Seed, SeedError, SeededFrom};
use crate::feeder::DeviceOptions;

const OWNER: [u8; 32] = [0x22; 32];
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
        let board = self.board.clone();
        let device = DeviceOptions {
            address: feeder.map(|feeder| feeder.address),
            ..DeviceOptions::default()
        };
        Server::open_with(self.options(), &device, move |_| {
            Ok(Box::new(AtBoard::new(Box::new(board))))
        })
        .unwrap()
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
        .pushed(0, Duration::from_millis(10))
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
        matches!(&answer, wellfed_rig::device::Answer::Status(status) if status.build == info.build.to_ascii_lowercase()),
        "{answer:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_uart_job_pauses_the_session_and_it_opens_again_after() {
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
