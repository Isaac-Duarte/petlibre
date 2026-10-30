//! The install's bridging, with no Flutter: the run on its own thread, every
//! wait on Dart answered through the same asks the screens answer, and the
//! simulated feeder on the far end of the "radio". What it proves is the
//! plumbing: the trait adapters, the thread and channels, the store, and that
//! a stop ends a run through its cleanup. The steps' own decisions are
//! `wellfed-client`'s and `wellfed-install`'s tests.

use std::future::Future;
use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use wellfed_app_bridge::api::install::PendingInstall;
use wellfed_app_bridge::api::install::{
    DnsResult, ImageCheck, ImageProvisioning, InstallAnswer, InstallAsk, InstallEnd, InstallEvent,
    InstallSpec, InstallStart, InstallUpdate, ListenUpdate, Resume, StopCase,
};
use wellfed_app_bridge::api::model::SetupRefusal;
use wellfed_app_bridge::api::setup::WifiCredentials;
use wellfed_app_bridge::install::{Listener, Pin, Run, check_dns, check_image, pending};
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_formats::canonical::sha256;
use wellfed_formats::manifest::{BuildManifest, Provisioning};
use wellfed_formats::review::{Details, ImageKind, ImageReview, ReviewKind};
use wellfed_formats::stock::Enrollment;
use wellfed_session::noise::Keypair;

const WAIT: Duration = Duration::from_secs(20);

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

/// A review folder as the APK unpacks it. Its bytes are made up, and pass
/// the same checks the image server makes before serving.
fn image(dir: &Path, provisioning: Provisioning, version: u32) -> Pin {
    let mut container = vec![0u8; 32];
    container.extend_from_slice(&wellfed_core::ota::MARKER);
    container.extend_from_slice(&[0x5a; 1024]);
    std::fs::write(dir.join("image.bin"), &container).unwrap();

    let manifest = BuildManifest {
        startup_identity: Some("ab".repeat(32)),
        provisioning,
        ..BuildManifest::default()
    };
    let manifest = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(dir.join("build-manifest.json"), &manifest).unwrap();

    let review = ImageReview {
        schema_version: 1,
        kind: ReviewKind::Plaf103DevelopmentImage,
        image_kind: ImageKind::Build,
        ota_sha256: sha256(&container),
        payload_sha256: String::new(),
        payload_bytes: 0,
        manifest_sha256: Some(sha256(&manifest)),
        review: Details::Stock {
            stock_sha256: String::new(),
        },
        reviewer_sha256: String::new(),
        application_boot_verified: false,
    };
    let review = serde_json::to_vec(&review).unwrap();
    std::fs::write(dir.join("review.json"), &review).unwrap();

    let body = wellfed_core::ota::offer::manifest_body(&wellfed_core::ota::offer::Signed {
        manifest: wellfed_core::ota::Manifest {
            version,
            image_len: 1,
            target_bank: 1,
            image_sha256: [0; 32],
            build_id: [0xab; 32],
        },
        kind: wellfed_core::ota::offer::ImageKind::Replacement,
        key_id: [0; 8],
    });
    let mut update = body.to_vec();
    update.extend_from_slice(&[0; 64]);
    std::fs::write(dir.join("update.manifest"), &update).unwrap();

    Pin {
        review_sha256: Box::leak(sha256(&review).into_boxed_str()),
        manifest_sha256: Box::leak(sha256(&update).into_boxed_str()),
        version,
        release: false,
    }
}

struct Fixture {
    /// Kept for its folders.
    dir: tempfile::TempDir,
    pin: Pin,
}

impl Fixture {
    fn new(provisioning: Provisioning) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("image")).unwrap();
        let pin = image(&dir.path().join("image"), provisioning, 7);
        Self { dir, pin }
    }

    fn spec(&self, start: InstallStart, phone_ip: &str) -> InstallSpec {
        let path = |name: &str| self.dir.path().join(name).to_str().unwrap().to_string();
        InstallSpec {
            start,
            address: None,
            phone_ip: phone_ip.into(),
            home: Some(WifiCredentials {
                ssid: "Maple St".into(),
                passphrase: "correcthorse".into(),
            }),
            label: "phone".into(),
            feeder_name: "Kitchen".into(),
            known_feeders: Vec::new(),
            store_path: path("app.json"),
            image_dir: path("image"),
            data_dir: path("install"),
            calibrate_clock: false,
            without_stock_plans: false,
            forget_enrollment: false,
            setup_again: false,
        }
    }
}

