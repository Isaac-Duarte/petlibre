//! The release check: the server as it ships, a separate process on a
//! fresh data directory, set up over HTTP, adopting the simulated feeder by
//! address with discovery off, reading its status, stopped by SIGTERM as a
//! service manager stops it, and started again to reconnect by the stored
//! address.
//!
//! By default it runs the binary cargo built for these tests. Point
//! `WELLFED_RELEASE_SERVER` at another (a static release binary) to check
//! that one. The container image's run is the ignored test: it needs
//! `WELLFED_RELEASE_IMAGE` and a container runtime, so it never runs in
//! `make check`. docs/api/README.md#packaging has both commands.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use wellfed_api::{AdoptFeeder, ClientRole, Connection, FeederLink, StoreState};
use wellfed_server_client::{Client, Url};
use wellfed_server_testkit::{ada_login, ada_setup, feeder, local, until};
use tokio::sync::oneshot;

/// How long a fresh process may take to listen, or to stop.
const START: Duration = Duration::from_secs(30);

/// The line the server logs once it listens, before its address.
const SERVING: &str = "serving on http://";

/// Where the image keeps its data, as its `WELLFED_SERVER_DATA` says.
const IMAGE_DATA: &str = "/data";

/// How the server is started: the binary itself, or the image under a
/// container runtime.
enum Launch {
    Binary(PathBuf),
    Image { runtime: String, image: String },
}

impl Launch {
    /// The command that serves `data` on a free localhost port, adopting
    /// and reconnecting by address only.
    fn command(&self, data: &Path, name: &str) -> Command {
        let mut command = match self {
            Self::Binary(path) => {
                let mut command = Command::new(path);
                command.arg("--data").arg(data);
                command
            }
            Self::Image { runtime, image } => {
                // Host networking, so the server and the simulator share
                // localhost; the data directory's owner runs it, so the
                // files it writes stay the test's to remove.
                let (uid, gid) = owner(data);
                let mut command = Command::new(runtime);
                command.args(["run", "--rm", "--network", "host", "--name", name]);
                if Path::new(runtime).file_name() == Some("podman".as_ref()) {
                    command.arg("--userns=keep-id");
                }
                command
                    .arg("--user")
                    .arg(format!("{uid}:{gid}"))
                    .arg("--volume")
                    .arg(format!("{}:{IMAGE_DATA}", data.display()))
                    .arg(image);
                command
            }
        };

        // Only the flags given here: a setting from the caller's
        // environment would change what is checked.
        for (variable, _) in std::env::vars_os() {
            if variable
                .to_str()
                .is_some_and(|name| name.starts_with("WELLFED_SERVER_"))
            {
                command.env_remove(variable);
            }
        }
        command
            .args(["--listen", "127.0.0.1:0", "--no-discovery"])
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        command
    }

