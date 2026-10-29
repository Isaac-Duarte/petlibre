//! The one owner session with the feeder, held for the server's life.
//!
//! A tokio task owns exactly one `wellfed_connect::Connection`. It:
//! - subscribes after the event it last took ([`DEVICE_CURSOR`]), and keeps
//!   the pushed history and the debug log in two rings, each to its own
//!   bound, that `GET /device/pushed` serves;
//! - follows the feeder's debug log into the state's tail and into
//!   [`DEVICE_LOG`], through a buffer that is never synced;
//! - reads the status every 10 s, and sets the clock as `wellfed time`
//!   does (this host's zone) when it is invalid or more than 2 s off;
//! - carries the requests of the `wellfed` CLI (`POST /device/ask`) and of
//!   the jobs ([`SessionLink`]). A read that got no answer goes once more,
//!   under the same id; anything else is sent once, and one that got no
//!   answer is reported uncertain. Each waits as long as the session gives
//!   its op (`wellfed_client::connection::Timing`: 75 s for an update key's
//!   Dispense press), and nothing on the socket gives up sooner.
//!
//! It closes while a job holds the UART, while an install push or
//! `stock-ota` holds sessions of their own, while relay 1 is off and while
//! the server stops, and otherwise reopens with a backoff. A request meets
//! a closed session with [`Unavailable`], unless it is only opening, when
//! it waits for it a while. One subscription per process: a second
//! subscriber on the same feeder starves.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use wellfed_connect::{Connection, Cursor, Received, Resume};
use wellfed_formats::bench::banks::Witness;
use wellfed_formats::bench::feeder::{Asked, Opened, PushedPage, SessionInfo, Stream};
use wellfed_formats::bench::live::{
    BenchState, ClockView, DEVICE_CURSOR, DEVICE_LOG, DeviceCursor, FeederStatus, Journal,
    JournalSource, Pause, Session, Unavailable, trim_tail,
};
use wellfed_formats::bench::notes::Seen;
use wellfed_formats::bench::now;
use wellfed_formats::hex;
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_model::{Answer, Op, Pushed, Status};
use wellfed_rig::device::{self, DeviceError, DeviceLink, Owner};

use crate::banks::Banks;
use crate::files;
use crate::notes::{Notes, event_name};
use crate::state::Store;

/// How often the status is read while the session is open.
const STATUS_EVERY: Duration = Duration::from_secs(10);
/// A clock further off than this is set again.
const SKEW_MAX_MS: i64 = 2_000;
/// After the feeder refuses a sync, the next waits this long.
const SYNC_REFUSED_WAIT: Duration = Duration::from_secs(5 * 60);
/// The longest a request waits for a session that is opening.
pub(crate) const OPEN_WAIT: Duration = Duration::from_secs(20);
/// Reconnection backoff: doubling from the first, up to the last.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How many events and gaps, and how many log batches, the rings keep.
const KEPT_EVENTS: usize = 4096;
const KEPT_LOGS: usize = 1024;
/// The longest `GET /device/pushed` holds a reader for news.
pub(crate) const PUSHED_WAIT_MAX: Duration = Duration::from_secs(30);

/// Whether the session sets the feeder's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClockSync {
    #[default]
    On,
    /// Leave it as it is (`serve --no-clock-sync`), so a check can watch a
    /// feeder with no clock.
    Off,
}

/// Where the session's key and feeder come from.
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    /// Root-relative: the owner key file (`wellfed setup` writes it).
    pub keys: PathBuf,
    /// Root-relative: the server config whose hotspot fields find the
    /// feeder.
    pub config: PathBuf,
    /// The feeder's address, given rather than found (the simulator).
    pub address: Option<SocketAddr>,
    pub clock: ClockSync,
    /// The directories the server's pointer goes in, for the `wellfed` CLI
    /// to find it from anywhere (`wellfed_formats::bench::rendezvous`).
    /// None publishes none: tests, which must not reach the user's.
    pub rendezvous: Vec<PathBuf>,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            keys: device::OWNER_KEYS.into(),
            config: wellfed_formats::stock::SERVER_CONFIG.into(),
            address: None,
            clock: ClockSync::On,
            rendezvous: Vec::new(),
        }
    }
}

