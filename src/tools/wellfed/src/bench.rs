//! The bench server's session (`wellfed-bench serve`), when one runs for
//! this checkout: it holds the one owner session with the research unit,
//! and every device command goes through it on `private/bench.sock`.
//!
//! Whenever a server answers there, a direct session is refused rather
//! than opened beside it: the server's session never resends, and a
//! request that got no answer through it must not go again another way.
//! Nor does the server lend its session to another key, a given address,
//! or the commands that open sessions of their own (`setup`, `install`,
//! `recover`, `join`, `restore-stock`).
//!
//! The server's subscription and log tail are its own, so this side
//! replays what the server kept rather than move either: `subscribe`
//! reads the pushed messages the server holds from the start of its ring,
//! and a `TailLog` reads its log lines the same way.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use tokio::net::UnixStream;
use wellfed_formats::bench::feeder::{Ask, Asked, DeviceUnavailable, PushedPage, SessionInfo};
use wellfed_model::{Answer, Capabilities, ClientRole, Op, Pushed};

/// Where the bench server listens: `private/bench.sock`, from the checkout.
pub(crate) const SOCKET: &str = "private/bench.sock";

/// The socket of the server that would hold `keys`'s session: beside the
/// key file, which for the default `private/wellfed-owner.json` is
/// [`SOCKET`].
pub(crate) fn beside(keys: &Path) -> PathBuf {
    keys.parent().unwrap_or(Path::new(".")).join("bench.sock")
}

/// The bench server, once it answered.
pub(crate) struct Bench {
    socket: PathBuf,
}

impl Bench {
    /// The server on `socket`, if one answers there. A socket file that
    /// nothing listens on is a killed server's, and no server.
    pub(crate) async fn probe(socket: &Path) -> Result<Option<Self>> {
        let socket = socket.to_path_buf();
        match UnixStream::connect(&socket).await {
            Ok(_) => Ok(Some(Self { socket })),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error)
                .with_context(|| format!("reach the bench server on {}", socket.display())),
        }
    }

    async fn send(&self, method: Method, path: &str, body: Vec<u8>) -> Result<(StatusCode, Bytes)> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .with_context(|| format!("reach the bench server on {}", self.socket.display()))?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
        tokio::spawn(connection);

        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "bench")
            .header(hyper::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))?;
        let response = sender.send_request(request).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes();
        Ok((status, body))
    }

    /// The answer's body when it is `OK`; the session's reason when it
    /// cannot carry the request.
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T> {
        let (status, body) = self.send(method, path, body).await?;
        match status {
            StatusCode::OK => Ok(serde_json::from_slice(&body)?),
            StatusCode::SERVICE_UNAVAILABLE => {
                let refused: DeviceUnavailable = serde_json::from_slice(&body)?;
                Err(refused.into())
            }
            status => bail!(
                "the bench server answered {status}: {}",
                String::from_utf8_lossy(&body)
            ),
        }
    }

    async fn session(&self) -> Result<SessionInfo> {
        self.call(Method::GET, "/device/session", Vec::new()).await
    }

    async fn pushed(&self, from: u64, wait: Duration) -> Result<PushedPage> {
        let path = format!("/device/pushed?from={from}&wait={}", wait.as_millis());
        self.call(Method::GET, &path, Vec::new()).await
    }
}

/// The server's session, lent for one command.
pub(crate) struct Lent {
    bench: Bench,
    info: SessionInfo,
    /// The next pushed message to read from the server's ring.
    cursor: u64,
    queued: VecDeque<Pushed>,
    /// Events and gaps after this one, once subscribed.
    events: Option<Option<u64>>,
    /// Log lines after this one, once a tail was asked for.
    logs: Option<Option<u32>>,
}

