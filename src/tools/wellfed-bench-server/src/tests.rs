//! The server end to end over a socket in a temporary checkout: a fake
//! relay board and a stand-in `wellfed-flash` script, which records the
//! signal that stops it.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use wellfed_formats::bench::lease;
use wellfed_formats::bench::live::{
    ActivityKind, BenchState, CancelState, JOBS, JobEnd, JobId, Phase, Uart,
};
use wellfed_relay::{AtBoard, Board, FakeBoard, POWER, STRAP, Switch};

use super::*;
use crate::api::{DuringWrite, Wait, WaitFor};
use crate::stopping::Ask;

mod feeder;

/// The output pipe is process-wide, so tests that run a server take turns
/// when they share a process.
pub(crate) static ONE_SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A server's state with nothing running and nothing read yet.
pub(crate) fn idle_state() -> BenchState {
    BenchState {
        now: 0.0,
        server: wellfed_formats::bench::live::ServerView {
            pid: 1,
            started: 0.0,
            phase: Phase::Serving,
            uart_port: false,
            stopping: None,
        },
        job: None,
        activity: Vec::new(),
        uart: Uart::Idle,
        relays: Default::default(),
        feeder: Default::default(),
        capture: Default::default(),
        banks: Default::default(),
        operator: Default::default(),
    }
}

/// Set in the process [`run_alone`] starts, which runs the test itself.
const ALONE: &str = "WELLFED_BENCH_SERVER_ALONE";

/// Whether the calling test is done, having run in a process of its own.
///
/// A test that points descriptors 1 and 2 at a pipe or a file takes them
/// from the whole process, and under plain `cargo test` the harness itself
/// writes there from other threads: its progress lands in the job streams
/// and terminals the tests read. No lock keeps the harness out, so each such
/// test runs again as the only test of a fresh process of this binary, and
/// the first one just waits for it. Under cargo-nextest, or one test thread,
/// every test already has the descriptors to itself.
pub(crate) fn run_alone() -> bool {
    if std::env::var_os(ALONE).is_some() || std::env::var_os("NEXTEST").is_some() {
        return false;
    }
    // The harness names a test's thread after it, unless it runs the tests
    // one at a time on its main thread.
    let current = std::thread::current();
    let Some(name) = current.name().filter(|name| *name != "main") else {
        return false;
    };

    let ran = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--test-threads=1", "--nocapture"])
        .env(ALONE, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&ran.stdout);
    let stderr = String::from_utf8_lossy(&ran.stderr);
    assert!(
        ran.status.success(),
        "{name} failed in a process of its own:\n{stdout}\n{stderr}"
    );
    // A name that matched nothing would pass having run nothing. The
    // harness's closing summary may go to the test's pipe, never to this
    // output, so the count it starts with and the test's own line say it.
    let started = format!("test {name} ... ");
    assert!(
        stdout.contains("\nrunning 1 test\n") && stdout.contains(&started),
        "{name} did not run exactly once in its own process:\n{stdout}\n{stderr}"
    );
    true
}

/// A checkout with a bench config and a stand-in flash client:
/// - `read observe` holds the port for `hold` seconds; a capture named
///   `burst` first prints 3000 lines on stderr;
/// - `update execute` reports 3 of 19 sectors, then writes until a signal,
///   which it records in `update-signal` at the root.
struct Checkout {
    dir: tempfile::TempDir,
    board: FakeBoard,
}

impl Checkout {
    fn new(hold: f64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("private/verification")).unwrap();
        fs::write(
            root.join("private/bench.json"),
            r#"{"relay_port": "relay", "uart_port": "uart", "baseline": "b", "rom_report": "r"}"#,
        )
        .unwrap();

        // Arguments: --root R read observe --port P --output O --seconds S
        let script = format!(
            r#"#!/bin/sh
case "$3 $4" in
"read observe")
  out="$8"
  mkdir -p "$out"
  case "$out" in *burst)
    i=0; while [ $i -lt 3000 ]; do echo "burst line $i of the capture's own output" >&2; i=$((i+1)); done ;;
  esac
  echo '{{"status":"running"}}' > "$out/report.json"
  trap 'echo INT > "$out/signal"; kill $! 2>/dev/null; exit 1' INT
  trap 'echo TERM > "$out/signal"; kill $! 2>/dev/null; exit 1' TERM
  sleep {hold} & wait $!
  echo '{{"status":"complete","result":{{"bytes":7}}}}' > "$out/report.json"
  ;;