fn started(fixture: &Fixture, spec: InstallSpec) -> (Run, mpsc::UnboundedReceiver<InstallUpdate>) {
    let run = Run::new(Some(fixture.pin));
    let (updates, heard) = mpsc::unbounded_channel();
    run.start(
        Some(spec),
        Keypair::from_secret([0x51; 32]),
        move |update| updates.send(update).is_ok(),
    );
    (run, heard)
}

async fn next(heard: &mut mpsc::UnboundedReceiver<InstallUpdate>) -> InstallUpdate {
    tokio::time::timeout(WAIT, heard.recv())
        .await
        .expect("no update in time")
        .expect("the run ended with no last update")
}

async fn next_ask(heard: &mut mpsc::UnboundedReceiver<InstallUpdate>) -> (u32, InstallAsk) {
    loop {
        match next(heard).await {
            InstallUpdate::Ask { id, ask } => return (id, ask),
            InstallUpdate::Event { .. } => {}
            InstallUpdate::Ended { end } => panic!("the run ended: {end:?}"),
        }
    }
}

async fn ended(heard: &mut mpsc::UnboundedReceiver<InstallUpdate>) -> InstallEnd {
    loop {
        if let InstallUpdate::Ended { end } = next(heard).await {
            // The stream closes after the last update.
            assert!(heard.recv().await.is_none());
            return end;
        }
    }
}

fn dns_spec(fixture: &Fixture, phone: &str) -> InstallSpec {
    let start = InstallStart::Dns {
        name: "broker.example.net".into(),
    };
    fixture.spec(start, phone)
}

/// The first thing a DNS route does is ask for the override, naming this
/// phone; a stop while it waits ends the run with nothing sent, free to
/// start over.
#[test]
fn a_stop_while_a_person_is_asked_ends_the_run() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let (run, mut heard) = started(&fixture, dns_spec(&fixture, "127.41.8.1"));

        let (_, ask) = next_ask(&mut heard).await;
        assert_eq!(
            ask,
            InstallAsk::SetOverride {
                name: "broker.example.net".into(),
                broker: "127.41.8.1".into(),
            }
        );

        run.stop();
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("a stopped run did not stop");
        };
        assert_eq!(case, StopCase::Cancelled);
        assert_eq!(resume, Resume::FromStart);
    });
}

/// A platform's failure comes back as the run's end, in its own words, and
/// an answer to an ask nobody waits for is dropped.
#[test]
fn an_ask_that_fails_ends_the_run_in_its_words() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let (run, mut heard) = started(&fixture, dns_spec(&fixture, "127.41.8.2"));

        let (id, _) = next_ask(&mut heard).await;
        run.answer(
            id,
            InstallAnswer::Failed {
                reason: "the screen went away".into(),
            },
        );
        let end = ended(&mut heard).await;
        let InstallEnd::Stopped { detail, .. } = end else {
            panic!("{end:?}");
        };
        assert_eq!(detail, "the screen went away");
        run.answer(id, InstallAnswer::Done);
    });
}

/// A stream with nobody on it fails the ask at once, so the run does not
/// wait forever for a person who is gone.
#[test]
fn a_run_with_nobody_listening_ends() {
    let fixture = Fixture::new(Provisioning::Consumer);
    let run = Run::new(Some(fixture.pin));
    let (done, finished) = std::sync::mpsc::channel();
    run.start(
        Some(dns_spec(&fixture, "127.41.8.3")),
        Keypair::from_secret([0x51; 32]),
        move |update| {
            if let InstallUpdate::Ended { end } = update {
                let _ = done.send(end);
            }
            false
        },
    );
    let end = finished.recv_timeout(WAIT).unwrap();
    assert!(matches!(end, InstallEnd::Stopped { .. }), "{end:?}");
}