/// The server's session for `keys`, refusing whatever it cannot lend.
pub(crate) async fn lend(
    bench: Bench,
    keys: &Path,
    feeder: Option<&str>,
    address: Option<&str>,
) -> Result<Lent> {
    if address.is_some() {
        bail!(
            "the bench server holds the feeder's session; --address would open another (stop \
             `wellfed-bench serve` to talk to the feeder directly)"
        );
    }
    let info = bench.session().await?;
    let ours = keys
        .canonicalize()
        .with_context(|| format!("read the key file {}", keys.display()))?;
    if ours != info.keys {
        bail!(
            "the bench server's session speaks as {}, not {}; it lends no other key",
            info.keys.display(),
            ours.display()
        );
    }
    if let Some(prefix) = feeder
        && !info.identity.starts_with(&prefix.to_ascii_lowercase())
    {
        bail!(
            "the bench server's session is with feeder {}, not {prefix}",
            &info.identity[..16]
        );
    }
    // Messages from here on, until a subscribe or a tail replays the ring.
    let cursor = bench.pushed(u64::MAX, Duration::ZERO).await?.next;
    Ok(Lent {
        bench,
        info,
        cursor,
        queued: VecDeque::new(),
        events: None,
        logs: None,
    })
}

impl Lent {
    pub(crate) fn identity(&self) -> &str {
        &self.info.identity
    }

    pub(crate) fn build(&self) -> &str {
        &self.info.build
    }

    pub(crate) fn role(&self) -> ClientRole {
        self.info.role
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.info.capabilities
    }

    /// Events after `after`, from what the server's subscription kept.
    pub(crate) fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<()> {
        if samples {
            bail!("the bench server's session is subscribed without samples");
        }
        self.events = Some(after);
        self.replay();
        Ok(())
    }

    fn replay(&mut self) {
        self.cursor = 0;
        self.queued.clear();
    }

    /// The next wanted message within `wait`; `None` when none came.
    pub(crate) async fn next(&mut self, wait: Duration) -> Result<Option<Pushed>> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(message) = self.queued.pop_front() {
                return Ok(Some(message));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let page = self.bench.pushed(self.cursor, left).await?;
            self.cursor = page.next;
            let wanted: Vec<Pushed> = page
                .messages
                .into_iter()
                .filter(|message| self.wants(message))
                .collect();
            self.queued.extend(wanted);
            if self.queued.is_empty() && Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    fn wants(&self, message: &Pushed) -> bool {
        match (message, self.events, self.logs) {
            (Pushed::Event(event), Some(after), _) => after.is_none_or(|after| event.seq > after),
            (Pushed::Gap(gap), Some(after), _) => after.is_none_or(|after| gap.to > after),
            (Pushed::Log(batch), _, Some(after)) => after.is_none_or(|after| batch.seq > after),
            _ => false,
        }
    }

    /// One request, sent once by the server under a fresh id. A tail is the
    /// server's own already: its lines are replayed, and the feeder's log
    /// cursor stays where the server's session left it.
    pub(crate) async fn ask(&mut self, op: &Op) -> Result<Answer> {
        if let Op::TailLog { after } = op {
            self.logs = Some(*after);
            self.replay();
            return Ok(Answer::Done { generation: None });
        }

        let body = serde_json::to_vec(&Ask {
            id: crate::device::random()?,
            op: op.clone(),
        })?;
        match self.bench.call(Method::POST, "/device/ask", body).await? {
            Asked::Answered { answer } => Ok(answer),
            Asked::Uncertain => bail!(
                "no answer came through the bench server; the request may have run, and is \
                 not sent again"
            ),
        }
    }
}

/// The CLI's side of the device routes, against a stand-in that serves
/// them from a session of its own with the simulator: the CLI never links
/// the bench, whose relay driver only the bench may. The server's side of
/// the same routes is `wellfed-bench-server`'s tests.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Json, Response};
    use axum::routing::{get, post};
    use clap::Parser;
    use tokio::sync::Mutex;
    use wellfed_feeder_sim::harness::Threaded;
    use wellfed_formats::bench::live::{Pause, Unavailable};

    use super::*;
    use crate::device::Connection;
    use crate::{Cli, Command, Target, connect};

    const OWNER: [u8; 32] = [0x22; 32];

    struct Stand {
        connection: Mutex<wellfed_connect::Connection>,
        info: SessionInfo,
        paused: bool,
    }

