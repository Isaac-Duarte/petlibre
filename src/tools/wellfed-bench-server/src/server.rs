//! `wellfed-bench serve`: the lease, the board and the socket for the
//! server's life, and one job at a time.
//!
//! Opening takes the bench lease before anything else, so a second server
//! or a hand-run tool is refused before a port opens; the jobs' children
//! then run on that lease (`lease::descends_from`). Quitting cancels the
//! running job and waits for its cleanup before the lease is released, so
//! no sequence is left half done by the server going away.

use std::fs::File;
use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::Notify;
use wellfed_formats::bench::banks::BANKS;
use wellfed_formats::bench::lease::{self, Holder, Lease};
use wellfed_formats::bench::live::{
    BenchState, CancelState, JOBS, JobEnd, JobId, JobView, Pause, Phase, ServerView, Uart,
};
use wellfed_formats::bench::notes::{NOTES, Note, Operator};
use wellfed_formats::bench::progress::{Current, ProgressSink};
use wellfed_formats::bench::rendezvous::{self, Rendezvous};
use wellfed_formats::bench::{CONFIG, Config, now, relative};
use wellfed_relay::{Board, POWER};
use wellfed_rig::Cancel;
use wellfed_rig::tools::Children;

use crate::api::{JobRequest, Refusal, SOCKET};
use crate::banks::Banks;
use crate::board::SharedBoard;
use crate::capture::CaptureService;
use crate::feeder::{DeviceOptions, Feeder, Task};
use crate::jobs::{self, Bed, JobError, Ran, Run, ToolPaths};
use crate::output::{self, Output};
use crate::routes;
use crate::state::{JobLog, JobOutput, Logs, Store};

/// How long a quitting server lets its clients' connections finish once
/// the job has ended.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// How often the relays are read while no job runs, for the board's front
/// buttons. Never while a job runs: the job owns the board then.
const RELAY_POLL: Duration = Duration::from_secs(30);
/// How often the hotspot's connection is looked at.
const HOTSPOT_POLL: Duration = Duration::from_secs(30);
/// How long a quitting server lets the feeder session close and write out
/// its log.
const SESSION_CLOSE: Duration = Duration::from_secs(2);

/// Why the server did not start, or stopped serving.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Lease(#[from] lease::LeaseError),
    #[error("{CONFIG}: {0}")]
    Config(wellfed_formats::Error),
    #[error("Relay board: {0}")]
    Relay(#[from] wellfed_relay::Error),
    #[error("{}: {error}", socket.display())]
    Socket {
        socket: PathBuf,
        error: std::io::Error,
    },
    #[error("The output pipe: {0}")]
    Output(std::io::Error),
    /// A ledger that does not read is never taken for an empty one: the
    /// page would show the banks as unknown and the next job's entry would
    /// overwrite what the bench knew.
    #[error("{}: {error}; fix it or delete it, then start the server again", path.display())]
    Banks {
        path: PathBuf,
        error: wellfed_formats::Error,
    },
    /// Another checkout's server answers for this user: the `wellfed` CLI
    /// finds one server, so one serves at a time.
    #[error("A bench server for {} already runs ({holder}); stop it first", root.display())]
    OtherServer { root: PathBuf, holder: Box<Holder> },
    #[error("{}: {error}", directory.display())]
    Rendezvous {
        directory: PathBuf,
        error: wellfed_formats::Error,
    },
}

/// Where the server runs and the tools its jobs run.
#[derive(Debug, Clone)]
pub struct Options {
    /// The checkout, canonical: its `private/` holds the config, the
    /// evidence and the socket.
    pub root: PathBuf,
    pub tools: ToolPaths,
}

/// An opened server: lease taken, board open and queried, socket bound.
pub struct Server {
    shared: Arc<Shared>,
    listener: std::os::unix::net::UnixListener,
    socket: PathBuf,
    /// The feeder session's task, started by [`Self::run`].
    session: Task,
    /// The pointer this server published, to withdraw when it quits.
    rendezvous: Option<(Vec<PathBuf>, Holder)>,
    /// Held until the server is dropped, after its last job.
    _lease: Lease,
}