"update execute")
  trap 'echo INT > "$2/update-signal"; kill $! 2>/dev/null; exit 1' INT
  trap 'echo TERM > "$2/update-signal"; kill $! 2>/dev/null; exit 1' TERM
  sleep 0.3
  echo "Verified changed sectors: 3/19" >&2
  sleep 30 & wait $!
  ;;
esac
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

    fn options(&self) -> Options {
        let root = self.root();
        Options {
            tools: ToolPaths {
                flash_bin: root.join("wellfed-flash"),
                stock_bin: root.join("absent"),
                build_bin: root.join("absent"),
            },
            root,
            console: crate::Console::Terminal,
        }
    }

    fn open(&self) -> Server {
        let board = self.board.clone();
        Server::open(self.options(), move |_| {
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
    /// The checkout the server runs for: its socket's `private/`'s parent.
    root: PathBuf,
    shared: Arc<crate::server::Shared>,
}

impl Running {
    fn start(server: Server) -> Self {
        Self::start_asked(server, None)
    }

    /// A server that also stops as its stopper decides, from `asked`.
    fn start_asked(server: Server, asked: Option<mpsc::UnboundedReceiver<Ask>>) -> Self {
        let client = Client::at(server.socket().to_path_buf());
        let root = server.socket().ancestors().nth(2).unwrap().to_path_buf();
        let shared = server.shared();
        let (stop, stopped) = oneshot::channel();
        let stopper = server.stopper();
        let done = tokio::spawn(server.run(async move {
            match asked {
                Some(asked) => tokio::select! {
                    () = stopper.until_stop(asked) => {}
                    _ = stopped => {}
                },
                None => {
                    let _ = stopped.await;
                }
            }
        }));
        Self {
            stop: Some(stop),
            done,
            client,
            root,
            shared,
        }
    }

    async fn submit(&self, words: &[&str]) -> Result<JobId, ClientError> {
        self.client.submit(&self.root, &argv(words)).await
    }

    async fn state(&self) -> BenchState {
        self.client.state().await.unwrap()
    }

    async fn quit(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.done.await.unwrap().unwrap();
    }
}

/// A job's words, with a short power-off so cold boots are quick.
fn argv(words: &[&str]) -> Vec<String> {
    words
        .iter()
        .chain(&["--off-seconds", "0.05"])
        .map(|word| (*word).to_string())
        .collect()
}

/// Follow `id` to its end, with the lines it printed.
async fn follow(client: &Client, id: JobId) -> (JobEnd, Vec<String>) {
    let (_interrupts, mut receiver) = mpsc::unbounded_channel();
    let mut lines = Vec::new();
    let followed = client
        .follow(id, |line| lines.push(line.to_string()), &mut receiver)
        .await
        .unwrap();
    match followed {
        Followed::Ended(end) => (end, lines),
        Followed::Detached => panic!("detached"),
    }
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Read the state until `done` holds of it.
async fn state_until(
    server: &Running,
    what: &str,
    mut done: impl FnMut(&BenchState) -> bool,
) -> BenchState {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state = server.state().await;
        if done(&state) {
            return state;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_streams_its_output_and_ends_with_its_code() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.5);
    // A job number an earlier server used is never used again.
    fs::create_dir_all(checkout.root().join(JOBS)).unwrap();
    fs::write(checkout.root().join(JOBS).join("41-old.log"), "").unwrap();
    let server = Running::start(checkout.open());

    // Opening queried every relay.
    let state = server.state().await;
    assert_eq!(state.relays.len(), 4);
    assert_eq!(state.server.phase, Phase::Serving);

    let id = server.submit(&["capture", "c1"]).await.unwrap();
    assert_eq!(id, 42);
    let (end, lines) = follow(&server.client, id).await;

    assert_eq!((end.code, end.error), (0, None));
    let captured = "Captured 7 bytes in private/captures/c1".to_string();
    assert!(lines.contains(&captured), "{lines:?}");
    let job = server.state().await.job.unwrap();
    assert_eq!((job.id, job.label.as_str()), (id, "capture c1"));
    assert_eq!(job.log, "private/bench-status/jobs/42-c1.log");
    let kept = fs::read_to_string(checkout.root().join(&job.log)).unwrap();
    assert!(kept.lines().any(|line| line == captured), "{kept}");
    let state = server.state().await;
    assert!(state.activity.is_empty() && state.uart == Uart::Idle);

    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_job_is_refused_while_one_runs() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(3.0);
    let server = Running::start(checkout.open());
    let first = server.submit(&["capture", "c1"]).await.unwrap();

    // The capture holds the UART while it runs.
    let state = state_until(&server, "the capture", |state| !state.activity.is_empty()).await;
    assert_eq!(state.activity[0].kind, ActivityKind::Flash);
    assert!(
        matches!(state.uart, Uart::Capturing { .. }),
        "{:?}",
        state.uart
    );

    let refused = server.submit(&["capture", "c2"]).await.unwrap_err();
    assert!(
        matches!(&refused, ClientError::Refused(api::Refusal::Busy { id, label })
            if *id == first && label == "capture c1"),
        "{refused:?}"
    );
    assert!(!checkout.capture("c2").exists());

    assert_eq!(follow(&server.client, first).await.0.code, 0);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_parses_every_job_and_refuses_what_clap_refuses() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let server = Running::start(checkout.open());

    let refusal = |result: Result<JobId, ClientError>| match result {
        Err(ClientError::Refused(refusal)) => refusal,
        other => panic!("not refused: {other:?}"),
    };
    // The catalog is the client's; as a job it would run the whole suite.
    assert!(matches!(
        refusal(server.submit(&["itest", "--list"]).await),
        api::Refusal::NotAJob { .. }
    ));
    for words in [
        &["capture"][..],
        &["itest"],
        &["itest", "i1", "--sustained-cycles", "0"],
        &["itest", "i1", "--rebuild"],
        &["itest", "i1", "--build", "--review", "r"],
        &["flash-prepare", "f1", "--target", "t", "--bank", "2"],
        &["ota", "o1", "--build", "--variant", "pulse"],
        &["serve"],
    ] {
        let refused = refusal(server.submit(words).await);
        assert!(
            matches!(&refused, api::Refusal::Unparsed { .. }),
            "{words:?}: {refused:?}"
        );
    }
    assert!(matches!(
        refusal(server.submit(&["capture", "../c3"]).await),
        api::Refusal::BadName { .. }
    ));
    let words = ["on".to_string(), "--off-seconds".into(), "1e9".into()];
    assert!(matches!(
        refusal(server.client.submit(&server.root, &words).await),
        api::Refusal::BadLength { .. }
    ));
    // The client sends its checkout beside the words, never as a flag.
    assert!(matches!(
        refusal(server.submit(&["on", "--root", "/elsewhere"]).await),
        api::Refusal::Unparsed { .. }
    ));
    // A checkout whose private/ links here reaches this server, and is
    // refused rather than run against this checkout's sources.
    let other = tempfile::tempdir().unwrap();
    let refused = refusal(server.client.submit(other.path(), &argv(&["off"])).await);
    assert!(
        matches!(&refused, api::Refusal::OtherCheckout { server: ours, client }
            if *ours == server.root.display().to_string()
                && *client == other.path().display().to_string()),
        "{refused:?}"
    );
    // Nothing ran.
    assert!(server.state().await.job.is_none());
    assert!(checkout.board.log().is_empty());

    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_interrupts_the_flash_client_and_the_cleanup_cuts_power() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(30.0);
    let server = Running::start(checkout.open());
    let id = server.submit(&["loader", "l1"]).await.unwrap();

    // Strapped, powered, then the strap released: the loader now only
    // waits on its capture.
    let board = checkout.board.clone();
    until("the strap's release", || {
        let log = board.log();
        log.ends_with(&[(STRAP, false)]) && log.contains(&(POWER, true))
    })
    .await;
    server.client.cancel(id, DuringWrite::Refuse).await.unwrap();

    let (end, lines) = follow(&server.client, id).await;
    assert_eq!(end.code, 1);
    assert!(end.error.is_some());
    let requested = format!("Cancel requested for job {id}");
    assert!(lines.contains(&requested), "{lines:?}");
    // The receive-only capture got SIGINT first and stopped on it.
    let signal = fs::read_to_string(checkout.capture("l1").join("signal")).unwrap();
    assert_eq!(signal.trim(), "INT");
    // The loader's cleanup: strap released, power cut, UART isolated.
    assert_eq!(checkout.board.state(), [false; 4]);
    let job = server.state().await.job.unwrap();
    assert_eq!(job.cancel, CancelState::Requested);

    // A cancel after the end changes nothing.
    let again = server
        .client
        .cancel(id, DuringWrite::Refuse)
        .await
        .unwrap_err();
    assert!(
        matches!(again, ClientError::Refused(api::Refusal::Finished { .. })),
        "{again:?}"
    );
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_mid_write_only_interrupts_the_flash_client_and_waits_for_it() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let root = checkout.root();
    fs::write(root.join("private/verification/u1-plan.json"), "{}").unwrap();
    // Powered with the strap released, as a prepare leaves the feeder.
    checkout.board.lock().on[1] = true;
    let server = Running::start(checkout.open());

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
        ])
        .await
        .unwrap();
    let state = state_until(&server, "the write's first count", |state| {
        state.uart
            == Uart::Writing {
                done: 3,
                total: 19,
                counted: true,
            }
    })
    .await;
    assert_eq!(state.activity[0].label, "update execute");
    // A cancel that does not say it means the write is refused, and the
    // writer is never signalled.
    let refused = server
        .client
        .cancel(id, DuringWrite::Refuse)
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            ClientError::Refused(api::Refusal::CancelDuringWrite { id: refused }) if refused == id
        ),
        "{refused:?}"
    );
    assert!(!root.join("update-signal").exists());
    server.client.cancel(id, DuringWrite::Cancel).await.unwrap();

    let (end, _) = follow(&server.client, id).await;
    assert_eq!(end.code, 1);
    // It ended only once the writer had stopped on SIGINT.
    let signal = fs::read_to_string(root.join("update-signal")).unwrap();
    assert_eq!(signal.trim(), "INT");
    // A stopped update is never followed by a boot: the feeder stays on.
    assert!(checkout.board.state()[0]);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_every_asker_gone_the_server_keeps_serving() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let (asks, asked) = mpsc::unbounded_channel();
    let server = Running::start_asked(checkout.open(), Some(asked));

    // As when the full-screen console's thread ends and its sender drops.
    drop(asks);
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(!server.done.is_finished());
    assert_eq!(server.state().await.server.phase, Phase::Serving);
    let id = server.submit(&["capture", "c1"]).await.unwrap();
    assert_eq!(follow(&server.client, id).await.0.code, 0);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigterm_during_a_write_lets_the_job_end_then_stops() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let root = checkout.root();
    fs::write(root.join("private/verification/u1-plan.json"), "{}").unwrap();
    checkout.board.lock().on[1] = true;
    let (asks, asked) = mpsc::unbounded_channel();
    let mut server = Running::start_asked(checkout.open(), Some(asked));

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
        ])
        .await
        .unwrap();
    state_until(&server, "the write", |state| {
        matches!(state.uart, Uart::Writing { .. })
    })
    .await;
    asks.send(Ask::Terminate).unwrap();
    asks.send(Ask::Terminate).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The writer was never signalled, and the server still serves, shows
    // the stop it holds, and takes no new job.
    assert!(!root.join("update-signal").exists());
    assert!(!server.done.is_finished());
    let state = server.state().await;
    assert!(
        matches!(state.uart, Uart::Writing { .. }),
        "{:?}",
        state.uart
    );
    assert_eq!(
        state.server.stopping.map(|deferred| deferred.job),
        Some(Some(id))
    );
    let refused = server.submit(&["capture", "c2"]).await.unwrap_err();
    assert!(
        matches!(
            refused,
            ClientError::Refused(api::Refusal::StoppingAfterWrite { job: Some(job) }) if job == id
        ),
        "{refused:?}"
    );

    // The stand-in writes until a signal: the operator cancels it, as a
    // real write would end on its own, and once the job ends the server
    // stops by itself.
    server.client.cancel(id, DuringWrite::Cancel).await.unwrap();
    let (end, _) = follow(&server.client, id).await;
    assert_eq!(end.code, 1);
    tokio::time::timeout(Duration::from_secs(20), &mut server.done)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    server.stop.take();
}

