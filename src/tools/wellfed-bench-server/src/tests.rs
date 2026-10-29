//! The server end to end over a socket in a temporary checkout: a fake
//! relay board and a stand-in `wellfed-flash` script, which records the
//! signal that stops it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use wellfed_formats::bench::lease;
use wellfed_formats::bench::live::{ActivityKind, CancelState, JobId, Phase, Uart};
use wellfed_relay::{AtBoard, FakeBoard, POWER, STRAP};
use wellfed_rig::commands::UartWhenOff;

use super::*;

/// The output pipe is process-wide, so tests that run a server take turns
/// when they share a process.
static ONE_SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A checkout with a bench config and a stand-in flash client whose
/// `read observe` holds the port for `hold` seconds.
struct Checkout {
    dir: tempfile::TempDir,
    board: FakeBoard,
}

impl Checkout {
    fn new(hold: f64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("private")).unwrap();
        fs::write(
            root.join("private/bench.json"),
            r#"{"relay_port": "relay", "uart_port": "uart", "baseline": "b", "rom_report": "r"}"#,
        )
        .unwrap();

        // Arguments: --root R read observe --port P --output O --seconds S
        let script = format!(
            r#"#!/bin/sh
out="$8"
mkdir -p "$out"
echo '{{"status":"running"}}' > "$out/report.json"
trap 'echo INT > "$out/signal"; kill $! 2>/dev/null; exit 1' INT
trap 'echo TERM > "$out/signal"; kill $! 2>/dev/null; exit 1' TERM
sleep {hold} & wait $!
echo '{{"status":"complete","result":{{"bytes":7}}}}' > "$out/report.json"
"#
        );
        let flash = root.join("wellfed-flash");
        fs::write(&flash, script).unwrap();
        fs::set_permissions(&flash, fs::Permissions::from_mode(0o755)).unwrap();

        Self {
            dir,
            board: FakeBoard::default(),
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn open(&self) -> Server {
        let root = self.root();
        let options = Options {
            tools: ToolPaths {
                flash_bin: root.join("wellfed-flash"),
                stock_bin: root.join("absent"),
                build_bin: root.join("absent"),
            },
            root,
        };
        let board = self.board.clone();
        Server::open(options, move |_| {
            Ok(Box::new(AtBoard::new(Box::new(board))))
        })
        .unwrap()
    }

    fn capture(&self, name: &str) -> PathBuf {
        self.root().join("private/captures").join(name)
    }
}

/// A server running on the test's runtime, until `stop`.
struct Running {
    stop: Option<oneshot::Sender<()>>,
    done: tokio::task::JoinHandle<Result<(), ServeError>>,
    client: Client,
}

impl Running {
    fn start(server: Server) -> Self {
        let client = Client::at(server.socket().to_path_buf());
        let (stop, stopped) = oneshot::channel();
        let done = tokio::spawn(server.run(async {
            let _ = stopped.await;
        }));
        Self {
            stop: Some(stop),
            done,
            client,
        }
    }