/// Why a session did not open, or ended.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SessionError {
    #[error(transparent)]
    Locate(#[from] DeviceError),
    #[error("the feeder lookup: {0}")]
    Lookup(#[from] tokio::task::JoinError),
    #[error("the system random source")]
    Random,
    #[error(transparent)]
    Connect(#[from] wellfed_connect::Error),
}

/// One request a caller hands the task.
struct Request {
    id: [u8; 16],
    op: Op,
    reply: oneshot::Sender<Result<Asked, Unavailable>>,
    /// Sent once already, and lost: a read goes again only once.
    resent: bool,
}

/// The handle every part of the server asks through.
#[derive(Clone)]
pub(crate) struct Feeder {
    requests: mpsc::UnboundedSender<Request>,
    /// Set while a job holds the UART or its own sessions, and once the
    /// server stops.
    hold: Arc<watch::Sender<Option<Pause>>>,
    events: Arc<Pushes>,
    logs: Arc<Pushes>,
    /// Whom the session speaks as and with; `None` when unconfigured.
    info: Arc<Mutex<Option<SessionInfo>>>,
    /// Root-relative, as the jobs name it.
    keys: PathBuf,
    /// The live state, where the session says whether it is open.
    store: Store,
}

/// What [`Feeder::new`] leaves for [`Task::run`] to start.
pub(crate) struct Task {
    owner: Result<Owner, wellfed_rig::Error>,
    address: Option<SocketAddr>,
    clock: ClockSync,
    root: PathBuf,
    store: Store,
    banks: Banks,
    notes: Notes,
    requests: mpsc::UnboundedReceiver<Request>,
    hold: watch::Receiver<Option<Pause>>,
    events: Arc<Pushes>,
    logs: Arc<Pushes>,
    info: Arc<Mutex<Option<SessionInfo>>>,
}

impl Feeder {
    /// The handle, and the task that serves it once the runtime runs. A
    /// key file or config that does not load leaves the session
    /// unconfigured: the server still runs its jobs.
    pub(crate) fn new(
        root: &Path,
        options: &DeviceOptions,
        store: Store,
        banks: Banks,
        notes: Notes,
    ) -> (Self, Task) {
        let owner = Owner::open(root, &options.keys, &options.config);
        let keys = root.join(&options.keys);
        let keys = keys.canonicalize().unwrap_or(keys);
        let info = owner.as_ref().ok().map(|owner| SessionInfo {
            root: root.to_path_buf(),
            keys,
            owner: hex::encode(owner.keys().public()),
            identity: hex::encode(&owner.identity()),
            opened: None,
        });
        let (requests, receiver) = mpsc::unbounded_channel();
        let (hold, held) = watch::channel(None);
        let events = Arc::new(Pushes::new(KEPT_EVENTS));
        let logs = Arc::new(Pushes::new(KEPT_LOGS));
        let info = Arc::new(Mutex::new(info));

        let feeder = Self {
            requests,
            hold: Arc::new(hold),
            events: Arc::clone(&events),
            logs: Arc::clone(&logs),
            info: Arc::clone(&info),
            keys: options.keys.clone(),
            store: store.clone(),
        };
        let task = Task {
            owner,
            address: options.address,
            clock: options.clock,
            root: root.to_path_buf(),
            store,
            banks,
            notes,
            requests: receiver,
            hold: held,
            events,
            logs,
            info,
        };
        (feeder, task)
    }

    /// Close the session for `pause`, or let it open again with `None`.
    pub(crate) fn hold(&self, pause: Option<Pause>) {
        self.hold.send_replace(pause);
    }

    /// Send `op` under `id` and wait for what came of it.
    pub(crate) async fn ask(&self, id: [u8; 16], op: Op) -> Result<Asked, Unavailable> {
        let (reply, answer) = oneshot::channel();
        let request = Request {
            id,
            op,
            reply,
            resent: false,
        };
        self.requests
            .send(request)
            .map_err(|_| Unavailable::Ended)?;
        answer.await.unwrap_or(Err(Unavailable::Ended))
    }

    pub(crate) fn info(&self) -> Option<SessionInfo> {
        lock(&self.info).clone()
    }

    pub(crate) fn pushed(&self, stream: Stream) -> &Pushes {
        match stream {
            Stream::Events => &self.events,
            Stream::Logs => &self.logs,
        }
    }

    /// The link a job's sequence asks through.
    pub(crate) fn link(&self) -> (PathBuf, SessionLink) {
        let link = SessionLink {
            requests: self.requests.clone(),
            hold: Arc::clone(&self.hold),
            store: self.store.clone(),
        };
        (self.keys.clone(), link)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn random<const N: usize>() -> Option<[u8; N]> {
    private::random(Path::new(SYSTEM_RANDOM)).ok()
}

/// The server's session lent to a job: a blocking request over the task's
/// channel, from the job's own thread.
pub(crate) struct SessionLink {
    requests: mpsc::UnboundedSender<Request>,
    hold: Arc<watch::Sender<Option<Pause>>>,
    /// Where the session says it has closed.
    store: Store,
}

impl DeviceLink for SessionLink {
    fn ask(&mut self, ask: &device::Ask) -> Result<device::Answer, DeviceError> {
        let id = private::random(Path::new(SYSTEM_RANDOM)).map_err(DeviceError::Random)?;
        let (reply, answer) = oneshot::channel();
        let request = Request {
            id,
            // This host's zone, as the server's own sync and `wellfed time`
            // send it, so no two syncs on the bench disagree.
            op: ask.op(&jiff::tz::TimeZone::system()),
            reply,
            resent: false,
        };
        if self.requests.send(request).is_err() {
            return Err(Unavailable::Ended.into());
        }
        match answer.blocking_recv() {
            Ok(Ok(Asked::Answered { answer })) => Ok(device::Answer::of(answer)),
            Ok(Ok(Asked::Uncertain)) => Err(DeviceError::Uncertain(ask.name())),
            Ok(Err(unavailable)) => Err(unavailable.into()),
            Err(_) => Err(Unavailable::Ended.into()),
        }
    }

    /// The push's sessions are the feeder's owner sessions until it ends;
    /// the server's own closes meanwhile and resumes after its cursor. It
    /// returns once the server's session has closed, or after
    /// [`PUSH_CLOSE_WAIT`] (a session mid-request takes a moment), so the
    /// push never opens beside it.
    fn hold_for_push(&mut self) -> Option<Box<dyn FnOnce() + Send>> {
        self.hold.send_if_modified(|hold| {
            let free = hold.is_none();
            if free {
                *hold = Some(Pause::Install);
            }
            free
        });
        // The job's own thread, outside the runtime: the standard clock.
        let deadline = std::time::Instant::now() + PUSH_CLOSE_WAIT;
        while !self.store.with(|state| closed(&state.feeder.session))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let hold = Arc::clone(&self.hold);
        Some(Box::new(move || {
            // A stop that came meanwhile stays.
            hold.send_if_modified(|hold| {
                let ours = *hold == Some(Pause::Install);
                if ours {
                    *hold = None;
                }
                ours
            });
        }))
    }
}

/// The longest a push waits for the server's session to close.
const PUSH_CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Whether no session is open or opening.
fn closed(session: &Session) -> bool {
    matches!(
        session,
        Session::Paused { .. } | Session::Unconfigured { .. } | Session::Idle
    )
}

/// A stream of pushed messages, numbered from the server's start, the
/// newest `bound` kept.
pub(crate) struct Pushes {
    inner: Mutex<Ring>,
    bound: usize,
    /// The number the next message takes.
    next: watch::Sender<u64>,
}

#[derive(Default)]
struct Ring {
    first: u64,
    messages: VecDeque<Pushed>,
    /// Every event (or log line) after this one is in the ring, or was
    /// pushed since.
    floor: Option<u64>,
    /// The epoch every event and gap here counts in.
    epoch: Option<u32>,
}

impl Pushes {
    fn new(bound: usize) -> Self {
        Self {
            inner: Mutex::default(),
            bound,
            next: watch::Sender::new(0),
        }
    }

    /// Start again, empty, holding `epoch`'s history after event `floor`
    /// (`None`: from the feeder's oldest). Numbering carries on, so a
    /// reader's place never points into the new history by mistake.
    /// Every reader waiting on the old history wakes, to read the new
    /// epoch and floor rather than wait out its time on a number that did
    /// not move.
    fn reset(&self, epoch: Option<u32>, floor: Option<u64>) {
        {
            let mut ring = lock(&self.inner);
            ring.first += ring.messages.len() as u64;
            ring.messages.clear();
            ring.floor = floor;
            ring.epoch = epoch;
        }
        self.next.send_modify(|_| {});
    }

    fn epoch(&self) -> Option<u32> {
        lock(&self.inner).epoch
    }

    fn push(&self, message: Pushed) {
        let next = {
            let mut ring = lock(&self.inner);
            ring.messages.push_back(message);
            if ring.messages.len() > self.bound {
                let dropped = ring.messages.pop_front();
                ring.first += 1;
                let gone = match dropped {
                    Some(Pushed::Event(event)) => Some(event.seq),
                    Some(Pushed::Gap(gap)) => Some(gap.to),
                    Some(Pushed::Log(batch)) => Some(u64::from(batch.seq)),
                    Some(Pushed::Sample(_)) | None => None,
                };
                if let Some(gone) = gone {
                    ring.floor = Some(ring.floor.map_or(gone, |floor| floor.max(gone)));
                }
            }
            ring.first + ring.messages.len() as u64
        };
        self.next.send_replace(next);
    }

    /// The messages from `from` on, waiting up to `wait` for the first, or
    /// for a reset, which answers at once with the new history's epoch.
    pub(crate) async fn page(&self, from: u64, wait: Duration) -> PushedPage {
        let mut next = self.next.subscribe();
        let deadline = Instant::now() + wait.min(PUSHED_WAIT_MAX);
        if *next.borrow_and_update() <= from {
            let _ = tokio::time::timeout_at(deadline, next.changed()).await;
        }

        let ring = lock(&self.inner);
        let first = from.max(ring.first);
        let skip = usize::try_from(first - ring.first).unwrap_or(usize::MAX);
        PushedPage {
            first,
            next: ring.first + ring.messages.len() as u64,
            floor: ring.floor,
            epoch: ring.epoch,
            messages: ring.messages.iter().skip(skip).cloned().collect(),
        }
    }
}

/// What a request the task sent itself is for.
enum Purpose {
    Caller(Request),
    Status,
    Sync,
    Tail,
}

/// Why a session ended.
enum Closed {
    Paused,
    Lost(SessionError),
    /// Every handle is gone: the server has quit.
    Ended,
}

/// Whether `op` only reads: one of these sent twice changes nothing, so a
/// read lost with its session goes once more.
fn read_only(op: &Op) -> bool {
    matches!(
        op,
        Op::Status
            | Op::GetSettings
            | Op::ListClients
            | Op::UpdateState
            | Op::GetTime
            | Op::Stats { .. }
            | Op::FeedTrace { .. }
            | Op::GetCalibration
            | Op::ReadBank1 { .. }
            | Op::GetPlans
            | Op::ReadStockPlans
            | Op::GetCall
    )
}

impl Task {
    /// Serve the session until every handle is dropped.
    pub(crate) async fn run(mut self) {
        let owner = match &self.owner {
            Ok(owner) => owner.clone(),
            Err(error) => {
                let error = error.to_string();
                return self.unconfigured(error).await;
            }
        };
        let mut states = self.store.subscribe();
        let mut log = DeviceLog::new(&self.root);
        let mut cursor: Option<Cursor> =
            wellfed_formats::load::<DeviceCursor>(&self.root.join(DEVICE_CURSOR))
                .ok()
                .map(|cursor| Cursor {
                    epoch: cursor.epoch,
                    seq: cursor.seq,
                });
        let mut waiting: Vec<(Request, Instant)> = Vec::new();
        let mut attempt = 0u32;
        let mut log_after: Option<(u32, u32)> = None;
        let mut sync_refused: Option<Instant> = None;

        loop {
            if let Some(pause) = paused(&self.hold, &states) {
                for (request, _) in waiting.drain(..) {
                    let _ = request.reply.send(Err(Unavailable::Paused {
                        pause: pause.clone(),
                    }));
                }
                attempt = 0;
                self.set_session(Session::Paused {
                    pause: pause.clone(),
                });
                if pause == Pause::Stopping || !self.wait_paused(pause, &mut states).await {
                    break;
                }
                continue;
            }

            let opened = self.open(&owner, &mut cursor).await;
            let (mut connection, address) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    attempt += 1;
                    let wait = backoff(attempt);
                    let error = error.to_string();
                    self.set_session(Session::Connecting {
                        attempt,
                        error: Some(error.clone()),
                        retry: Some(now() + wait.as_secs_f64()),
                    });
                    if !self
                        .wait_retry(wait, &mut states, &mut waiting, Some(&error))
                        .await
                    {
                        break;
                    }
                    continue;
                }
            };
            attempt = 0;
            self.set_session(Session::Open {
                address: address.to_string(),
                build: connection.build().to_string(),
                since: now(),
            });

            let mut session = Open {
                connection: &mut connection,
                pending: HashMap::new(),
                cursor: &mut cursor,
                log: &mut log,
                log_after: &mut log_after,
                sync_refused: &mut sync_refused,
                tailing: false,
                clock: self.clock,
                store: self.store.clone(),
                banks: self.banks.clone(),
                notes: self.notes.clone(),
                root: self.root.clone(),
                events: Arc::clone(&self.events),
                logs: Arc::clone(&self.logs),
            };
            let closed = session
                .serve(&mut self.requests, &self.hold, &mut states, &mut waiting)
                .await;
            log.flush();
            if let Some(info) = lock(&self.info).as_mut() {
                info.opened = None;
            }
            match closed {
                Closed::Paused => {}
                Closed::Lost(error) => {
                    attempt = 1;
                    let error = error.to_string();
                    self.set_session(Session::Connecting {
                        attempt,
                        error: Some(error.clone()),
                        retry: Some(now() + BACKOFF_FIRST.as_secs_f64()),
                    });
                    if !self
                        .wait_retry(BACKOFF_FIRST, &mut states, &mut waiting, Some(&error))
                        .await
                    {
                        break;
                    }
                }
                Closed::Ended => break,
            }
        }
        log.flush();
    }

    /// No session can open: answer every request so, until the server quits.
    async fn unconfigured(mut self, error: String) {
        // No session is open to close, but a job that runs sessions of its
        // own (an install that creates the key) is shown its hold, as it
        // would be beside an open one.
        let shown = |held: Option<Pause>| match held {
            Some(pause) => Session::Paused { pause },
            None => Session::Unconfigured {
                error: error.clone(),
            },
        };
        self.set_session(shown(self.hold.borrow().clone()));
        loop {
            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { return };
                    let _ = request.reply.send(Err(Unavailable::Unconfigured {
                        error: error.clone(),
                    }));
                }
                changed = self.hold.changed() => {
                    let held = self.hold.borrow().clone();
                    if changed.is_err() || held == Some(Pause::Stopping) {
                        return;
                    }
                    self.set_session(shown(held));
                }
            }
        }
    }

    fn set_session(&self, session: Session) {
        self.store.update(|state| state.feeder.session = session);
    }

    /// Find the feeder, open a session and subscribe after `cursor`.
    async fn open(
        &self,
        owner: &Owner,
        cursor: &mut Option<Cursor>,
    ) -> Result<(Connection, SocketAddr), SessionError> {
        let address = match self.address {
            Some(address) => address,
            None => {
                let finding = owner.clone();
                tokio::task::spawn_blocking(move || finding.locate()).await??
            }
        };
        let ephemeral = random().ok_or(SessionError::Random)?;
        let mut connection =
            Connection::open(owner.keys(), owner.identity(), address, ephemeral).await?;
        let epoch = connection.epoch();
        if cursor.is_some_and(|cursor| cursor.epoch != epoch) {
            *cursor = None;
        }
        let resume = connection.subscribe(*cursor, false).await?;

        // The held history is one epoch's. A new epoch (a store reset)
        // starts it again, empty, as does the first subscription; a
        // reconnect in the same epoch carries on from the same cursor.
        if self.events.epoch() != Some(epoch) || resume == Resume::Restarted {
            let floor = match resume {
                Resume::Continued => cursor.map(|cursor| cursor.seq),
                Resume::Restarted => None,
            };
            self.events.reset(Some(epoch), floor);
        }
        if let Some(info) = lock(&self.info).as_mut() {
            info.opened = Some(Opened {
                build: connection.build().to_string(),
                role: connection.role(),
                capabilities: connection.capabilities(),
            });
        }
        Ok((connection, address))
    }

    /// Answer requests as paused until the pause ends; `false` once every
    /// handle is gone.
    async fn wait_paused(
        &mut self,
        pause: Pause,
        states: &mut watch::Receiver<BenchState>,
    ) -> bool {
        loop {
            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { return false };
                    let _ = request.reply.send(Err(Unavailable::Paused { pause: pause.clone() }));
                }
                changed = self.hold.changed() => if changed.is_err() { return false },
                _ = states.changed() => {}
            }
            if paused(&self.hold, states).as_ref() != Some(&pause) {
                return true;
            }
        }
    }

    /// Wait `wait` before the next try. A request arriving meanwhile waits
    /// with the others and cuts the wait short, so a caller is never held
    /// behind the backoff; one that waited [`OPEN_WAIT`] is told the
    /// session is not open. `false` once every handle is gone.
    async fn wait_retry(
        &mut self,
        wait: Duration,
        states: &mut watch::Receiver<BenchState>,
        waiting: &mut Vec<(Request, Instant)>,
        error: Option<&str>,
    ) -> bool {
        let until = Instant::now() + wait;
        loop {
            expire(waiting, error);
            let next_expiry = waiting.iter().map(|(_, at)| *at).min();
            let wake = next_expiry.map_or(until, |at| at.min(until));
            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { return false };
                    if waiting.iter().any(|(other, _)| other.id == request.id) {
                        refuse_duplicate(request);
                        continue;
                    }
                    waiting.push((request, Instant::now() + OPEN_WAIT));
                    return true;
                }
                changed = self.hold.changed() => {
                    if changed.is_err() { return false }
                    return true;
                }
                _ = states.changed() => {
                    if paused(&self.hold, states).is_some() { return true }
                }
                () = tokio::time::sleep_until(wake) => {
                    if Instant::now() >= until { return true }
                }
            }
        }
    }
}