impl Server {
    /// Take the lease, open the board `open_board` names from the config
    /// and query it, then bind the socket. Each step refuses before the
    /// next opens anything.
    pub fn open(
        options: Options,
        open_board: impl FnOnce(&Config) -> wellfed_relay::Result<Box<dyn Board>>,
    ) -> Result<Self, ServeError> {
        Self::open_with(options, &DeviceOptions::default(), open_board)
    }

    /// [`Self::open`], with the feeder session's key, config and address.
    pub fn open_with(
        options: Options,
        device: &DeviceOptions,
        open_board: impl FnOnce(&Config) -> wellfed_relay::Result<Box<dyn Board>>,
    ) -> Result<Self, ServeError> {
        // The socket, the evidence and the job logs are the owner's alone,
        // whatever umask the server was started with.
        rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077));

        let lease = Lease::acquire(&options.root)?;
        let config: Config =
            wellfed_formats::load(&options.root.join(CONFIG)).map_err(ServeError::Config)?;

        let store = Store::new(BenchState {
            now: now(),
            server: ServerView {
                pid: std::process::id(),
                started: now(),
                phase: Phase::Serving,
                uart_port: Path::new(&config.uart_port).exists(),
            },
            job: None,
            activity: Vec::new(),
            uart: Uart::Idle,
            relays: Default::default(),
            feeder: Default::default(),
            capture: Default::default(),
            banks: Default::default(),
            operator: operator(&options.root),
        });
        let banks =
            Banks::open(&options.root, store.clone()).map_err(|error| ServeError::Banks {
                path: options.root.join(BANKS),
                error,
            })?;
        let board = SharedBoard::new(open_board(&config)?, store.clone());
        board.query()?;
        let (feeder, session) = Feeder::new(&options.root, device, store.clone(), banks.clone());
        let capture = CaptureService::new(
            options.root.clone(),
            config.uart_port.clone(),
            options.tools.clone(),
            store.clone(),
            banks.clone(),
        );

        if let Some(other) = rendezvous::live_in(&device.rendezvous)
            && other.root != options.root
        {
            return Err(ServeError::OtherServer {
                root: other.root,
                holder: Box::new(other.holder),
            });
        }
        let socket = options.root.join(SOCKET);
        let listener = bind(&socket).map_err(|error| ServeError::Socket {
            socket: socket.clone(),
            error,
        })?;
        let rendezvous = if device.rendezvous.is_empty() {
            None
        } else {
            Some(publish(&device.rendezvous, &options.root, &socket)?)
        };
        let output = output::install().map_err(ServeError::Output)?;

        let jobs = Jobs {
            last: last_job(&options.root.join(JOBS)),
            ..Jobs::default()
        };
        let bed = Bed {
            root: options.root,
            config,
            board,
            tools: options.tools,
            feeder,
        };
        Ok(Self {
            shared: Arc::new(Shared {
                bed,
                store,
                output,
                jobs: Mutex::new(jobs),
                banks,
                capture,
            }),
            listener,
            socket,
            session,
            rendezvous,
            _lease: lease,
        })
    }

    /// The live state, for a reader in this process (the page).
    pub fn store(&self) -> Store {
        self.shared.store.clone()
    }

    /// The jobs' output, for a reader in this process (the page).
    pub fn outputs(&self) -> Outputs {
        Outputs(Arc::clone(&self.shared))
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// What the socket's handlers share, for a test to reach past them.
    #[cfg(test)]
    pub(crate) fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }

    /// A line on the server's console, which is no job's output.
    pub fn say(&self, line: String) {
        self.shared.output.say(line);
    }

    /// Serve until `shutdown` completes, then cancel the running job, wait
    /// for its cleanup and close. The lease goes when `self` does.
    pub async fn run(self, shutdown: impl Future<Output = ()>) -> Result<(), ServeError> {
        let socket_error = |error| ServeError::Socket {
            socket: self.socket.clone(),
            error,
        };
        let listener = self.listener.try_clone().map_err(socket_error)?;
        listener.set_nonblocking(true).map_err(socket_error)?;
        let listener = tokio::net::UnixListener::from_std(listener).map_err(socket_error)?;

        let closing = Arc::new(Notify::new());
        let closed = Arc::clone(&closing);
        let router = routes::router(Arc::clone(&self.shared));
        let mut serving = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { closed.notified().await })
                .await
        });
        let session = tokio::spawn(self.session.run());
        let hotspot = tokio::spawn(hotspot(
            self.shared.bed.root.clone(),
            self.shared.store.clone(),
        ));
        let idle = Arc::clone(&self.shared);
        let _ = tokio::task::spawn_blocking(move || idle.resume_capture()).await;

        let polled = Arc::clone(&self.shared);
        let polling = tokio::spawn(async move {
            let mut ticks = tokio::time::interval(RELAY_POLL);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                let shared = Arc::clone(&polled);
                let _ = tokio::task::spawn_blocking(move || shared.poll_relays()).await;
            }
        });

        // A socket that fails ends the serving early; the job is still
        // stopped before the lease goes.
        let failed = tokio::select! {
            () = shutdown => None,
            served = &mut serving => Some(served),
        };

        polling.abort();
        hotspot.abort();
        self.shared
            .output
            .say("Stopping: cancelling the running job, then releasing the bench".into());
        if let Some(job) = self.shared.stop() {
            let _ = tokio::task::spawn_blocking(move || job.join()).await;
        }
        let capture = self.shared.capture.clone();
        let _ = tokio::task::spawn_blocking(move || capture.stop()).await;
        // The session closes on the stop's hold and writes out its log.
        let mut session = session;
        if tokio::time::timeout(SESSION_CLOSE, &mut session)
            .await
            .is_err()
        {
            session.abort();
        }

        let served = match failed {
            Some(served) => served,
            None => {
                closing.notify_one();
                match tokio::time::timeout(CLOSE_WAIT, &mut serving).await {
                    Ok(served) => served,
                    // A client still reading the end of a stream.
                    Err(_) => {
                        serving.abort();
                        Ok(Ok(()))
                    }
                }
            }
        };
        let _ = std::fs::remove_file(&self.socket);
        if let Some((directories, holder)) = &self.rendezvous {
            for directory in directories {
                rendezvous::withdraw(directory, holder);
            }
        }

        match served {
            Ok(Err(error)) => Err(socket_error(error)),
            // Served to the end, or the serving task was cut short.
            Ok(Ok(())) | Err(_) => Ok(()),
        }
    }
}

