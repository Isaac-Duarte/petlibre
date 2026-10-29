//! The one owner session with the feeder, held for the server's life.
//!
//! A tokio task owns exactly one `wellfed_connect::Connection`. It:
//! - subscribes after the event it last took ([`DEVICE_CURSOR`]), and keeps
//!   every pushed message in a ring that `GET /device/pushed` serves;
//! - follows the feeder's debug log into the state's tail and into
//!   [`DEVICE_LOG`], through a buffer that is never synced;
//! - reads the status every 10 s, and sets the clock as `wellfed time`
//!   does when it is invalid or more than 2 s off;
//! - carries the requests of the `wellfed` CLI (`POST /device/ask`) and of
//!   the jobs ([`SessionLink`]), each sent once: one that got no answer is
//!   reported uncertain, never sent again.
//!
//! It closes while a job holds the UART, while relay 1 is off and while
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
use wellfed_connect::{Connection, Cursor, Received};
use wellfed_formats::bench::banks::Witness;
use wellfed_formats::bench::feeder::{Asked, PushedPage, SessionInfo};
use wellfed_formats::bench::live::{
    BenchState, ClockView, DEVICE_CURSOR, DEVICE_LOG, DeviceCursor, FeederStatus, Journal,
    JournalSource, Pause, Session, Unavailable, trim_tail,
};
use wellfed_formats::bench::now;
use wellfed_formats::hex;
use wellfed_formats::private::{self, SYSTEM_RANDOM};
use wellfed_model::{Answer, Op, Pushed, Status};
use wellfed_rig::device::{self, DeviceError, DeviceLink, Owner};

use crate::banks::Banks;
use crate::files;
use crate::state::Store;

/// How often the status is read while the session is open.
const STATUS_EVERY: Duration = Duration::from_secs(10);
/// A clock further off than this is set again.
const SKEW_MAX_MS: i64 = 2_000;
/// The longest a request waits for a session that is opening.
const OPEN_WAIT: Duration = Duration::from_secs(20);
/// Reconnection backoff: doubling from the first, up to the last.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How many pushed messages the ring keeps for `GET /device/pushed`.
const KEPT_PUSHED: usize = 4096;
/// The longest `GET /device/pushed` holds a reader for news.
pub(crate) const PUSHED_WAIT_MAX: Duration = Duration::from_secs(30);

/// Where the session's key and feeder come from.
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    /// Root-relative: the owner key file (`wellfed setup` writes it).
    pub keys: PathBuf,
    /// Root-relative: the server config whose hotspot fields and time
    /// zone find the feeder and set its clock.
    pub config: PathBuf,
    /// The feeder's address, given rather than found (the simulator).
    pub address: Option<SocketAddr>,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            keys: device::OWNER_KEYS.into(),
            config: wellfed_formats::stock::SERVER_CONFIG.into(),
            address: None,
        }
    }
}

/// One request a caller hands the task.
struct Request {
    id: [u8; 16],
    op: Op,
    reply: oneshot::Sender<Result<Asked, Unavailable>>,
}

/// The handle every part of the server asks through.
#[derive(Clone)]
pub(crate) struct Feeder {
    requests: mpsc::UnboundedSender<Request>,
    /// Set while a job holds the UART, and once the server stops.
    hold: Arc<watch::Sender<Option<Pause>>>,
    pushed: Arc<Pushes>,
    info: Arc<Mutex<Option<SessionInfo>>>,
    /// Root-relative, as the jobs name it.
    keys: PathBuf,
    /// The owner's time zone, for a job's clock sync; UTC when unconfigured.
    zone: jiff::tz::TimeZone,
}