/// A run with nothing to run says so, rather than running twice.
#[test]
fn a_run_starts_once() {
    let run = Run::new(None);
    let (done, finished) = std::sync::mpsc::channel();
    run.start(None, Keypair::from_secret([0x51; 32]), move |update| {
        if let InstallUpdate::Ended { end } = update {
            let _ = done.send(end);
        }
        true
    });
    let end = finished.recv_timeout(WAIT).unwrap();
    assert!(matches!(
        end,
        InstallEnd::Stopped {
            case: StopCase::Other,
            ..
        }
    ));
}

/// An image the review cannot hold together stops before the feeder is
/// touched: no ask ever goes out.
#[test]
fn a_run_with_no_image_stops_before_anything_starts() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        std::fs::remove_file(fixture.dir.path().join("image/image.bin")).unwrap();
        let (_run, mut heard) = started(&fixture, dns_spec(&fixture, "127.41.8.4"));
        let end = ended(&mut heard).await;
        assert!(
            matches!(
                end,
                InstallEnd::Stopped {
                    resume: Resume::Nothing,
                    ..
                }
            ),
            "{end:?}"
        );
    });
}

/// Pick up from bank 0 with the "radio" a TCP stream to the simulated
/// feeder's recovery window, whose frames are the characteristic's: the
/// link's bytes and the code go through the asks. The recovery window takes
/// no Wi-Fi, so the feeder's own refusal comes back, typed, after the code
/// was proved, and the link is hung up.
#[test]
fn a_run_speaks_to_the_feeder_over_the_platforms_radio() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let sim = Running::start(&Flash::memory(), None).await;
        sim.type_line("hold lock").unwrap();
        sim.type_line("hold lock+dispense").unwrap();

        let (run, mut heard) = started(&fixture, fixture.spec(InstallStart::Bank0, "127.41.8.5"));

        let mut stream: Option<tokio::net::TcpStream> = None;
        let mut hello = vec![1, 0, 2];
        let mut buffer = [0u8; 1024];
        let mut closed = false;
        let mut coded = false;
        let end = loop {
            let update = tokio::select! {
                update = next(&mut heard) => update,
                n = async {
                    match &mut stream {
                        Some(stream) => stream.read(&mut buffer).await.unwrap(),
                        None => std::future::pending::<usize>().await,
                    }
                } => {
                    if n == 0 {
                        // The feeder hangs up once it answered.
                        stream = None;
                        run.link_lost();
                    }
                    // Split each read, as notifications split frames.
                    for piece in buffer[..n].chunks(7) {
                        run.notified(piece.to_vec());
                    }
                    continue;
                }
            };
            match update {
                InstallUpdate::Ask { id, ask } => {
                    match ask {
                        InstallAsk::OpenLink => {
                            stream =
                                Some(tokio::net::TcpStream::connect(sim.address).await.unwrap());
                        }
                        InstallAsk::WriteLink { pieces } => {
                            assert!(pieces.iter().all(|piece| piece.len() <= 20));
                            hello.extend(pieces.concat());
                            let bytes = std::mem::take(&mut hello);
                            stream.as_mut().unwrap().write_all(&bytes).await.unwrap();
                        }
                        InstallAsk::CloseLink => {
                            closed = true;
                            stream = None;
                        }
                        other => panic!("{other:?}"),
                    }
                    run.answer(id, InstallAnswer::Done);
                }
                InstallUpdate::Event {
                    event: InstallEvent::Code { code, seconds },
                } => {
                    assert_eq!(seconds, 60);
                    coded = true;
                    let presses: Vec<String> = code.iter().map(u8::to_string).collect();
                    sim.type_line(&format!("press {}", presses.join(" ")))
                        .unwrap();
                }
                InstallUpdate::Event { .. } => {}
                InstallUpdate::Ended { end } => break end,
            }
        };

        assert!(coded);
        assert!(closed);
        let InstallEnd::Stopped { case, resume, .. } = end else {
            panic!("{end:?}");
        };
        assert_eq!(
            case,
            StopCase::SetupFailed {
                refusal: Some(SetupRefusal::Invalid)
            }
        );
        assert_eq!(resume, Resume::FromBank0);
        sim.stop().await;
    });
}