/// A wait that checks its job number reads the jobs and the state; a job
/// that starts holds the jobs lock while it writes the state. Taken in
/// opposite orders the two deadlock, which this shows as a timeout: the
/// wait's own check against the locks a start takes, in tight loops on
/// threads of their own, then real waits over the socket beside real jobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waits_and_job_starts_side_by_side_never_deadlock() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.0);
    let server = Running::start(checkout.open());
    let wait = Wait {
        what: WaitFor::Job,
        id: Some(1),
        text: None,
        kind: None,
        build: None,
        bank: None,
        label: None,
        timeout_ms: 0,
    };

    let done = Arc::new(AtomicBool::new(false));
    let (finished, finishing) = std::sync::mpsc::channel();
    for _ in 0..3 {
        let (shared, done, wait) = (Arc::clone(&server.shared), Arc::clone(&done), wait.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let _ = crate::wait::checked(&shared, &wait);
            }
        });
    }
    let shared = Arc::clone(&server.shared);
    std::thread::spawn(move || {
        for _ in 0..200_000 {
            shared.lock_as_a_start_does();
        }
        let _ = finished.send(());
    });
    let locked =
        tokio::task::spawn_blocking(move || finishing.recv_timeout(Duration::from_secs(60)))
            .await
            .unwrap();
    done.store(true, Ordering::SeqCst);
    assert!(locked.is_ok(), "a wait's check and a job start deadlocked");

    let client = Arc::new(Client::at(server.root.join(api::SOCKET)));
    let waiting = Arc::new(AtomicBool::new(true));
    let mut waiters = Vec::new();
    for _ in 0..3 {
        let (client, waiting, wait) = (Arc::clone(&client), Arc::clone(&waiting), wait.clone());
        waiters.push(tokio::spawn(async move {
            while waiting.load(Ordering::SeqCst) {
                let _ = client.wait(&wait).await;
            }
        }));
    }
    let started = tokio::time::timeout(Duration::from_secs(60), async {
        for _ in 0..10 {
            let id = server.submit(&["on"]).await.unwrap();
            follow(&server.client, id).await;
        }
    })
    .await;
    waiting.store(false, Ordering::SeqCst);
    assert!(started.is_ok(), "a wait and a job start deadlocked");
    for waiter in waiters {
        tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .unwrap()
            .unwrap();
    }
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_burst_of_output_streams_without_blocking() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let server = Running::start(checkout.open());

    let id = server.submit(&["capture", "burst"]).await.unwrap();
    let (end, lines) = tokio::time::timeout(Duration::from_secs(30), follow(&server.client, id))
        .await
        .expect("the burst blocked the job");

    assert_eq!(end.code, 0);
    let burst = lines
        .iter()
        .filter(|line| line.starts_with("burst line"))
        .count();
    assert_eq!(burst, 3000);
    // Well past a pipe's 64 KiB.
    let bytes: usize = lines.iter().map(|line| line.len() + 1).sum();
    assert!(bytes > 100 << 10, "{bytes}");
    server.quit().await;
}