fn refuse_duplicate(request: Request) {
    let _ = request.reply.send(Err(Unavailable::DuplicateId {
        id: hex::encode(&request.id),
    }));
}

/// Tell requests that waited too long for a session that it is not open.
fn expire(waiting: &mut Vec<(Request, Instant)>, error: Option<&str>) {
    let now = Instant::now();
    let mut kept = Vec::new();
    for (request, at) in waiting.drain(..) {
        if at <= now {
            let _ = request.reply.send(Err(Unavailable::NotOpen {
                error: error.map(str::to_string),
            }));
        } else {
            kept.push((request, at));
        }
    }
    *waiting = kept;
}

fn backoff(attempt: u32) -> Duration {
    let doubled = BACKOFF_FIRST.saturating_mul(1 << attempt.saturating_sub(1).min(5));
    doubled.min(BACKOFF_MAX)
}

/// Why the session must stay closed now: the server's hold, or the
/// feeder's supply relay read off. A relay not read yet is no reason.
fn paused(
    hold: &watch::Receiver<Option<Pause>>,
    states: &watch::Receiver<BenchState>,
) -> Option<Pause> {
    if let Some(pause) = hold.borrow().clone() {
        return Some(pause);
    }
    let off = states
        .borrow()
        .relays
        .get(&wellfed_relay::POWER.to_string())
        .is_some_and(|relay| !relay.on);
    off.then_some(Pause::PowerOff)
}