/// A resumed run with this install's record finds its feeder through the
/// phone's own NSD and opens a session with the phone's key. A stop then
/// ends it through its cleanup, with nothing left to start over from.
#[test]
fn a_resumed_run_finds_its_feeder_over_the_platforms_discovery() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let keys = Keypair::from_secret([0x51; 32]);
        let sim = Running::start(&Flash::memory(), Some(*keys.public())).await;

        let spec = fixture.spec(InstallStart::Bank0, "127.41.8.6");
        let data = Path::new(&spec.data_dir).to_path_buf();
        std::fs::create_dir_all(data.join("backups")).unwrap();
        let record = Enrollment {
            client_id: "AF_TEST".into(),
            username: "stock".into(),
            password: "its-password".into(),
            address: "127.41.8.7".parse().unwrap(),
            enrolled_at: chrono::DateTime::from_timestamp(1, 0).unwrap(),
        };
        std::fs::write(
            Enrollment::path(&data.join("installer.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let paired = serde_json::json!({
            "device": "AF_TEST",
            "identity": wellfed_formats::hex::encode(&sim.identity),
            "address": "127.0.0.1:9",
        });
        std::fs::write(
            data.join("backups/installing.json"),
            serde_json::to_vec(&paired).unwrap(),
        )
        .unwrap();
        assert_eq!(pending(&data), PendingInstall::Paired);

        let (run, mut heard) = started(&fixture, spec);
        let mut located = false;
        let mut paired = None;
        let end = loop {
            match next(&mut heard).await {
                InstallUpdate::Ask { id, ask } => {
                    let InstallAsk::Locate { identity } = ask else {
                        panic!("{ask:?}");
                    };
                    assert_eq!(identity, wellfed_formats::hex::encode(&sim.identity));
                    located = true;
                    run.answer(
                        id,
                        InstallAnswer::Located {
                            address: Some(sim.address.to_string()),
                        },
                    );
                }
                InstallUpdate::Event {
                    event:
                        InstallEvent::Paired {
                            identity, address, ..
                        },
                } => paired = Some((identity, address)),
                InstallUpdate::Event {
                    event: InstallEvent::ReplacementAnswers { .. },
                } => run.stop(),
                InstallUpdate::Event { .. } => {}
                InstallUpdate::Ended { end } => break end,
            }
        };

        assert!(located);
        assert_eq!(
            paired,
            Some((sim.identity.to_vec(), Some(sim.address.to_string())))
        );
        assert!(matches!(end, InstallEnd::Stopped { .. }), "{end:?}");
        sim.stop().await;
    });
}