    async fn session(State(stand): State<Arc<Stand>>) -> Response {
        if stand.paused {
            let reason = Unavailable::Paused {
                pause: Pause::PowerOff,
            };
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(DeviceUnavailable { reason }),
            )
                .into_response();
        }
        Json(stand.info.clone()).into_response()
    }

    async fn ask(State(stand): State<Arc<Stand>>, Json(ask): Json<Ask>) -> Json<Asked> {
        let answer = stand.connection.lock().await.ask(ask.id, &ask.op).await;
        Json(match answer {
            Ok(answer) => Asked::Answered { answer },
            Err(_) => Asked::Uncertain,
        })
    }

    async fn pushed() -> Json<PushedPage> {
        Json(PushedPage {
            first: 0,
            next: 0,
            messages: Vec::new(),
        })
    }

    /// A checkout holding the owner's key file, and the stand-in on its
    /// `private/bench.sock`.
    async fn stand_in(feeder: &Threaded, paused: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("private")).unwrap();
        let keys = root.join("private/wellfed-owner.json");
        let file = serde_json::json!({
            "version": 1,
            "secret": wellfed_formats::hex::encode(&OWNER),
            "label": "bench",
            "feeders": [{ "identity": wellfed_formats::hex::encode(&feeder.identity) }],
        });
        fs::write(&keys, file.to_string()).unwrap();

        let keypair = wellfed_session::noise::Keypair::from_secret(OWNER);
        let connection =
            wellfed_connect::Connection::open(&keypair, feeder.identity, feeder.address, [9; 32])
                .await
                .unwrap();
        let info = SessionInfo {
            keys: keys.clone(),
            identity: wellfed_formats::hex::encode(&feeder.identity),
            build: connection.build().to_string(),
            role: connection.role(),
            capabilities: connection.capabilities(),
        };
        let stand = Arc::new(Stand {
            connection: Mutex::new(connection),
            info,
            paused,
        });
        let router = axum::Router::new()
            .route("/device/session", get(session))
            .route("/device/ask", post(ask))
            .route("/device/pushed", get(pushed))
            .with_state(stand);
        let listener = tokio::net::UnixListener::bind(root.join(SOCKET)).unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });
        (dir, root)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn device_commands_go_through_the_bench_server_whenever_one_answers() {
        let owner = *wellfed_session::noise::Keypair::from_secret(OWNER).public();
        let feeder = Threaded::start(owner);
        let (_dir, root) = stand_in(&feeder, false).await;
        let keys = root.join("private/wellfed-owner.json");

        let target = Target {
            keys: keys.clone(),
            feeder: None,
            address: None,
        };
        let (_, identity, connection) = connect(&target).await.unwrap();
        assert!(matches!(connection, Connection::Bench(_)));
        assert_eq!(identity, feeder.identity);

        // `wellfed status` as the command line runs it.
        let cli =
            Cli::try_parse_from(["wellfed", "status", "--keys", keys.to_str().unwrap()]).unwrap();
        let Command::Status(args) = cli.command else {
            panic!("not a status command");
        };
        crate::history::status_command(args).await.unwrap();

        // Beside the server, a direct session is refused, never opened.
        let direct = Target {
            address: Some(feeder.address.to_string()),
            ..target
        };
        let refused = connect(&direct).await.err().unwrap().to_string();
        assert!(
            refused.contains("--address would open another"),
            "{refused}"
        );
        // Nor does the server lend its session to another key.
        let other = root.join("private/other-owner.json");
        fs::copy(&keys, &other).unwrap();
        let refused = connect(&Target {
            keys: other,
            feeder: None,
            address: None,
        })
        .await
        .err()
        .unwrap()
        .to_string();
        assert!(refused.contains("lends no other key"), "{refused}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paused_server_refuses_with_its_reason_and_nothing_goes_direct() {
        let owner = *wellfed_session::noise::Keypair::from_secret(OWNER).public();
        let feeder = Threaded::start(owner);
        let (_dir, root) = stand_in(&feeder, true).await;

        let target = Target {
            keys: root.join("private/wellfed-owner.json"),
            feeder: None,
            address: None,
        };
        let refused = connect(&target).await.err().unwrap();

        let reason = refused.downcast_ref::<DeviceUnavailable>().unwrap();
        assert_eq!(
            reason.reason,
            Unavailable::Paused {
                pause: Pause::PowerOff
            }
        );
    }
}