/// Put the terminal back on the process's stdout and stderr and let the
/// console write out what it holds, after a server opened in this process.
/// `serve` calls it on every way out, so its last lines, an error among
/// them, are never lost. Safe to repeat.
pub fn restore_output() {
    output::close_installed();
}

/// Bind the socket, replacing one a killed server left (the lease, already
/// held, says no live server owns it), and make it the owner's alone.
fn bind(socket: &Path) -> std::io::Result<std::os::unix::net::UnixListener> {
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Point every one of `directories`' rendezvous at this server, so a
/// client finds it whichever of them its own environment names.
fn publish(
    directories: &[PathBuf],
    root: &Path,
    socket: &Path,
) -> Result<(Vec<PathBuf>, Holder), ServeError> {
    let holder = Holder::current()?;
    let pointer = Rendezvous {
        socket: socket.to_path_buf(),
        root: root.to_path_buf(),
        holder: holder.clone(),
    };
    for directory in directories {
        rendezvous::publish(directory, &pointer).map_err(|error| ServeError::Rendezvous {
            directory: directory.clone(),
            error,
        })?;
    }
    Ok((directories.to_vec(), holder))
}

/// Whether the hotspot may be read now: never while a job runs or the UART
/// is in a loader session or a write, as the relays are not polled then
/// either. A process spawned mid-write is load the write does not need.
pub(crate) fn hotspot_due(state: &BenchState) -> bool {
    let job_running = state.job.as_ref().is_some_and(|job| job.end.is_none());
    let uart_busy = matches!(state.uart, Uart::Loader | Uart::Writing { .. });
    !job_running && !uart_busy
}

/// Whether the bench hotspot's connection is up, every [`HOTSPOT_POLL`],
/// into the feeder's view. Read-only: nmcli only lists what is active.
async fn hotspot(root: PathBuf, store: Store) {
    use wellfed_rig::hotspot::{self, HOTSPOT, HotspotFile};

    let mut ticks = tokio::time::interval(HOTSPOT_POLL);
    loop {
        ticks.tick().await;
        if !hotspot_due(&store.read()) {
            continue;
        }
        let Ok(file) = wellfed_formats::load::<HotspotFile>(&root.join(HOTSPOT)) else {
            store.update(|state| state.feeder.hotspot = None);
            continue;
        };
        let up = tokio::task::spawn_blocking(move || hotspot::active(&file.connection).ok())
            .await
            .ok()
            .flatten();
        store.update(|state| state.feeder.hotspot = up);
    }
}

/// What the operator's notes say, replayed from the start; a line that
/// does not read is skipped, as the page skips one.
fn operator(root: &Path) -> Operator {
    let text = std::fs::read_to_string(root.join(NOTES)).unwrap_or_default();
    let notes: Vec<Note> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    Operator::fold(&notes)
}

/// The highest job number among the logs in `directory`, so a new server
/// carries on from it rather than overwrite an earlier job's log.
fn last_job(directory: &Path) -> JobId {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let (id, _) = name.to_str()?.split_once('-')?;
            id.parse::<JobId>().ok()
        })
        .max()
        .unwrap_or(0)
}