#[test]
fn the_image_is_checked_against_its_pin_and_its_own_review() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(check_image(dir.path(), None), ImageCheck::Absent);

    let pin = image(dir.path(), Provisioning::Consumer, 7);
    assert_eq!(check_image(dir.path(), None), ImageCheck::NotPinned);
    assert_eq!(
        check_image(dir.path(), Some(&pin)),
        ImageCheck::Ready {
            provisioning: ImageProvisioning::Consumer,
            build: "ab".repeat(32),
            version: 7,
        }
    );

    // Another version pinned, another review, another manifest.
    let mut other = pin;
    other.version = 8;
    assert!(matches!(
        check_image(dir.path(), Some(&other)),
        ImageCheck::Mismatch { .. }
    ));
    let mut other = pin;
    other.review_sha256 = Box::leak("00".repeat(32).into_boxed_str());
    assert!(matches!(
        check_image(dir.path(), Some(&other)),
        ImageCheck::Mismatch { .. }
    ));
    let mut other = pin;
    other.manifest_sha256 = Box::leak("00".repeat(32).into_boxed_str());
    assert!(matches!(
        check_image(dir.path(), Some(&other)),
        ImageCheck::Mismatch { .. }
    ));

    // The pinned review no longer holds the image it binds.
    std::fs::write(dir.path().join("image.bin"), [0; 64]).unwrap();
    assert!(matches!(
        check_image(dir.path(), Some(&pin)),
        ImageCheck::Mismatch { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn a_name_that_cannot_be_a_brokers_is_refused_before_any_lookup() {
    assert_eq!(
        check_dns("1.2.3.4", "192.168.1.9").await,
        DnsResult::BadName
    );
    assert_eq!(check_dns("", "192.168.1.9").await, DnsResult::BadName);
    assert_eq!(
        check_dns("broker.example.net", "not an address").await,
        DnsResult::Unresolved
    );
}

#[test]
fn a_listener_with_no_enrolled_feeder_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("installer.json"), "{}").unwrap();

    let listener = Listener::new();
    let (done, finished) = std::sync::mpsc::channel();
    listener.start(
        dir.path().to_str().unwrap(),
        Duration::ZERO,
        move |update| done.send(update).is_ok(),
    );
    let update = finished.recv_timeout(WAIT).unwrap();
    assert!(matches!(update, ListenUpdate::Failed { .. }));
}

#[test]
fn what_an_earlier_run_left_is_read_from_its_records() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(pending(dir.path()), PendingInstall::Nothing);

    // Enrolled, with a request that never settled and no pairing.
    let (id, at) = (uuid::Uuid::from_u128(7), chrono::DateTime::UNIX_EPOCH);
    wellfed_install::stock::installer_config(
        &dir.path().join("installer.json"),
        "127.41.8.20:1883".parse().unwrap(),
        None,
    )
    .unwrap();
    enroll(dir.path(), "AF_A", "127.41.8.21");
    assert_eq!(pending(dir.path()), PendingInstall::Nothing);
    let record = serde_json::json!({
        "device_id": "AF_A",
        "ota_trials": {
            id.to_string(): {
                "sequence": 1,
                "input": {"request_id": id, "server_token": id},
                "created_at": at, "updated_at": at,
                "phase": "uncertain", "download_reported": false,
                "resolution_note": null, "image_sha256": "ab".repeat(32),
            }
        },
    });
    let ledger = wellfed_install::stock::ledger_path(&dir.path().join("installer.json"), "AF_A");
    std::fs::write(ledger.unwrap(), serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(pending(dir.path()), PendingInstall::RequestOpen);

    // Paired since, with no address yet: that says more.
    std::fs::create_dir_all(dir.path().join("backups")).unwrap();
    std::fs::write(
        dir.path().join("backups/installing.json"),
        serde_json::to_vec(&serde_json::json!({
            "device": "AF_A",
            "identity": "ab".repeat(32),
            "address": null,
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(pending(dir.path()), PendingInstall::Paired);
}

/// The enrollment record beside an installer's config in `data`.
fn enroll(data: &Path, device: &str, address: &str) {
    let record = Enrollment {
        client_id: device.into(),
        username: "stock".into(),
        password: "its-password".into(),
        address: address.parse().unwrap(),
        enrolled_at: chrono::DateTime::from_timestamp(1, 0).unwrap(),
    };
    std::fs::write(
        Enrollment::path(&data.join("installer.json")),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

/// A bench image is reached where the listener heard it: the run opens its
/// recovery window there, with no lease left to say where stock connected
/// from, and the phone pairs.
#[test]
fn a_bench_run_goes_where_the_listener_heard_it() {
    local(async {
        let fixture = Fixture::new(Provisioning::Bench);
        let sim = Running::start(&Flash::memory(), None).await;

        let mut spec = fixture.spec(InstallStart::Bank0, "127.41.8.22");
        spec.address = Some(sim.address.to_string());
        let (run, mut heard) = started(&fixture, spec);

        let (id, ask) = next_ask(&mut heard).await;
        assert_eq!(ask, InstallAsk::OpenRecoveryWindow);
        sim.type_line("hold lock").unwrap();
        sim.type_line("hold lock+dispense").unwrap();
        run.answer(id, InstallAnswer::Done);

        let paired = loop {
            match next(&mut heard).await {
                InstallUpdate::Event {
                    event: InstallEvent::Code { code, .. },
                } => {
                    let presses: Vec<String> = code.iter().map(u8::to_string).collect();
                    sim.type_line(&format!("press {}", presses.join(" ")))
                        .unwrap();
                }
                InstallUpdate::Event {
                    event: InstallEvent::Paired { identity, .. },
                } => break identity,
                InstallUpdate::Ended { end } => panic!("{end:?}"),
                _ => {}
            }
        };
        assert_eq!(paired, sim.identity.to_vec());

        run.stop();
        assert!(matches!(
            ended(&mut heard).await,
            InstallEnd::Stopped { .. }
        ));
        sim.stop().await;
    });
}

/// Picking up a bench image with nothing said about where it is: the lease is
/// gone with the run that held it, so the enrollment's record says, and the
/// run waits there. A stop ends the wait; it never reads "no address".
#[test]
fn a_bench_run_with_no_lease_looks_where_the_feeder_enrolled_from() {
    local(async {
        let fixture = Fixture::new(Provisioning::Bench);
        let spec = fixture.spec(InstallStart::Bank0, "127.41.8.23");
        let data = Path::new(&spec.data_dir).to_path_buf();
        std::fs::create_dir_all(&data).unwrap();
        enroll(&data, "AF_A", "127.41.8.24");

        let (run, mut heard) = started(&fixture, spec);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            heard.try_recv().is_err(),
            "the run ended, or asked, before the feeder answered"
        );
        run.stop();

        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("a stopped run did not stop");
        };
        assert_eq!(case, StopCase::Cancelled);
        assert_eq!(resume, Resume::FromBank0);
    });
}

/// A stock feeder on the broker, reporting its version until aborted.
fn stock_feeder(broker: &str, device: &str) -> tokio::task::JoinHandle<()> {
    use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};

    let mut options = MqttOptions::new(device, broker, 1883);
    options.set_credentials("stock-user", "its-own-password");
    options.set_clean_session(true);
    let (client, mut events) = AsyncClient::new(options, 32);
    let (subscribe, report) = (
        format!("dl/PLAF103/{device}/device/+/sub"),
        format!("dl/PLAF103/{device}/device/event/post"),
    );
    tokio::spawn(async move {
        let mut beat = tokio::time::interval(Duration::from_millis(200));
        loop {
            tokio::select! {
                event = events.poll() => match event {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        let _ = client.subscribe(&subscribe, QoS::AtLeastOnce).await;
                    }
                    Ok(_) => {}
                    Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
                },
                _ = beat.tick() => {
                    let body = serde_json::json!({
                        "cmd": "DEVICE_START_EVENT",
                        "ts": chrono::Utc::now().timestamp_millis(),
                        "softwareVersion": "1.1.15",
                        "powerType": 1,
                    });
                    let _ = client.try_publish(&report, QoS::AtMostOnce, false, body.to_string());
                }
            }
        }
    })
}

/// A run from stock up to the feeder check, answering what comes before it,
/// with `device` on the broker. The ask's id comes back with the run.
async fn to_the_feeder_check(
    fixture: &Fixture,
    spec: InstallSpec,
    broker: &str,
    device: &str,
) -> (Run, mpsc::UnboundedReceiver<InstallUpdate>, u32) {
    let (run, mut heard) = started(fixture, spec);
    let mut feeder = None;
    loop {
        let (id, ask) = next_ask(&mut heard).await;
        match ask {
            InstallAsk::SetOverride { .. } => run.answer(id, InstallAnswer::Done),
            // The broker is listening now, and only now may the feeder come.
            InstallAsk::RestartFeeder => {
                run.answer(id, InstallAnswer::Done);
                feeder = Some(stock_feeder(broker, device));
            }
            InstallAsk::RecognizeFeeder {
                device: asked,
                address,
            } => {
                assert_eq!(asked, device);
                assert!(!address.is_empty());
                feeder.expect("the feeder was restarted first").abort();
                return (run, heard, id);
            }
            other => panic!("{other:?}"),
        }
    }
}

/// Stock's side on this thread's runtime: the broker, its trust on first use
/// and the server's session, as the phone runs them. Saying no to one feeder
/// forgets it, and the next one is served, not refused by the first one's
/// ledger. Past the check the image server wants private addresses, which a
/// loopback test has not got, so the run stops there with its own words.
#[test]
fn another_feeder_is_served_after_the_owner_said_no_to_the_first() {
    // SAFETY: nextest runs each test in a process of its own, and no other
    // thread is running yet.
    unsafe { std::env::set_var("WELLFED_ALLOW_UNREVIEWED_OTA_BUILD", "1") };
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let broker = "127.41.8.30";

        let spec = dns_spec(&fixture, broker);
        let (run, mut heard, id) =
            to_the_feeder_check(&fixture, spec, broker, "AF_FIRST_FEEDER").await;
        run.answer(id, InstallAnswer::Yes { yes: false });
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("the run went on");
        };
        assert_eq!(case, StopCase::NotYourFeeder);
        assert_eq!(resume, Resume::FromStart);

        let mut spec = dns_spec(&fixture, broker);
        spec.forget_enrollment = true;
        let (run, mut heard, id) =
            to_the_feeder_check(&fixture, spec, broker, "AF_OTHER_FEEDER").await;
        run.answer(id, InstallAnswer::Yes { yes: true });
        let end = ended(&mut heard).await;
        let InstallEnd::Stopped { case, detail, .. } = &end else {
            panic!("{end:?}");
        };
        assert_eq!(*case, StopCase::ServerFailed, "{detail}");
    });
}