/// A board that panics on its first switch, then works.
struct PanicsOnce {
    panicked: Arc<AtomicBool>,
    board: AtBoard,
}

impl Board for PanicsOnce {
    fn set(&mut self, channel: u8, switch: Switch) -> wellfed_relay::Result<()> {
        assert!(
            self.panicked.swap(true, Ordering::SeqCst),
            "a bug in the job"
        );
        self.board.set(channel, switch)
    }

    fn get(&mut self, channel: u8) -> wellfed_relay::Result<bool> {
        self.board.get(channel)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_job_ends_and_the_next_one_runs() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let fake = checkout.board.clone();
    let opened = Server::open(checkout.options(), move |_| {
        Ok(Box::new(PanicsOnce {
            panicked: Arc::default(),
            board: AtBoard::new(Box::new(fake)),
        }))
    })
    .unwrap();
    let server = Running::start(opened);

    let id = server.submit(&["on"]).await.unwrap();
    let (end, _) = follow(&server.client, id).await;
    assert_eq!(
        (end.code, end.error.as_deref()),
        (1, Some("The job panicked"))
    );

    let next = server.submit(&["on"]).await.unwrap();
    assert_eq!(follow(&server.client, next).await.0.code, 0);
    assert!(checkout.board.state()[0]);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_poll_skips_while_a_job_runs() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(2.0);
    let server = Running::start(checkout.open());
    let id = server.submit(&["capture", "c1"]).await.unwrap();
    state_until(&server, "the capture", |state| !state.activity.is_empty()).await;

    let shared = Arc::clone(&server.shared);
    let during = tokio::task::spawn_blocking(move || shared.poll_relays())
        .await
        .unwrap();
    assert!(!during, "polled the board under a job");

    follow(&server.client, id).await;
    let shared = Arc::clone(&server.shared);
    let after = tokio::task::spawn_blocking(move || shared.poll_relays())
        .await
        .unwrap();
    assert!(after);
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_posted_as_the_last_one_ends_is_never_busy() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(0.1);
    let server = Running::start(checkout.open());

    for _ in 0..5 {
        let id = server.submit(&["off"]).await.unwrap();
        assert_eq!(follow(&server.client, id).await.0.code, 0);
    }
    server.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_detaches_and_another_attaches_from_the_first_line() {
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(1.0);
    let server = Running::start(checkout.open());
    let id = server.submit(&["capture", "c1"]).await.unwrap();

    let (interrupts, mut receiver) = mpsc::unbounded_channel();
    interrupts.send(Interrupt::Detach).unwrap();
    let detached = server
        .client
        .follow(id, |_| {}, &mut receiver)
        .await
        .unwrap();
    assert_eq!(detached, Followed::Detached);
    // The job runs on without a reader.
    assert!(server.state().await.job.unwrap().end.is_none());

    let (end, lines) = follow(&server.client, id).await;
    assert_eq!(end.code, 0);
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
    if crate::tests::run_alone() {
        return;
    }
    let _one = ONE_SERVER.lock().await;
    let checkout = Checkout::new(30.0);
    let root = checkout.root();
    // Whatever the caller's umask, the socket is the owner's alone.
    let was = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o002));
    let server = Running::start(checkout.open());
    rustix::process::umask(was);
    // The server holds the bench for its life.
    assert!(lease::inspect(&root).unwrap().is_some());
    server.submit(&["loader", "l1"]).await.unwrap();
    let board = checkout.board.clone();
    // Powered with the strap held, then released: the loader now only
    // waits on its capture. The sequence also releases the strap before
    // anything else, so that alone is no sign its observe started.
    until("the strap's release", || {
        let log = board.log();
        log.ends_with(&[(STRAP, false)]) && log.contains(&(POWER, true))
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
fn a_bank_ledger_that_does_not_read_refuses_the_start_before_the_board_opens() {
    let checkout = Checkout::new(0.1);
    fs::create_dir_all(checkout.root().join("private/bench-status")).unwrap();
    fs::write(checkout.root().join("private/bench-status/banks.json"), "{").unwrap();
    let opened = AtomicBool::new(false);

    let refused = Server::open(checkout.options(), |_| {
        opened.store(true, Ordering::SeqCst);
        Ok(Box::new(AtBoard::new(Box::new(checkout.board.clone()))))
    });

    let Err(ServeError::Banks { path, .. }) = refused else {
        panic!("a corrupt ledger was taken for an empty one");
    };
    assert!(path.ends_with("private/bench-status/banks.json"));
    assert!(!opened.load(Ordering::SeqCst));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{",
        "the ledger stays for the operator"
    );
}