    async fn quit(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.done.await.unwrap().unwrap();
    }
}

fn spec(command: JobCommand) -> JobSpec {
    JobSpec {
        command,
        off_seconds: 0.05,
        uart_when_off: UartWhenOff::Isolated,
    }
}

fn capture(name: &str) -> JobSpec {
    spec(JobCommand::Capture {
        name: name.into(),
        seconds: 60.0,
    })
}

fn loader(name: &str) -> JobSpec {
    spec(JobCommand::Loader { name: name.into() })
}

/// Follow `id` to its end, with the lines it printed.
async fn follow(client: &Client, id: JobId) -> (Followed, Vec<String>) {
    let (_interrupts, mut receiver) = mpsc::unbounded_channel();
    let mut lines = Vec::new();
    let followed = client
        .follow(id, |line| lines.push(line.to_string()), &mut receiver)
        .await
        .unwrap();
    (followed, lines)
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn ended(followed: Followed) -> wellfed_formats::bench::live::JobEnd {
    match followed {
        Followed::Ended(end) => end,
        Followed::Detached => panic!("detached"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_streams_its_output_and_ends_with_its_code() {
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.5);
    let server = Running::start(checkout.open());

    // Opening queried every relay.
    let state = server.client.state().await.unwrap();
    assert_eq!(state.relays.len(), 4);
    assert_eq!(state.server.phase, Phase::Serving);

    let id = server.client.submit(&capture("c1")).await.unwrap();
    let (followed, lines) = follow(&server.client, id).await;

    let end = ended(followed);
    assert_eq!((end.code, end.error), (0, None));
    assert!(
        lines
            .iter()
            .any(|line| line == "Captured 7 bytes in private/captures/c1"),
        "{lines:?}"
    );
    let state = server.client.state().await.unwrap();
    let job = state.job.unwrap();
    assert_eq!((job.id, job.label.as_str()), (id, "capture c1"));
    assert_eq!(job.end.map(|end| end.code), Some(0));
    assert!(state.activity.is_empty() && state.uart == Uart::Idle);

    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_job_is_refused_while_one_runs() {
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(3.0);
    let server = Running::start(checkout.open());
    let first = server.client.submit(&capture("c1")).await.unwrap();

    // The capture holds the UART while it runs.
    let mut state = server.client.state().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while state.activity.is_empty() {
        assert!(Instant::now() < deadline, "the capture never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
        state = server.client.state().await.unwrap();
    }
    assert_eq!(state.activity[0].kind, ActivityKind::Flash);
    assert!(
        matches!(state.uart, Uart::Capturing { .. }),
        "{:?}",
        state.uart
    );

    let refused = server.client.submit(&capture("c2")).await.unwrap_err();
    assert!(
        matches!(&refused, ClientError::Refused(api::Refusal::Busy { id, label })
            if *id == first && label == "capture c1"),
        "{refused:?}"
    );
    let bad = server.client.submit(&capture("../c3")).await.unwrap_err();
    assert!(
        matches!(&bad, ClientError::Refused(api::Refusal::BadName { .. })),
        "{bad:?}"
    );
    assert!(!checkout.capture("c2").exists());

    assert_eq!(ended(follow(&server.client, first).await.0).code, 0);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_interrupts_the_flash_client_and_the_cleanup_cuts_power() {
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(30.0);
    let server = Running::start(checkout.open());
    let id = server.client.submit(&loader("l1")).await.unwrap();

    // Strapped, powered, then the strap released: the loader now only
    // waits on its capture.
    let board = checkout.board.clone();
    until("the strap's release", || {
        let log = board.log();
        log.ends_with(&[(STRAP, false)]) && log.contains(&(POWER, true))
    })
    .await;
    server.client.cancel(id).await.unwrap();

    let (followed, lines) = follow(&server.client, id).await;
    let end = ended(followed);
    assert_eq!(end.code, 1);
    assert!(end.error.is_some());
    assert!(
        lines
            .iter()
            .any(|line| line == &format!("Cancel requested for job {id}")),
        "{lines:?}"
    );
    // The flash client got SIGINT, never a kill, and stopped on it.
    let signal = fs::read_to_string(checkout.capture("l1").join("signal")).unwrap();
    assert_eq!(signal.trim(), "INT");
    // The loader's cleanup: strap released, power cut, UART isolated.
    assert_eq!(checkout.board.state(), [false; 4]);
    let job = server.client.state().await.unwrap().job.unwrap();
    assert_eq!(job.cancel, CancelState::Requested);

    let again = server.client.cancel(id).await.unwrap_err();
    assert!(
        matches!(again, ClientError::Refused(api::Refusal::Finished { .. })),
        "{again:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_detaches_and_another_attaches_from_the_first_line() {
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(1.0);
    let server = Running::start(checkout.open());
    let id = server.client.submit(&capture("c1")).await.unwrap();

    let (interrupts, mut receiver) = mpsc::unbounded_channel();
    interrupts.send(Interrupt::Detach).unwrap();
    let detached = server
        .client
        .follow(id, |_| {}, &mut receiver)
        .await
        .unwrap();
    assert_eq!(detached, Followed::Detached);
    // The job runs on without a reader.
    assert!(
        server
            .client
            .state()
            .await
            .unwrap()
            .job
            .unwrap()
            .end
            .is_none()
    );

    let (followed, lines) = follow(&server.client, id).await;
    assert_eq!(ended(followed).code, 0);
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Captured 7 bytes"))
    );

    let missing = server.client.follow(99, |_| {}, &mut receiver).await;
    assert!(
        matches!(
            missing,
            Err(ClientError::Refused(api::Refusal::NoSuchJob { id: 99 }))
        ),
        "{missing:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quitting_cancels_the_job_waits_for_its_cleanup_then_frees_the_lease() {
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(30.0);
    let root = checkout.root();
    let server = Running::start(checkout.open());
    // The server holds the bench for its life.
    assert!(lease::inspect(&root).unwrap().is_some());
    server.client.submit(&loader("l1")).await.unwrap();
    let board = checkout.board.clone();
    until("the strap's release", || {
        board.log().ends_with(&[(STRAP, false)])
    })
    .await;

    let socket = root.join(api::SOCKET);
    assert_eq!(
        fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    server.quit().await;

    assert_eq!(checkout.board.state(), [false; 4]);
    let signal = fs::read_to_string(checkout.capture("l1").join("signal")).unwrap();
    assert_eq!(signal.trim(), "INT");
    assert!(lease::inspect(&root).unwrap().is_none());
    assert!(!socket.exists());
}

#[tokio::test]
async fn with_no_server_a_client_says_how_to_start_one() {
    let dir = tempfile::tempdir().unwrap();

    let refused = Client::new(dir.path()).state().await.unwrap_err();

    assert!(
        matches!(&refused, ClientError::NoServer { socket } if socket == &dir.path().join(api::SOCKET)),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("wellfed-bench serve"));
}

#[test]
fn a_job_spec_reads_back_from_its_json() {
    let spec = loader("unit");

    let text = serde_json::to_string(&spec).unwrap();
    let back: JobSpec = serde_json::from_str(&text).unwrap();

    assert!(text.contains(r#""loader":{"name":"unit"}"#), "{text}");
    assert_eq!(back.command.describe(), "loader unit");
    assert!(Path::new(api::SOCKET).starts_with("private"));
}