/// A dead server is the server's failure: not "no feeder connected".
#[test]
fn a_taken_port_is_not_no_feeder() {
    // SAFETY: as above, and the listener below is this thread's own.
    unsafe { std::env::set_var("WELLFED_ALLOW_UNREVIEWED_OTA_BUILD", "1") };
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let broker = "127.41.8.31";
        let _taken = tokio::net::TcpListener::bind((broker, 1883)).await.unwrap();

        let (run, mut heard) = started(&fixture, dns_spec(&fixture, broker));
        loop {
            match next(&mut heard).await {
                InstallUpdate::Ask { id, .. } => run.answer(id, InstallAnswer::Done),
                InstallUpdate::Event { .. } => {}
                InstallUpdate::Ended { end } => {
                    let InstallEnd::Stopped { case, .. } = end else {
                        panic!("{end:?}");
                    };
                    assert_eq!(case, StopCase::ServerFailed);
                    break;
                }
            }
        }
    });
}

/// The screen refuses an image first; the run does not take its word for it.
#[test]
fn a_run_refuses_an_image_its_pin_does_not_name() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let mut other = fixture.pin;
        other.version = 8;
        let run = Run::new(Some(other));
        let (updates, mut heard) = mpsc::unbounded_channel();
        run.start(
            Some(dns_spec(&fixture, "127.41.8.32")),
            Keypair::from_secret([0x51; 32]),
            move |update| updates.send(update).is_ok(),
        );
        let end = ended(&mut heard).await;
        assert!(
            matches!(
                end,
                InstallEnd::Stopped {
                    case: StopCase::Other,
                    resume: Resume::Nothing,
                    ..
                }
            ),
            "{end:?}"
        );
    });
}