/// One open session and what it is waiting on.
struct Open<'a> {
    connection: &'a mut Connection,
    pending: HashMap<[u8; 16], Purpose>,
    cursor: &'a mut Option<Cursor>,
    log: &'a mut DeviceLog,
    /// The boot count and line number the log was last read to.
    log_after: &'a mut Option<(u32, u32)>,
    /// When the feeder last refused a sync.
    sync_refused: &'a mut Option<Instant>,
    /// Whether this session asked for the log yet.
    tailing: bool,
    clock: ClockSync,
    store: Store,
    banks: Banks,
    notes: Notes,
    root: PathBuf,
    events: Arc<Pushes>,
    logs: Arc<Pushes>,
}

impl Open<'_> {
    async fn serve(
        &mut self,
        requests: &mut mpsc::UnboundedReceiver<Request>,
        hold: &watch::Receiver<Option<Pause>>,
        states: &mut watch::Receiver<BenchState>,
        waiting: &mut Vec<(Request, Instant)>,
    ) -> Closed {
        let mut hold = hold.clone();
        for (request, _) in waiting.drain(..) {
            self.send(request);
        }
        self.request(Purpose::Status, &Op::Status);
        let mut ticks = tokio::time::interval_at(Instant::now() + STATUS_EVERY, STATUS_EVERY);

        let closed = loop {
            if paused(&hold, states).is_some() {
                break Closed::Paused;
            }
            tokio::select! {
                request = requests.recv() => match request {
                    // A hold may have come since the check above: nothing
                    // goes out under it.
                    Some(request) => match paused(&hold, states) {
                        Some(pause) => {
                            let _ = request.reply.send(Err(Unavailable::Paused { pause }));
                        }
                        None => self.send(request),
                    },
                    None => break Closed::Ended,
                },
                received = self.connection.receive(STATUS_EVERY) => match received {
                    Ok(Received::Answer { id, answer }) => self.answered(id, answer),
                    Ok(Received::Pushed(message)) => self.pushed(message),
                    Err(wellfed_connect::Error::Timeout) => {}
                    Err(error) => break Closed::Lost(error.into()),
                },
                _ = ticks.tick() => {
                    self.log.flush();
                    self.request(Purpose::Status, &Op::Status);
                }
                _ = hold.changed() => {}
                _ = states.changed() => {}
            }
        };

        // A read still out goes once more on the next session; anything
        // else may have run, and is never sent again.
        for (_, purpose) in self.pending.drain() {
            let Purpose::Caller(request) = purpose else {
                continue;
            };
            if read_only(&request.op) && !request.resent {
                let again = Request {
                    resent: true,
                    ..request
                };
                waiting.push((again, Instant::now() + OPEN_WAIT));
            } else {
                let _ = request.reply.send(Ok(Asked::Uncertain));
            }
        }
        closed
    }

    fn send(&mut self, request: Request) {
        if self.pending.contains_key(&request.id) {
            return refuse_duplicate(request);
        }
        match self.connection.request(request.id, &request.op) {
            Ok(()) => {
                self.pending.insert(request.id, Purpose::Caller(request));
            }
            // Refused before anything was written: it never reached the
            // feeder.
            Err(error) => {
                let _ = request.reply.send(Err(Unavailable::NotOpen {
                    error: Some(error.to_string()),
                }));
            }
        }
    }

    fn request(&mut self, purpose: Purpose, op: &Op) {
        let Some(id) = random() else {
            return;
        };
        if self.connection.request(id, op).is_ok() {
            self.pending.insert(id, purpose);
        }
    }

    fn answered(&mut self, id: [u8; 16], answer: Option<Answer>) {
        let Some(purpose) = self.pending.remove(&id) else {
            return;
        };
        match (purpose, answer) {
            (Purpose::Caller(request), Some(answer)) => {
                let _ = request.reply.send(Ok(Asked::Answered { answer }));
            }
            // No answer in time: a read goes once more, under its id.
            (Purpose::Caller(request), None) if read_only(&request.op) && !request.resent => {
                self.send(Request {
                    resent: true,
                    ..request
                });
            }
            (Purpose::Caller(request), None) => {
                let _ = request.reply.send(Ok(Asked::Uncertain));
            }
            (Purpose::Status, Some(Answer::Status(status))) => self.status(&status),
            (Purpose::Sync, Some(Answer::Done { .. })) => {
                self.store
                    .update(|state| state.feeder.clock.synced_at = Some(now()));
                // Read the clock back at once rather than at the next tick.
                self.request(Purpose::Status, &Op::Status);
            }
            (Purpose::Sync, Some(Answer::Refused(_))) => {
                *self.sync_refused = Some(Instant::now());
            }
            // A build without a log, a lost status, a sync that went
            // unanswered: the next status reads again.
            (Purpose::Status | Purpose::Sync | Purpose::Tail, _) => {}
        }
    }

    fn status(&mut self, status: &Status) {
        let host_ms = jiff::Timestamp::now().as_millisecond();
        let clock_ms = status.clock.map(|clock| clock.as_millisecond());
        let skew_ms = clock_ms.map(|clock| clock - host_ms);
        let build = status.build.to_ascii_lowercase();

        self.store.update(|state| {
            let feeder = &mut state.feeder;
            feeder.status = Some(FeederStatus {
                build: build.clone(),
                bank: status.bank,
                uptime_ms: status.uptime_ms,
                boot_count: status.boot_count,
                rssi: status.rssi,
                clock_ms: clock_ms.and_then(|ms| u64::try_from(ms).ok()),
                locked: status.locked,
                last_event: status.last_event,
                sessions: status.sessions,
            });
            feeder.status_at = Some(now());
            // Its count includes this session.
            feeder.other_sessions = status.sessions.saturating_sub(1);
            feeder.clock = ClockView {
                skew_ms,
                synced_at: feeder.clock.synced_at,
            };
            if let Some(seq) = status.last_event
                && feeder.journal.is_none_or(|journal| seq > journal.seq)
            {
                feeder.journal = Some(Journal {
                    seq,
                    epoch: self.cursor.map(|cursor| cursor.epoch),
                    from: JournalSource::Status,
                });
            }
        });
        self.banks.observe(status.bank, &build, Witness::Status);

        let skewed = skew_ms.is_none_or(|skew| skew.abs() > SKEW_MAX_MS);
        let refused_lately = self
            .sync_refused
            .is_some_and(|at| at.elapsed() < SYNC_REFUSED_WAIT);
        if self.clock == ClockSync::On && skewed && !refused_lately {
            // As `wellfed time` sets it: this host's clock and zone.
            let sync =
                wellfed_client::time::sync(jiff::Timestamp::now(), &jiff::tz::TimeZone::system());
            self.request(Purpose::Sync, &Op::TimeSync(sync));
        }

        // Once a session, after its first status: the log's line numbers
        // count from each boot, so after a restart the tail starts again
        // from the oldest line the feeder keeps, and otherwise after the
        // last line already written.
        if !self.tailing {
            self.tailing = true;
            let after = match *self.log_after {
                Some((boot, seq)) if boot == status.boot_count => Some(seq),
                _ => None,
            };
            *self.log_after = Some((status.boot_count, after.unwrap_or(0)));
            // A tail from the oldest line is another boot's numbering:
            // the held log starts again with it.
            if after.is_none() {
                self.logs.reset(None, None);
            }
            self.request(Purpose::Tail, &Op::TailLog { after });
        }
    }

    fn pushed(&mut self, message: Pushed) {
        match &message {
            Pushed::Event(event) => {
                *self.cursor = self.connection.cursor();
                self.store.update(|state| {
                    state.feeder.journal = Some(Journal {
                        seq: event.seq,
                        epoch: Some(event.epoch),
                        from: JournalSource::Event,
                    });
                });
                self.keep_cursor();
                self.notes
                    .seen(&Seen::Event(&event_name(event.body.kind())));
                self.events.push(message);
            }
            Pushed::Gap(_) => {
                *self.cursor = self.connection.cursor();
                self.keep_cursor();
                self.events.push(message);
            }
            Pushed::Log(batch) => {
                if let Some((_, seq)) = self.log_after.as_mut() {
                    *seq = batch.seq;
                }
                self.log.write(&batch.lines);
                for line in &batch.lines {
                    self.notes.seen(&Seen::Line(line));
                }
                let lines = batch.lines.clone();
                self.store.update(|state| {
                    state.feeder.log.extend(lines);
                    trim_tail(&mut state.feeder.log);
                });
                self.logs.push(message);
            }
            Pushed::Sample(_) => {}
        }
    }

    fn keep_cursor(&self) {
        if let Some(cursor) = *self.cursor {
            let kept = DeviceCursor {
                epoch: cursor.epoch,
                seq: cursor.seq,
            };
            let _ = files::replace_json(&self.root.join(DEVICE_CURSOR), &kept);
        }
    }
}