/// What [`Feeder::new`] leaves for [`Feeder::run`] to start.
pub(crate) struct Task {
    owner: Result<Owner, String>,
    address: Option<SocketAddr>,
    keys: PathBuf,
    root: PathBuf,
    store: Store,
    banks: Banks,
    requests: mpsc::UnboundedReceiver<Request>,
    hold: watch::Receiver<Option<Pause>>,
    pushed: Arc<Pushes>,
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
    ) -> (Self, Task) {
        let owner = Owner::open(root, &options.keys, &options.config).map_err(|e| e.to_string());
        let zone = owner
            .as_ref()
            .map_or(jiff::tz::TimeZone::UTC, |owner| owner.zone.clone());
        let (requests, receiver) = mpsc::unbounded_channel();
        let (hold, held) = watch::channel(None);
        let pushed = Arc::new(Pushes::default());
        let info = Arc::new(Mutex::new(None));
        let keys = root.join(&options.keys);
        let keys = keys.canonicalize().unwrap_or(keys);

        let feeder = Self {
            requests,
            hold: Arc::new(hold),
            pushed: Arc::clone(&pushed),
            info: Arc::clone(&info),
            keys: options.keys.clone(),
            zone,
        };
        let task = Task {
            owner,
            address: options.address,
            keys,
            root: root.to_path_buf(),
            store,
            banks,
            requests: receiver,
            hold: held,
            pushed,
            info,
        };
        (feeder, task)
    }

    /// Close the session for `pause`, or let it open again with `None`.
    pub(crate) fn hold(&self, pause: Option<Pause>) {
        self.hold.send_replace(pause);
    }

    /// Send `op` once under `id` and wait for what came of it.
    pub(crate) async fn ask(&self, id: [u8; 16], op: Op) -> Result<Asked, Unavailable> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request { id, op, reply })
            .map_err(|_| Unavailable::Ended)?;
        answer.await.unwrap_or(Err(Unavailable::Ended))
    }

    pub(crate) fn info(&self) -> Option<SessionInfo> {
        lock(&self.info).clone()
    }

    pub(crate) fn pushed(&self) -> &Pushes {
        &self.pushed
    }

    /// The link a job's sequence asks through.
    pub(crate) fn link(&self) -> (PathBuf, SessionLink) {
        let link = SessionLink {
            requests: self.requests.clone(),
            zone: self.zone.clone(),
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
    zone: jiff::tz::TimeZone,
}

impl DeviceLink for SessionLink {
    fn ask(&mut self, ask: &device::Ask) -> Result<device::Answer, DeviceError> {
        let id = private::random(Path::new(SYSTEM_RANDOM)).map_err(DeviceError::Random)?;
        let (reply, answer) = oneshot::channel();
        let request = Request {
            id,
            op: ask.op(&self.zone),
            reply,
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
}

/// Every message the session was pushed, numbered from the server's start,
/// the newest [`KEPT_PUSHED`] kept.
#[derive(Default)]
pub(crate) struct Pushes {
    inner: Mutex<Ring>,
    /// The number the next message takes.
    next: watch::Sender<u64>,
}

#[derive(Default)]
struct Ring {
    first: u64,
    messages: VecDeque<Pushed>,
}

impl Pushes {
    fn push(&self, message: Pushed) {
        let next = {
            let mut ring = lock(&self.inner);
            ring.messages.push_back(message);
            if ring.messages.len() > KEPT_PUSHED {
                ring.messages.pop_front();
                ring.first += 1;
            }
            ring.first + ring.messages.len() as u64
        };
        self.next.send_replace(next);
    }

    /// The messages from `from` on, waiting up to `wait` for the first.
    pub(crate) async fn page(&self, from: u64, wait: Duration) -> PushedPage {
        let mut next = self.next.subscribe();
        let deadline = Instant::now() + wait.min(PUSHED_WAIT_MAX);
        while *next.borrow_and_update() <= from {
            if tokio::time::timeout_at(deadline, next.changed())
                .await
                .is_err()
            {
                break;
            }
        }

        let ring = lock(&self.inner);
        let first = from.max(ring.first);
        let skip = usize::try_from(first - ring.first).unwrap_or(usize::MAX);
        PushedPage {
            first,
            next: ring.first + ring.messages.len() as u64,
            messages: ring.messages.iter().skip(skip).cloned().collect(),
        }
    }
}

/// What a request the task sent itself is for.
enum Purpose {
    Caller(oneshot::Sender<Result<Asked, Unavailable>>),
    Status,
    Sync,
    Tail,
}

/// Why a session ended.
enum Closed {
    Paused,
    Lost(String),
    /// Every handle is gone: the server has quit.
    Ended,
}

impl Task {
    /// Serve the session until every handle is dropped.
    pub(crate) async fn run(mut self) {
        let owner = match self.owner.clone() {
            Ok(owner) => owner,
            Err(error) => return self.unconfigured(error).await,
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
                tailing: false,
                store: self.store.clone(),
                banks: self.banks.clone(),
                root: self.root.clone(),
                pushed: Arc::clone(&self.pushed),
                zone: owner.zone.clone(),
            };
            let closed = session
                .serve(&mut self.requests, &self.hold, &mut states, &mut waiting)
                .await;
            log.flush();
            *lock(&self.info) = None;
            match closed {
                Closed::Paused => {}
                Closed::Lost(error) => {
                    attempt = 1;
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
        self.set_session(Session::Unconfigured {
            error: error.clone(),
        });
        loop {
            tokio::select! {
                request = self.requests.recv() => {
                    let Some(request) = request else { return };
                    let _ = request.reply.send(Err(Unavailable::Unconfigured {
                        error: error.clone(),
                    }));
                }
                changed = self.hold.changed() => {
                    let stopping = *self.hold.borrow() == Some(Pause::Stopping);
                    if changed.is_err() || stopping {
                        return;
                    }
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
    ) -> Result<(Connection, SocketAddr), String> {
        let address = match self.address {
            Some(address) => address,
            None => {
                let finding = owner.clone();
                tokio::task::spawn_blocking(move || finding.locate())
                    .await
                    .map_err(|error| error.to_string())?
                    .map_err(|error| error.to_string())?
            }
        };
        let ephemeral = random().ok_or("the system random source")?;
        let mut connection = Connection::open(&owner.keys, owner.identity, address, ephemeral)
            .await
            .map_err(|error| error.to_string())?;
        if cursor.is_some_and(|cursor| cursor.epoch != connection.epoch()) {
            *cursor = None;
        }
        connection
            .subscribe(*cursor, false)
            .await
            .map_err(|error| error.to_string())?;

        *lock(&self.info) = Some(SessionInfo {
            keys: self.keys.clone(),
            identity: hex::encode(&owner.identity),
            build: connection.build().to_string(),
            role: connection.role(),
            capabilities: connection.capabilities(),
        });
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

/// Why the session must stay closed now: the server's hold, or relay 1
/// read off. A relay not read yet is no reason.
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
        .get("1")
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
    /// Whether this session asked for the log yet.
    tailing: bool,
    store: Store,
    banks: Banks,
    root: PathBuf,
    pushed: Arc<Pushes>,
    zone: jiff::tz::TimeZone,
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
                    Some(request) => self.send(request),
                    None => break Closed::Ended,
                },
                received = self.connection.receive(STATUS_EVERY) => match received {
                    Ok(Received::Answer { id, answer }) => self.answered(id, answer),
                    Ok(Received::Pushed(message)) => self.pushed(message),
                    Err(wellfed_connect::Error::Timeout) => {}
                    Err(error) => break Closed::Lost(error.to_string()),
                },
                _ = ticks.tick() => {
                    self.log.flush();
                    self.request(Purpose::Status, &Op::Status);
                }
                _ = hold.changed() => {}
                _ = states.changed() => {}
            }
        };

        // Each request still out may have run: none is sent again.
        for (_, purpose) in self.pending.drain() {
            if let Purpose::Caller(reply) = purpose {
                let _ = reply.send(Ok(Asked::Uncertain));
            }
        }
        closed
    }

    fn send(&mut self, request: Request) {
        match self.connection.request(request.id, &request.op) {
            Ok(()) => {
                self.pending
                    .insert(request.id, Purpose::Caller(request.reply));
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
            (Purpose::Caller(reply), Some(answer)) => {
                let _ = reply.send(Ok(Asked::Answered { answer }));
            }
            (Purpose::Caller(reply), None) => {
                let _ = reply.send(Ok(Asked::Uncertain));
            }
            (Purpose::Status, Some(Answer::Status(status))) => self.status(&status),
            (Purpose::Sync, Some(Answer::Done { .. })) => {
                self.store
                    .update(|state| state.feeder.clock.synced_at = Some(now()));
                // Read the clock back at once rather than at the next tick.
                self.request(Purpose::Status, &Op::Status);
            }
            // A build without a log, a refused sync, a lost status: the next
            // status reads again.
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

        if skew_ms.is_none_or(|skew| skew.abs() > SKEW_MAX_MS) {
            let sync = wellfed_client::time::sync(jiff::Timestamp::now(), &self.zone);
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
            }
            Pushed::Gap(_) => {
                *self.cursor = self.connection.cursor();
                self.keep_cursor();
            }
            Pushed::Log(batch) => {
                if let Some((_, seq)) = self.log_after.as_mut() {
                    *seq = batch.seq;
                }
                self.log.write(&batch.lines);
                let lines = batch.lines.clone();
                self.store.update(|state| {
                    state.feeder.log.extend(lines);
                    trim_tail(&mut state.feeder.log);
                });
            }
            Pushed::Sample(_) => {}
        }
        self.pushed.push(message);
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