/// A manifest for another build is refused before anything is sent, and a
/// release pin takes no bench image.
#[test]
fn the_manifest_must_name_the_review_s_build_and_a_release_a_consumer_image() {
    let dir = tempfile::tempdir().unwrap();
    let pin = image(dir.path(), Provisioning::Bench, 7);
    let mut release = pin;
    release.release = true;
    assert!(matches!(
        check_image(dir.path(), Some(&pin)),
        ImageCheck::Ready {
            provisioning: ImageProvisioning::Bench,
            ..
        }
    ));
    assert!(matches!(
        check_image(dir.path(), Some(&release)),
        ImageCheck::Mismatch { .. }
    ));

    let manifest = BuildManifest {
        startup_identity: Some("cd".repeat(32)),
        ..BuildManifest::default()
    };
    let manifest = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(dir.path().join("build-manifest.json"), &manifest).unwrap();
    let mut review: ImageReview =
        serde_json::from_slice(&std::fs::read(dir.path().join("review.json")).unwrap()).unwrap();
    review.manifest_sha256 = Some(sha256(&manifest));
    let review = serde_json::to_vec(&review).unwrap();
    std::fs::write(dir.path().join("review.json"), &review).unwrap();
    let mut repinned = pin;
    repinned.review_sha256 = Box::leak(sha256(&review).into_boxed_str());
    assert!(matches!(
        check_image(dir.path(), Some(&repinned)),
        ImageCheck::Mismatch { .. }
    ));
}