/// The output of the running job and of the last few, by id.
#[derive(Clone)]
pub struct Outputs(Arc<Shared>);

impl Outputs {
    pub fn get(&self, id: JobId) -> Option<JobOutput> {
        self.0.log(id).map(JobOutput)
    }
}

/// What the socket's handlers and the job thread share.
pub(crate) struct Shared {
    bed: Bed,
    pub(crate) store: Store,
    output: Arc<Output>,
    jobs: Mutex<Jobs>,
    pub(crate) banks: Banks,
    capture: CaptureService,
}

#[derive(Default)]
struct Jobs {
    /// The last id given; the next job takes one more.
    last: JobId,
    running: Option<Running>,
    logs: Logs,
    stopping: bool,
}

struct Running {
    id: JobId,
    label: String,
    cancel: Cancel,
    children: Children,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What stopping a job needs, taken out of the jobs lock so the signals
/// are sent without holding it.
struct Stopping {
    id: JobId,
    cancel: Cancel,
    children: Children,
    log: Option<Arc<JobLog>>,
}

impl Stopping {
    fn of(running: &Running, log: Option<Arc<JobLog>>) -> Self {
        Self {
            id: running.id,
            cancel: running.cancel.clone(),
            children: running.children.clone(),
            log,
        }
    }
}

impl Shared {
    fn jobs(&self) -> MutexGuard<'_, Jobs> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Parse the request's words as a job and start it on a thread of its
    /// own, unless it comes from another checkout, a job is running or the
    /// server is stopping.
    pub(crate) fn start(self: &Arc<Self>, request: &JobRequest) -> Result<JobId, Refusal> {
        if request.root != self.bed.root {
            return Err(Refusal::OtherCheckout {
                server: self.bed.root.display().to_string(),
                client: request.root.display().to_string(),
            });
        }
        let spec = jobs::parse(&request.argv)?;

        let mut jobs = self.jobs();
        if jobs.stopping {
            return Err(Refusal::Stopping);
        }
        if let Some(running) = &jobs.running {
            return Err(Refusal::Busy {
                id: running.id,
                label: running.label.clone(),
            });
        }

        jobs.last += 1;
        let id = jobs.last;
        let label = spec.command.describe();
        let file_name = format!(
            "{id}-{}.log",
            spec.command
                .name()
                .unwrap_or_else(|| spec.command.label().word())
        );
        let path = self.bed.root.join(JOBS).join(file_name);
        let file = std::fs::create_dir_all(self.bed.root.join(JOBS))
            .and_then(|()| File::create_new(&path));
        let file = match file {
            Ok(file) => Some(file),
            Err(error) => {
                self.output.say(format!(
                    "Job {id}: no log file ({}: {error})",
                    path.display()
                ));
                None
            }
        };
        let log = jobs.logs.add(id, file);

        let (cancel, children) = (Cancel::new(), Children::new());
        let store = self.store.clone();
        children.watch(move |list| {
            store.update(|state| {
                state.activity = list.to_vec();
                state.uart = Uart::of(state.job.as_ref().and_then(|j| j.record.as_ref()), list);
            });
        });

        self.store.update(|state| {
            state.job = Some(JobView {
                id,
                label: label.clone(),
                log: relative(&self.bed.root, &path),
                accepted: now(),
                record: None,
                upcoming: Vec::new(),
                cancel: CancelState::None,
                end: None,
            });
        });
        self.output.say(format!("Job {id}: {label}"));
        self.output.route(Some(Arc::clone(&log)));

        let run = Run {
            cancel: cancel.clone(),
            children: children.clone(),
            sink: Box::new(JobSink {
                id,
                store: self.store.clone(),
            }),
        };
        let shared = Arc::clone(self);
        let thread = std::thread::Builder::new()
            .name(format!("job {id}"))
            .spawn(move || shared.run_job(id, &spec, run, &log));
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                drop(jobs);
                self.output.route(None);
                self.finish(id, None, 1, Some(JobError::Spawn(error)));
                return Ok(id);
            }
        };

