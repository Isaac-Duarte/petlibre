//! The bench server's session (`wellfed-bench serve`), when one runs for
//! this user: it holds the one owner session with the research unit, and
//! every device command goes through it.
//!
//! The server is found by its per-user pointer
//! (`wellfed_formats::bench::rendezvous`), whatever this command's working
//! directory or key file. While the server it names answers:
//! - a command whose key is the server's goes through its session;
//! - anything else is refused with [`BenchRefusal`], naming the server:
//!   another key, `--address`, and the commands that open sessions of their
//!   own (`setup`, `install`, `recover`, `join`, `restore-stock`);
//! - a direct session is never opened beside it, nor used as a fallback: a
//!   write that got no answer through the server must not go again another
//!   way.
//!
//! A pointer whose process is gone (killed, or its pid reused) names no
//! server. One whose process lives but whose socket does not answer
//! refuses every device command: that server may still hold the feeder.
//!
//! The server's subscription and log tail are its own, so this side
//! replays what the server kept rather than move either: `subscribe` reads
//! the history the server holds, and a `TailLog` the log lines it holds.
//! Both refuse up front, before any write can go out, when the server's
//! rings do not reach back far enough, and a follower the ring outran is
//! stopped rather than moved on silently.

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
use wellfed_formats::bench::feeder::{
    Ask, Asked, DeviceUnavailable, Opened, PushedPage, SessionInfo, Stream,
};
use wellfed_formats::bench::rendezvous;
use wellfed_formats::owner::OwnerKeyFile;
use wellfed_model::{Answer, Capabilities, ClientRole, Op, Pushed};
use wellfed_session::noise::Keypair;

/// How long a lent session may take to open, as the server lets a request
/// wait for it.
const OPEN_WAIT: Duration = Duration::from_secs(20);
const OPEN_POLL: Duration = Duration::from_millis(200);

/// Why a running bench server refused this command its session. Each names
/// the server's checkout: stopping that server is the way past it.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BenchRefusal {
    #[error(
        "the bench server for {} holds the feeder's session as owner key {owner}; {} is another \
         key, and gets no session beside it (stop `wellfed-bench serve` to use it)",
        root.display(),
        keys.display()
    )]
    OtherKey {
        root: PathBuf,
        keys: PathBuf,
        owner: String,
    },
    #[error(
        "the bench server for {} holds the feeder's session; --address would open another \
         (stop `wellfed-bench serve` to talk to the feeder directly)",
        root.display()
    )]
    Address { root: PathBuf },
    #[error(
        "`wellfed {command}` opens sessions of its own, and the bench server for {} holds the \
         feeder's; stop `wellfed-bench serve` first",
        root.display()
    )]
    OwnSessions {
        command: &'static str,
        root: PathBuf,
    },
    #[error(
        "the bench server for {} holds a session with feeder {served}, not {asked}",
        root.display()
    )]
    OtherFeeder {
        root: PathBuf,
        served: String,
        asked: String,
    },
    #[error(
        "the bench server for {} did not open its feeder session within {} s",
        root.display(),
        OPEN_WAIT.as_secs()
    )]
    NotOpen { root: PathBuf },
    /// The server's history starts after event `floor`: what came before
    /// is only on the feeder.
    #[error(
        "the bench server for {} holds the history after event {floor} only; pass --after \
         {floor}, or stop `wellfed-bench serve` to read further back",
        root.display()
    )]
    HistoryStarts { root: PathBuf, floor: u64 },
    #[error(
        "the bench server for {} holds the feeder's log after line {floor} only; pass --after \
         {floor}, or stop `wellfed-bench serve` to read further back",
        root.display()
    )]
    LogStarts { root: PathBuf, floor: u64 },
    /// The server's ring dropped `missed` messages this command had not
    /// read yet.
    #[error(
        "the bench server for {} moved past {missed} messages before they were read; read \
         again with --after the last one shown",
        root.display()
    )]
    FellBehind { root: PathBuf, missed: u64 },
    /// The pointer's process lives, but its socket does not answer: that
    /// server may still hold the feeder, so nothing goes direct.
    #[error(
        "bench server pid {pid} for {} is alive but not answering on {}; stop it, or wait for it",
        root.display(),
        socket.display()
    )]
    NotAnswering {
        pid: u32,
        root: PathBuf,
        socket: PathBuf,
    },
}