/// What a run is given decides what stops it, in its own words, before any
/// of it starts, and leaves a way on by the same route.
#[test]
fn a_run_that_cannot_start_says_why_and_touches_nothing() {
    local(async {
        // A request with no Wi-Fi to give the replacement could not be taken
        // back, and the replacement would have nothing to join.
        let fixture = Fixture::new(Provisioning::Consumer);
        let mut spec = dns_spec(&fixture, "127.41.8.30");
        spec.home = None;
        let data = Path::new(&spec.data_dir).to_path_buf();
        let (_run, mut heard) = started(&fixture, spec);
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("a run with no Wi-Fi started");
        };
        assert_eq!(case, StopCase::NoWifiGiven);
        assert_eq!(resume, Resume::FromStart);
        assert!(!data.join("installer.json").exists());

        // A new Wi-Fi for a feeder that never joined needs one too.
        let mut spec = fixture.spec(InstallStart::Bank0, "127.41.8.30");
        spec.home = None;
        spec.setup_again = true;
        let (_run, mut heard) = started(&fixture, spec);
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("a run with no Wi-Fi started");
        };
        assert_eq!(case, StopCase::NoWifiGiven);
        assert_eq!(resume, Resume::FromBank0);

        // A phone off Wi-Fi, and a Wi-Fi no feeder could be given.
        let spec = dns_spec(&fixture, "");
        let (_run, mut heard) = started(&fixture, spec);
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("a phone with no address started");
        };
        assert_eq!(case, StopCase::PhoneOffWifi);
        assert_eq!(resume, Resume::FromStart);

        let mut spec = dns_spec(&fixture, "127.41.8.30");
        spec.home = Some(WifiCredentials {
            ssid: String::new(),
            passphrase: "correcthorse".into(),
        });
        let (_run, mut heard) = started(&fixture, spec);
        let InstallEnd::Stopped { case, resume, .. } = ended(&mut heard).await else {
            panic!("an empty Wi-Fi name started");
        };
        assert_eq!(case, StopCase::WifiRefused);
        assert_eq!(resume, Resume::FromStart);
    });
}

/// Forgetting the broker's trust is for a new request. A run from bank 0 finds
/// its own record through that trust, so it keeps it.
#[test]
fn a_pick_up_keeps_the_feeder_the_broker_trusted() {
    local(async {
        let fixture = Fixture::new(Provisioning::Bench);
        let mut spec = fixture.spec(InstallStart::Bank0, "127.41.8.31");
        spec.forget_enrollment = true;
        let data = Path::new(&spec.data_dir).to_path_buf();
        std::fs::create_dir_all(&data).unwrap();
        enroll(&data, "AF_A", "127.41.8.32");

        let (run, mut heard) = started(&fixture, spec);
        tokio::time::sleep(Duration::from_millis(300)).await;
        run.stop();
        ended(&mut heard).await;
        assert!(Enrollment::path(&data.join("installer.json")).exists());
    });
}

/// A feeder that paired and never joined: the run waits for it, unless the
/// owner gave a new Wi-Fi, when it goes through Bluetooth setup again and gives
/// that one to the same key.
#[test]
fn a_new_wifi_goes_through_setup_again_for_a_feeder_that_never_joined() {
    local(async {
        let fixture = Fixture::new(Provisioning::Consumer);
        let sim = Running::start(&Flash::memory(), None).await;

        let mut spec = fixture.spec(InstallStart::Bank0, "127.41.8.33");
        spec.setup_again = true;
        let data = Path::new(&spec.data_dir).to_path_buf();
        std::fs::create_dir_all(data.join("backups")).unwrap();
        enroll(&data, "AF_A", "127.41.8.34");
        let paired = serde_json::json!({
            "device": "AF_A",
            "identity": wellfed_formats::hex::encode(&sim.identity),
            "address": null,
        });
        std::fs::write(
            data.join("backups/installing.json"),
            serde_json::to_vec(&paired).unwrap(),
        )
        .unwrap();

        // It looks on the network first, and the feeder is not there.
        let (run, mut heard) = started(&fixture, spec);
        let mut opened = false;
        loop {
            match next(&mut heard).await {
                InstallUpdate::Ask {
                    id,
                    ask: InstallAsk::Locate { .. },
                } => run.answer(id, InstallAnswer::Located { address: None }),
                InstallUpdate::Ask {
                    id,
                    ask: InstallAsk::OpenLink,
                } => {
                    opened = true;
                    run.answer(
                        id,
                        InstallAnswer::Failed {
                            reason: "the test stops here".into(),
                        },
                    );
                    run.stop();
                }
                InstallUpdate::Ask { ask, .. } => panic!("{ask:?}"),
                InstallUpdate::Event { .. } => {}
                InstallUpdate::Ended { .. } => break,
            }
        }
        assert!(opened, "the run did not go back to Bluetooth");
        sim.stop().await;
    });
}