        jobs.running = Some(Running {
            id,
            label,
            cancel,
            children,
            thread: Some(thread),
        });
        Ok(id)
    }

    fn run_job(&self, id: JobId, spec: &jobs::JobSpec, run: Run, log: &Arc<JobLog>) {
        // The port is the job's alone: the server's own capture lets go of
        // it first, and the session closes for a job that holds the UART.
        if spec.command.holds_uart() {
            self.bed.feeder.hold(Some(Pause::UartJob {
                job: id,
                label: spec.command.describe(),
            }));
        } else if spec.command.own_sessions() {
            self.bed.feeder.hold(Some(Pause::StockOta { job: id }));
        }
        self.capture.stop();

        let ran = catch_unwind(AssertUnwindSafe(|| jobs::run(&self.bed, spec, run)));
        let Ran {
            code,
            error,
            written,
        } = ran.unwrap_or(Ran {
            code: 1,
            error: Some(JobError::Panicked),
            written: Vec::new(),
        });

        // Every line the job wrote reaches its stream before the end does.
        self.output.sync();
        self.output.route(None);
        // The board's front buttons may have moved a relay meanwhile.
        self.look_at_bench();
        self.banks.record(&written, spec.command.name());
        let stopping = self.jobs().stopping;
        if !stopping {
            self.bed.feeder.hold(None);
        }
        self.finish(id, Some(log), code, error);
        self.resume_capture();
    }

    /// Capture the UART while no job runs and the feeder is powered; stop
    /// capturing once relay 1 is off.
    ///
    /// Decided and started under the jobs lock, as a job is accepted: a job
    /// is either running here, so no capture starts, or it starts after,
    /// and its own stop (the first thing its thread does) ends this one.
    pub(crate) fn resume_capture(&self) {
        let powered = self
            .store
            .read()
            .relays
            .get(&POWER.to_string())
            .is_some_and(|relay| relay.on);
        let jobs = self.jobs();
        let idle = jobs.running.is_none() && !jobs.stopping;
        if idle && powered {
            self.capture.start();
            return;
        }
        drop(jobs);
        if !powered {
            self.capture.stop();
        }
    }

    /// `run` while no job runs, under the jobs lock so none starts during
    /// it, or the refusal a job would get.
    pub(crate) fn while_idle<T>(&self, run: impl FnOnce() -> T) -> Result<T, Refusal> {
        let jobs = self.jobs();
        if jobs.stopping {
            return Err(Refusal::Stopping);
        }
        if let Some(running) = &jobs.running {
            return Err(Refusal::Busy {
                id: running.id,
                label: running.label.clone(),
            });
        }
        Ok(run())
    }

    /// The capture service, for a test to watch.
    #[cfg(test)]
    pub(crate) fn capture(&self) -> &CaptureService {
        &self.capture
    }

    pub(crate) fn feeder(&self) -> &Feeder {
        &self.bed.feeder
    }

    /// Free the bench, then record the end everywhere: the state, the
    /// stream, the console. A client that sees the end can start the next
    /// job at once.
    fn finish(&self, id: JobId, log: Option<&Arc<JobLog>>, code: u8, error: Option<JobError>) {
        let log = {
            let mut jobs = self.jobs();
            if jobs
                .running
                .as_ref()
                .is_some_and(|running| running.id == id)
            {
                jobs.running = None;
            }
            log.cloned().or_else(|| jobs.logs.get(id))
        };

        let error = error.map(|error| error.to_string());
        let ended = match &error {
            Some(error) => format!("Job {id} stopped: {error}"),
            None => format!("Job {id} ended with code {code}"),
        };
        let end = JobEnd {
            code,
            error,
            finished: now(),
        };
        self.store.update(|state| {
            // The next job may already be the state's.
            if let Some(job) = state.job.as_mut().filter(|job| job.id == id) {
                job.end = Some(end.clone());
                state.activity.clear();
                state.uart = Uart::Idle;
            }
        });
        if let Some(log) = log {
            log.finish(end);
        }
        self.output.say(ended);
    }

    /// Ask the running job `id` to stop: its sequence sees the cancel at
    /// its next wait, and its children are signalled as a terminal's Ctrl+C
    /// would have.
    pub(crate) fn cancel(&self, id: JobId) -> Result<(), Refusal> {
        let jobs = self.jobs();
        let Some(running) = jobs.running.as_ref().filter(|running| running.id == id) else {
            return match jobs.logs.get(id) {
                Some(_) => Err(Refusal::Finished { id }),
                None => Err(Refusal::NoSuchJob { id }),
            };
        };
        let stopping = Stopping::of(running, jobs.logs.get(id));
        drop(jobs);

        self.interrupt(&stopping);
        Ok(())
    }

    fn interrupt(&self, stopping: &Stopping) {
        // The flag first: the rig's own waits then stop their receive-only
        // captures, and a flash client that exits on the signal is read as
        // the interrupt it was.
        stopping.cancel.cancel();
        stopping.children.interrupt();

        let id = stopping.id;
        self.store.update(|state| {
            if let Some(job) = state
                .job
                .as_mut()
                .filter(|job| job.id == id && job.end.is_none())
            {
                job.cancel = CancelState::Requested;
            }
        });
        if let Some(log) = &stopping.log {
            log.push(format!("Cancel requested for job {id}"));
        }
        self.output.say(format!("Job {id}: cancel requested"));
    }

    /// Refuse new jobs and cancel the running one; its thread, to wait on.
    fn stop(&self) -> Option<std::thread::JoinHandle<()>> {
        self.store
            .update(|state| state.server.phase = Phase::Stopping);
        self.bed.feeder.hold(Some(Pause::Stopping));
        let mut jobs = self.jobs();
        jobs.stopping = true;
        let log = jobs
            .running
            .as_ref()
            .and_then(|running| jobs.logs.get(running.id));
        let running = jobs.running.as_mut()?;
        let thread = running.thread.take();
        let stopping = Stopping::of(running, log);
        drop(jobs);

        self.interrupt(&stopping);
        thread
    }

    /// Read the relays unless a job runs; whether it did.
    ///
    /// The jobs lock is held only for the check: four AT round trips to a
    /// slow or unplugged board must not stall new jobs, streams and
    /// cancels. A job that starts during the read waits at its first
    /// switch, since the board's lock is held for the whole read, and its
    /// switches then go out after it, never between two of its queries.
    pub(crate) fn poll_relays(&self) -> bool {
        {
            let jobs = self.jobs();
            if jobs.running.is_some() || jobs.stopping {
                return false;
            }
        }
        self.look_at_bench();
        self.resume_capture();
        true
    }

    /// Read the relays and look for the UART adapter's port: the board's
    /// front buttons may have moved a relay, and a replugged adapter may
    /// have come or gone.
    fn look_at_bench(&self) {
        if let Err(error) = self.bed.board.query() {
            self.output.say(format!("Relay board: {error}"));
        }
        let present = Path::new(&self.bed.config.uart_port).exists();
        self.store.update(|state| state.server.uart_port = present);
    }

    /// The output of job `id`, running or among the last few.
    pub(crate) fn log(&self, id: JobId) -> Option<Arc<JobLog>> {
        self.jobs().logs.get(id)
    }
}

/// A job's record into the live state.
struct JobSink {
    id: JobId,
    store: Store,
}

impl ProgressSink for JobSink {
    fn publish(&mut self, current: &Current) {
        let id = self.id;
        self.store.update(|state| {
            if let Some(job) = state.job.as_mut().filter(|job| job.id == id) {
                job.update(current);
            }
            state.uart = Uart::of(Some(current), &state.activity);
        });
    }
}