    /// Remove a container a failed test left running.
    fn clean_up(&self, name: &str) {
        if let Self::Image { runtime, .. } = self {
            let _ = Command::new(runtime)
                .args(["rm", "--force", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn owner(path: &Path) -> (u32, u32) {
    let metadata = std::fs::metadata(path).expect("the data directory exists");
    (metadata.uid(), metadata.gid())
}

/// A running server process, killed if a failing test drops it.
struct Server<'a> {
    launch: &'a Launch,
    name: String,
    child: Option<Child>,
    address: SocketAddr,
}

impl<'a> Server<'a> {
    /// Start the server on `data` and wait until it listens.
    async fn start(launch: &'a Launch, data: &Path, run: u32) -> Self {
        let name = format!("wellfed-release-check-{}-{run}", std::process::id());
        let mut child = launch
            .command(data, &name)
            .spawn()
            .expect("the server starts");
        let stdout = child.stdout.take().expect("stdout is piped");

        // The log is read to its end, so the server never blocks on a full
        // pipe; each line is echoed for a failing test's output.
        let (listening, address) = oneshot::channel();
        std::thread::spawn(move || {
            let mut listening = Some(listening);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                eprintln!("server: {line}");
                if let Some(at) = line.find(SERVING)
                    && let Some(address) = line[at + SERVING.len()..]
                        .split_whitespace()
                        .next()
                        .and_then(|address| address.parse::<SocketAddr>().ok())
                    && let Some(listening) = listening.take()
                {
                    let _ = listening.send(address);
                }
            }
        });

        let mut server = Self {
            launch,
            name,
            child: Some(child),
            address: SocketAddr::from(([0, 0, 0, 0], 0)),
        };
        server.address = tokio::time::timeout(START, address)
            .await
            .expect("the server listens in time")
            .expect("the server logs its address before its output ends");
        server
    }

    fn client(&self) -> Client {
        let base = Url::parse(&format!("http://{}", self.address)).expect("an address is a URL");
        Client::new(&base).expect("an http URL makes a client")
    }

    /// Stop the server as systemd and container runtimes do, with SIGTERM,
    /// and wait for it to exit.
    /// The child stays in `self` until it has exited, so a panic on the way
    /// (no `kill`, a hung shutdown) leaves it to [`Drop`] to kill and clean
    /// up.
    async fn terminate(mut self) -> ExitStatus {
        let pid = self
            .child
            .as_ref()
            .expect("the server runs until stopped")
            .id();
        let signal = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .expect("kill runs");
        assert!(signal.success(), "SIGTERM is sent");

        let polled = tokio::time::timeout(START, async {
            loop {
                let child = self.child.as_mut().expect("the server runs until stopped");
                if let Some(status) = child.try_wait().expect("the server's status reads") {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        let status = polled.expect("the server stops on SIGTERM in time");

        self.child = None;
        status
    }
}

impl Drop for Server<'_> {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.launch.clean_up(&self.name);
        }
    }
}

/// The whole check against `launch`.
fn release_check(launch: &Launch) {
    local(async {
        let sim = feeder::start().await;
        let data = tempfile::tempdir().expect("a temporary data directory");

        // A fresh start: set up, adopt by address, read the status.
        let server = Server::start(launch, data.path(), 1).await;
        let mut owner = server.client();
        assert_eq!(
            owner.health().await.expect("liveness answers").version,
            env!("CARGO_PKG_VERSION")
        );
        owner
            .setup(&ada_setup())
            .await
            .expect("the household sets up");
        let kitchen = owner
            .adopt(&AdoptFeeder {
                invite: feeder::invite(&sim, ClientRole::Server).await,
                name: "kitchen".into(),
                address: Some(sim.address.to_string()),
            })
            .await
            .expect("the server adopts the simulator by address")
            .id;
        let online = [FeederLink {
            id: kitchen,
            name: "kitchen".into(),
            connection: Connection::Online,
        }];
        until("the feeder shows online", || async {
            owner.status().await.expect("readiness answers").feeders == online
        })
        .await;
        let status = owner.status().await.expect("readiness answers");
        assert_eq!(status.store, StoreState::Ok);
        let detail = owner.feeder(kitchen).await.expect("the feeder reads");
        let live = detail.status.expect("the feeder's status is mirrored");
        assert!(live.live, "the status is the feeder's own, read live");
        assert!(live.status.clock.is_some(), "the server set the clock");

        // A backup lands in the data directory, with no key beside it.
        let backup = owner.backup().await.expect("a backup is taken");
        let backups = data.path().join("backups");
        assert!(backups.join(&backup.name).is_file(), "{backup:?}");
        assert!(backups.join("README.txt").is_file());

        assert!(server.terminate().await.success(), "a clean stop");
        let key = data.path().join("keys").join("server.key");
        assert_eq!(
            std::fs::read(&key).expect("the key file is written").len(),
            32
        );
        let mode = std::fs::metadata(&key).expect("the key file").permissions();
        assert_eq!(mode.mode() & 0o777, 0o600, "only the server reads its key");

        // A restart on the same directory reconnects by the stored
        // address, with no discovery to fall back on.
        let server = Server::start(launch, data.path(), 2).await;
        let mut owner = server.client();
        owner.login(&ada_login()).await.expect("the owner logs in");
        until("the feeder reconnects", || async {
            owner.status().await.expect("readiness answers").feeders == online
        })
        .await;
        let detail = owner.feeder(kitchen).await.expect("the feeder reads");
        assert!(detail.status.expect("the status is mirrored").live);
        assert!(server.terminate().await.success(), "a clean stop");
    });
}

#[test]
fn the_binary_starts_fresh_and_adopts_the_simulator() {
    let binary = std::env::var_os("WELLFED_RELEASE_SERVER").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_wellfed-server")),
        PathBuf::from,
    );
    release_check(&Launch::Binary(binary));
}

#[test]
#[ignore = "needs WELLFED_RELEASE_IMAGE and a container runtime"]
fn the_image_starts_fresh_and_adopts_the_simulator() {
    let image = std::env::var("WELLFED_RELEASE_IMAGE")
        .expect("WELLFED_RELEASE_IMAGE names the image to check");
    let runtime = std::env::var("WELLFED_RELEASE_RUNTIME").unwrap_or_else(|_| "docker".into());
    release_check(&Launch::Image { runtime, image });
}