/// The feeder's log as files, one a UTC day, each line stamped with the
/// host's time. Buffered and flushed at each status read, never synced.
struct DeviceLog {
    directory: PathBuf,
    day: String,
    file: Option<BufWriter<File>>,
}

impl DeviceLog {
    fn new(root: &Path) -> Self {
        Self {
            directory: root.join(DEVICE_LOG),
            day: String::new(),
            file: None,
        }
    }

    fn write(&mut self, lines: &[String]) {
        let stamp = jiff::Timestamp::now();
        let day = stamp.strftime("%Y-%m-%d").to_string();
        if day != self.day || self.file.is_none() {
            self.flush();
            self.day = day;
            self.file = std::fs::create_dir_all(&self.directory)
                .and_then(|()| {
                    File::options()
                        .create(true)
                        .append(true)
                        .open(self.directory.join(format!("{}.log", self.day)))
                })
                .ok()
                .map(BufWriter::new);
        }
        let Some(file) = &mut self.file else {
            return;
        };
        let at = stamp.strftime("%Y-%m-%dT%H:%M:%SZ");
        for line in lines {
            // A full disk loses the file's copy, never the state's tail.
            let _ = writeln!(file, "{at} {line}");
        }
    }

    fn flush(&mut self) {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use wellfed_model::{Event, EventBody};

    use super::*;

    fn event(seq: u64) -> Pushed {
        Pushed::Event(Event {
            epoch: 1,
            seq,
            at: None,
            body: EventBody::Boot {
                bank: 1,
                reset_flags: 0,
            },
        })
    }

    #[tokio::test]
    async fn a_ring_that_drops_events_says_where_its_history_starts() {
        let ring = Pushes::new(4);
        ring.reset(Some(1), Some(10));
        for seq in 11..=13 {
            ring.push(event(seq));
        }
        let page = ring.page(0, Duration::ZERO).await;
        assert_eq!((page.floor, page.messages.len()), (Some(10), 3));

        for seq in 14..=17 {
            ring.push(event(seq));
        }
        let page = ring.page(0, Duration::ZERO).await;
        // 11 to 13 fell out: the ring now holds every event after 13.
        assert_eq!((page.first, page.next, page.floor), (3, 7, Some(13)));
        assert_eq!(page.messages.len(), 4);
    }

    #[test]
    fn only_reads_go_twice() {
        assert!(read_only(&Op::Status));
        assert!(read_only(&Op::Stats {
            section: wellfed_model::StatsSection::Counters
        }));
        assert!(!read_only(&Op::SetLock { locked: true }));
        assert!(!read_only(&Op::Feed { portions: 1 }));
        assert!(!read_only(&Op::TailLog { after: None }));
    }

    #[tokio::test]
    async fn a_new_epoch_starts_the_history_again_and_numbering_carries_on() {
        let ring = Pushes::new(8);
        ring.reset(Some(1), Some(10));
        for seq in 11..=13 {
            ring.push(event(seq));
        }

        ring.reset(Some(2), None);
        let page = ring.page(0, Duration::ZERO).await;

        assert_eq!((page.first, page.next), (3, 3));
        assert_eq!((page.epoch, page.floor), (Some(2), None));
        assert!(page.messages.is_empty());
    }
}