/// The bench server, once it answered.
pub(crate) struct Bench {
    socket: PathBuf,
    root: PathBuf,
}

impl Bench {
    /// The server a live pointer in `directories` names. A live server
    /// that does not answer is refused, never taken for no server.
    pub(crate) async fn find(directories: &[PathBuf]) -> Result<Option<Self>> {
        let Some(pointer) = rendezvous::live_in(directories) else {
            return Ok(None);
        };
        match UnixStream::connect(&pointer.socket).await {
            Ok(_) => Ok(Some(Self {
                socket: pointer.socket,
                root: pointer.root,
            })),
            Err(_) => Err(BenchRefusal::NotAnswering {
                pid: pointer.holder.pid,
                root: pointer.root,
                socket: pointer.socket,
            }
            .into()),
        }
    }

    /// Refuse `command`, which opens sessions of its own.
    pub(crate) fn refuse(self, command: &'static str) -> BenchRefusal {
        BenchRefusal::OwnSessions {
            command,
            root: self.root,
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
            StatusCode::SERVICE_UNAVAILABLE | StatusCode::CONFLICT => {
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

    async fn pushed(&self, stream: Stream, from: u64, wait: Duration) -> Result<PushedPage> {
        let stream = match stream {
            Stream::Events => "events",
            Stream::Logs => "logs",
        };
        let path = format!(
            "/device/pushed?stream={stream}&from={from}&wait={}",
            wait.as_millis()
        );
        self.call(Method::GET, &path, Vec::new()).await
    }
}

/// The public half of the key in the file at `keys`, hex; `None` when there
/// is no such file or it holds no key.
fn public_key(keys: &Path) -> Option<String> {
    let file: OwnerKeyFile = wellfed_formats::load(keys).ok()?;
    let secret = wellfed_formats::hex::decode::<32>(&file.secret)?;
    Some(wellfed_formats::hex::encode(
        Keypair::from_secret(secret).public(),
    ))
}

/// The server's session, lent for one command.
pub(crate) struct Lent {
    bench: Bench,
    identity: String,
    opened: Opened,
    events: Reading,
    logs: Reading,
}

/// One stream of the server's pushed messages, as this command reads it.
#[derive(Default)]
struct Reading {
    /// Wanted, after this number (an event's sequence, a log line's), once
    /// the command subscribed or asked for the tail.
    after: Option<Option<u64>>,
    /// The history's epoch `after` counts in.
    epoch: Option<u32>,
    /// The next message to read from the server's ring.
    cursor: u64,
    queued: VecDeque<Pushed>,
}

/// The server's session for `keys`, refusing whatever it cannot lend.
pub(crate) async fn lend(
    bench: Bench,
    keys: &Path,
    feeder: Option<&str>,
    address: Option<&str>,
) -> Result<Lent> {
    if address.is_some() {
        return Err(BenchRefusal::Address { root: bench.root }.into());
    }
    let mut info = bench.session().await?;
    // The key, not the file: a copy of the server's key speaks as it.
    if public_key(keys).as_deref() != Some(info.owner.as_str()) {
        return Err(BenchRefusal::OtherKey {
            root: bench.root,
            keys: keys.to_path_buf(),
            owner: info.owner[..16].to_string(),
        }
        .into());
    }
    if let Some(prefix) = feeder
        && !info.identity.starts_with(&prefix.to_ascii_lowercase())
    {
        return Err(BenchRefusal::OtherFeeder {
            root: bench.root,
            served: info.identity[..16].to_string(),
            asked: prefix.to_string(),
        }
        .into());
    }

    // An opening session is waited for, as the server waits a request.
    let deadline = Instant::now() + OPEN_WAIT;
    let opened = loop {
        if let Some(opened) = info.opened.take() {
            break opened;
        }
        if Instant::now() >= deadline {
            return Err(BenchRefusal::NotOpen { root: bench.root }.into());
        }
        tokio::time::sleep(OPEN_POLL).await;
        info = bench.session().await?;
    };
    Ok(Lent {
        bench,
        identity: info.identity,
        opened,
        events: Reading::default(),
        logs: Reading::default(),
    })
}

impl Lent {
    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    pub(crate) fn build(&self) -> &str {
        &self.opened.build
    }

    pub(crate) fn role(&self) -> ClientRole {
        self.opened.role
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.opened.capabilities
    }

    /// Events after `after`, from what the server's subscription kept. A
    /// history the server does not reach back to is refused here, before
    /// the command sends anything else: a feed must never go out ahead of
    /// a refusal of its own events.
    pub(crate) async fn subscribe(&mut self, after: Option<u64>, samples: bool) -> Result<()> {
        if samples {
            bail!("the bench server's session is subscribed without samples");
        }
        let page = self.bench.pushed(Stream::Events, 0, Duration::ZERO).await?;
        if let Some(floor) = page.floor
            && after.is_none_or(|after| after < floor)
        {
            return Err(BenchRefusal::HistoryStarts {
                root: self.bench.root.clone(),
                floor,
            }
            .into());
        }
        self.events = Reading {
            after: Some(after),
            epoch: page.epoch,
            cursor: page.first,
            queued: VecDeque::new(),
        };
        Ok(())
    }

    /// The next wanted message within `wait`; `None` when none came.
    pub(crate) async fn next(&mut self, wait: Duration) -> Result<Option<Pushed>> {
        let deadline = Instant::now() + wait;
        let stream = match (self.events.after, self.logs.after) {
            (Some(_), _) => Stream::Events,
            (None, Some(_)) => Stream::Logs,
            // Nothing asked for: nothing comes.
            (None, None) => {
                tokio::time::sleep(wait).await;
                return Ok(None);
            }
        };
        loop {
            let reading = match stream {
                Stream::Events => &mut self.events,
                Stream::Logs => &mut self.logs,
            };
            if let Some(message) = reading.queued.pop_front() {
                return Ok(Some(message));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let page = self.bench.pushed(stream, reading.cursor, left).await?;
            let reading = match stream {
                Stream::Events => &mut self.events,
                Stream::Logs => &mut self.logs,
            };
            // The ring dropped what this follower had not read: it stops,
            // rather than carry on past a hole it cannot see.
            if page.first > reading.cursor {
                return Err(BenchRefusal::FellBehind {
                    root: self.bench.root.clone(),
                    missed: page.first - reading.cursor,
                }
                .into());
            }
            // A new epoch (a store reset): its numbers are no comparison
            // for the old one's, so all of its history is new.
            if page.epoch != reading.epoch {
                reading.epoch = page.epoch;
                reading.after = Some(None);
            }
            reading.cursor = page.next;
            let (after, epoch) = (reading.after.flatten(), reading.epoch);
            let wanted = page
                .messages
                .into_iter()
                .filter(|message| wants(message, after, epoch));
            reading.queued.extend(wanted);
            if reading.queued.is_empty() && Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    /// One request, sent by the server under a fresh id. A tail is the
    /// server's own already: its lines are replayed, and the feeder's log
    /// cursor stays where the server's session left it.
    pub(crate) async fn ask(&mut self, op: &Op) -> Result<Answer> {
        if let Op::TailLog { after } = op {
            let after = after.map(u64::from);
            let page = self.bench.pushed(Stream::Logs, 0, Duration::ZERO).await?;
            if let Some(floor) = page.floor
                && after.is_none_or(|after| after < floor)
            {
                return Err(BenchRefusal::LogStarts {
                    root: self.bench.root.clone(),
                    floor,
                }
                .into());
            }
            self.logs = Reading {
                after: Some(after),
                epoch: None,
                cursor: page.first,
                queued: VecDeque::new(),
            };
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

/// Whether a message is past `after`, which counts in `epoch`: events and
/// gaps by sequence, log batches by their last line. An event of another
/// epoch is never compared with it, and is always new.
fn wants(message: &Pushed, after: Option<u64>, epoch: Option<u32>) -> bool {
    let last = match message {
        Pushed::Event(event) if Some(event.epoch) != epoch => return true,
        Pushed::Gap(gap) if Some(gap.epoch) != epoch => return true,
        Pushed::Event(event) => event.seq,
        Pushed::Gap(gap) => gap.to,
        Pushed::Log(batch) => u64::from(batch.seq),
        Pushed::Sample(_) => return false,
    };
    after.is_none_or(|after| last > after)
}

/// The CLI's side of the device routes, against a stand-in that serves
/// them from a session of its own with the simulator: the CLI never links
/// the bench, whose relay driver only the bench may. The server's side of
/// the same routes is `wellfed-bench-server`'s tests.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Json, Response};
    use axum::routing::{get, post};
    use serde::Deserialize;
    use tokio::sync::Mutex;
    use wellfed_feeder_sim::harness::Threaded;
    use wellfed_formats::bench::lease::Holder;
    use wellfed_formats::bench::live::{Pause, Unavailable};
    use wellfed_formats::bench::rendezvous::Rendezvous;
    use wellfed_model::{Event, EventBody};

    use super::*;
    use crate::device::Connection;
    use crate::{Target, connect_via};

    const OWNER: [u8; 32] = [0x22; 32];
    const MEMBER: [u8; 32] = [0x33; 32];

    /// What the stand-in's ring holds: its first number, floor and epoch,
    /// and its messages.
    #[derive(Clone, Default)]
    struct Ring {
        first: u64,
        floor: Option<u64>,
        epoch: Option<u32>,
        messages: Vec<Pushed>,
    }

    struct Stand {
        connection: Mutex<wellfed_connect::Connection>,
        info: SessionInfo,
        paused: bool,
        ring: std::sync::Mutex<Ring>,
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

    #[derive(Deserialize)]
    struct From {
        from: u64,
    }

    /// The ring from `from`, as the server pages it.
    async fn pushed(State(stand): State<Arc<Stand>>, Query(at): Query<From>) -> Json<PushedPage> {
        let ring = stand.ring.lock().unwrap().clone();
        let next = ring.first + ring.messages.len() as u64;
        let first = at.from.max(ring.first);
        let skip = usize::try_from(first - ring.first).unwrap();
        Json(PushedPage {
            first,
            next,
            floor: ring.floor,
            epoch: ring.epoch,
            messages: ring.messages.into_iter().skip(skip).collect(),
        })
    }

    fn key_file(path: &Path, secret: [u8; 32], feeder: &Threaded) {
        let file = serde_json::json!({
            "version": 1,
            "secret": wellfed_formats::hex::encode(&secret),
            "label": "bench",
            "feeders": [{ "identity": wellfed_formats::hex::encode(&feeder.identity) }],
        });
        fs::write(path, file.to_string()).unwrap();
    }

    /// A checkout holding the owner's key file, a stand-in server on its
    /// `private/bench.sock`, and its pointer, written where a server's
    /// environment would (XDG and the state directory).
    struct Standing {
        _dir: tempfile::TempDir,
        root: PathBuf,
        /// XDG's, then the state directory.
        pointers: Vec<PathBuf>,
        keys: PathBuf,
        stand: Arc<Stand>,
    }

    async fn stand_in(feeder: &Threaded, paused: bool, ring: Ring) -> Standing {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("checkout");
        fs::create_dir_all(root.join("private")).unwrap();
        let keys = root.join("private/wellfed-owner.json");
        key_file(&keys, OWNER, feeder);

        let keypair = Keypair::from_secret(OWNER);
        let connection =
            wellfed_connect::Connection::open(&keypair, feeder.identity, feeder.address, [9; 32])
                .await
                .unwrap();
        let info = SessionInfo {
            root: root.clone(),
            keys: keys.clone(),
            owner: wellfed_formats::hex::encode(keypair.public()),
            identity: wellfed_formats::hex::encode(&feeder.identity),
            opened: Some(Opened {
                build: connection.build().to_string(),
                role: connection.role(),
                capabilities: connection.capabilities(),
            }),
        };
        let stand = Arc::new(Stand {
            connection: Mutex::new(connection),
            info,
            paused,
            ring: std::sync::Mutex::new(ring),
        });
        let router = axum::Router::new()
            .route("/device/session", get(session))
            .route("/device/ask", post(ask))
            .route("/device/pushed", get(pushed))
            .with_state(Arc::clone(&stand));
        let socket = root.join("private/bench.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await });

        let pointers = vec![dir.path().join("run"), dir.path().join("state")];
        for directory in &pointers {
            rendezvous::publish(
                directory,
                &Rendezvous {
                    socket: socket.clone(),
                    root: root.clone(),
                    holder: Holder::current().unwrap(),
                },
            )
            .unwrap();
        }
        Standing {
            _dir: dir,
            root,
            pointers,
            keys,
            stand,
        }
    }

    fn simulator() -> Threaded {
        Threaded::start(*Keypair::from_secret(OWNER).public())
    }

    fn target(keys: &Path) -> Target {
        Target {
            keys: keys.to_path_buf(),
            feeder: None,
            address: None,
        }
    }

    fn event(epoch: u32, seq: u64) -> Pushed {
        Pushed::Event(Event {
            epoch,
            seq,
            at: None,
            body: EventBody::Boot {
                bank: 1,
                reset_flags: 0,
            },
        })
    }

    fn seqs(messages: &[Pushed]) -> Vec<(u32, u64)> {
        messages
            .iter()
            .filter_map(|message| match message {
                Pushed::Event(event) => Some((event.epoch, event.seq)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_server_is_found_from_anywhere_and_lends_only_its_own_key() {
        let feeder = simulator();
        let standing = stand_in(&feeder, false, Ring::default()).await;
        // A client whose environment has no XDG looks in the state
        // directory only, and finds the server there.
        let pointers = &standing.pointers[1..];

        // A copy of the key, outside the checkout and any cwd's private/,
        // is the server's key: it goes through the server.
        let elsewhere = tempfile::tempdir().unwrap();
        let copy = elsewhere.path().join("owner.json");
        fs::copy(&standing.keys, &copy).unwrap();
        let (_, identity, connection) = connect_via(&target(&copy), pointers).await.unwrap();
        assert!(matches!(connection, Connection::Bench(_)));
        assert_eq!(identity, feeder.identity);

        // `wellfed status`'s request, through the server.
        let (_, _, mut connection) = connect_via(&target(&standing.keys), pointers)
            .await
            .unwrap();
        let answer = connection.ask(&Op::Status).await.unwrap();
        assert!(matches!(answer, Answer::Status(_)), "{answer:?}");

        // Another key, and --address, are refused, naming the server.
        let member = elsewhere.path().join("member.json");
        key_file(&member, MEMBER, &feeder);
        let refused = connect_via(&target(&member), pointers).await.err().unwrap();
        assert!(
            matches!(refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::OtherKey { root, .. }) if *root == standing.root),
            "{refused}"
        );
        let direct = Target {
            address: Some(feeder.address.to_string()),
            ..target(&standing.keys)
        };
        let refused = connect_via(&direct, pointers).await.err().unwrap();
        assert!(
            matches!(
                refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::Address { .. })
            ),
            "{refused}"
        );

        // So are the commands that open sessions of their own.
        let refused = crate::refuse_beside_bench("install", pointers)
            .await
            .err()
            .unwrap();
        assert!(
            matches!(
                refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::OwnSessions {
                    command: "install",
                    ..
                })
            ),
            "{refused}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dead_server_is_no_server_and_a_live_one_that_does_not_answer_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let pointers = [dir.path().to_path_buf()];
        let mut holder = Holder::current().unwrap();
        holder.start_ticks += 1;
        let pointer = Rendezvous {
            socket: dir.path().join("bench.sock"),
            root: dir.path().to_path_buf(),
            holder,
        };
        rendezvous::publish(dir.path(), &pointer).unwrap();
        assert!(Bench::find(&pointers).await.unwrap().is_none());
        crate::refuse_beside_bench("setup", &pointers)
            .await
            .unwrap();

        // A live process whose socket refuses may still hold the feeder:
        // nothing goes direct beside it.
        let pointer = Rendezvous {
            holder: Holder::current().unwrap(),
            ..pointer
        };
        rendezvous::publish(dir.path(), &pointer).unwrap();
        let refused = Bench::find(&pointers).await.err().unwrap();
        assert!(
            matches!(refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::NotAnswering { pid, .. }) if *pid == std::process::id()),
            "{refused}"
        );
        assert!(
            crate::refuse_beside_bench("setup", &pointers)
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_paused_server_refuses_with_its_reason_and_nothing_goes_direct() {
        let feeder = simulator();
        let standing = stand_in(&feeder, true, Ring::default()).await;

        let refused = connect_via(&target(&standing.keys), &standing.pointers)
            .await
            .err()
            .unwrap();

        let reason = refused.downcast_ref::<DeviceUnavailable>().unwrap();
        assert_eq!(
            reason.reason,
            Unavailable::Paused {
                pause: Pause::PowerOff
            }
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_history_the_server_does_not_reach_back_to_is_refused_before_anything_is_sent() {
        let feeder = simulator();
        let ring = Ring {
            floor: Some(40),
            epoch: Some(2),
            ..Ring::default()
        };
        let standing = stand_in(&feeder, false, ring).await;
        let (_, _, mut connection) = connect_via(&target(&standing.keys), &standing.pointers)
            .await
            .unwrap();

        // Refused at the subscription: a feed sent after it never runs
        // ahead of a refusal of its own events.
        let refused = connection.subscribe(None, false).await.err().unwrap();
        assert!(
            matches!(
                refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::HistoryStarts { floor: 40, .. })
            ),
            "{refused}"
        );
        let refused = connection
            .ask(&Op::TailLog { after: None })
            .await
            .map(|_| ());
        assert!(refused.is_err(), "the log shares the ring's floor here");

        // From the floor on, the server's history is whole.
        connection.subscribe(Some(40), false).await.unwrap();
        assert!(connection.next(Duration::ZERO).await.unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn after_a_new_epoch_the_history_counts_in_it_alone() {
        let feeder = simulator();
        // The server's ring is the new epoch's, after a store reset; the
        // command asks after event 7 of it, as a feed asks after the
        // status's newest event.
        let ring = Ring {
            first: 50,
            floor: None,
            epoch: Some(2),
            messages: vec![event(2, 7), event(2, 8)],
        };
        let standing = stand_in(&feeder, false, ring).await;
        let (_, _, mut connection) = connect_via(&target(&standing.keys), &standing.pointers)
            .await
            .unwrap();
        connection.subscribe(Some(7), false).await.unwrap();

        let next = connection.next(Duration::ZERO).await.unwrap().unwrap();
        assert_eq!(seqs(&[next]), [(2, 8)]);

        // A further reset mid-follow: the newer epoch's events all count,
        // whatever their numbers.
        *standing.stand.ring.lock().unwrap() = Ring {
            first: 52,
            floor: None,
            epoch: Some(3),
            messages: vec![event(3, 1), event(3, 2)],
        };
        let mut seen = Vec::new();
        while let Some(message) = connection.next(Duration::ZERO).await.unwrap() {
            seen.push(message);
        }
        assert_eq!(seqs(&seen), [(3, 1), (3, 2)]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_follower_the_ring_outran_stops_rather_than_skip_ahead() {
        let feeder = simulator();
        let ring = Ring {
            first: 0,
            floor: None,
            epoch: Some(1),
            messages: vec![event(1, 1)],
        };
        let standing = stand_in(&feeder, false, ring).await;
        let (_, _, mut connection) = connect_via(&target(&standing.keys), &standing.pointers)
            .await
            .unwrap();
        connection.subscribe(None, false).await.unwrap();
        assert!(connection.next(Duration::ZERO).await.unwrap().is_some());

        // The ring drops what was pushed after it before this follower
        // read it: its next page starts past its place.
        *standing.stand.ring.lock().unwrap() = Ring {
            first: 5,
            floor: Some(5),
            epoch: Some(1),
            messages: vec![event(1, 6)],
        };
        let refused = connection.next(Duration::ZERO).await.err().unwrap();
        assert!(
            matches!(
                refused.downcast_ref::<BenchRefusal>(),
                Some(BenchRefusal::FellBehind { missed: 4, .. })
            ),
            "{refused}"
        );
    }
}
