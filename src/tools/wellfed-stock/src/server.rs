//! Stock's side of an install in the caller's process: the broker stock
//! connects to, the server's session with stock (its controller and
//! install-trial ledger) and, when asked, the loopback API in
//! [`crate::api`].
//!
//! `wellfed-stock serve` runs it for the bench, and `wellfed install` and
//! the Android app (through `wellfed-install`) run it as a task they own and
//! stop: an app cannot spawn this binary, and in one process the server
//! learns of an enrollment at once. With an installer's config the server's
//! session starts only once a feeder enrolled, since its identity and topics
//! derive from the device's ID; the broker holds that feeder's CONNACK
//! until the session has subscribed ([`broker::Handoff`]), so stock's
//! connect burst, its version report among it, reaches the server.
//!
//! Callers reach the running server through a [`Handle`]: its status, and
//! the trial requests the API's routes make. Nothing here retries a trial:
//! a request that gets no answer within 5 s is [`Refusal::Unanswered`], and
//! its outcome is read from the ledger, never by asking again.

use std::{net::IpAddr, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};
use tracing::info;

use crate::{
    api::{self, ApiState},
    build_info::BUILD,
    clock,
    config::{self, Config, State},
    controller::{self, Controller, Rejection, Status},
    enroll::Enrollment,
    mqtt::{
        self, Request,
        broker::{self, DeviceAuth, Handoff},
    },
    ota,
};

/// Whether the server may dispatch the stock-to-replacement install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtaTrial {
    Enabled,
    Disabled,
}

/// Whether the server also answers its loopback HTTP API. The bench's
/// tools and page read it; an app, which reaches the server through its
/// [`Handle`], leaves it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Loopback,
    Off,
}

/// How a server runs.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub trial: OtaTrial,
    /// With an installer's config and no feeder enrolled yet: how long the
    /// broker trusts the first stock feeder that connects.
    pub enroll: Option<Duration>,
    pub clock: clock::Policy,
    pub api: Api,
}

/// How long a request waits for the server's answer. A request it times
/// out may still have been recorded, so its outcome is read from the
/// ledger, not asked again.
const ANSWER_WAIT: Duration = Duration::from_secs(5);
/// How long a stopped server gets to end its tasks.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// The running server, owned by whoever started it.
pub struct Server {
    task: Option<JoinHandle<Result<()>>>,
    stop: watch::Sender<bool>,
    handle: watch::Receiver<Option<Handle>>,
}

impl Server {
    /// Start the server for the config at `path` (canonical: the
    /// enrollment, the ledger and the image lease sit beside it). Refused
    /// at once when the config cannot run (no device and no enrollment
    /// window), or when the trial is enabled on a build that is not
    /// reviewable ([`crate::build_info::Build::require_reviewed`]). Must be
    /// called inside a tokio runtime.
    pub fn start(path: &Path, options: Options) -> Result<Self> {
        // Every caller that enables the trial dispatches stock's install, so
        // every one is held to a build whose sources can be reviewed.
        if options.trial == OtaTrial::Enabled {
            BUILD.require_reviewed()?;
        }
        let state = config::state(path)?;
        let window = match (&state, options.enroll) {
            (State::Device(_), _) => None,
            (State::Unenrolled(_), Some(window)) => Some(window),
            (State::Unenrolled(_), None) => bail!(
                "no feeder is enrolled in {}; pass --enroll-minutes during an install",
                path.display()
            ),
        };

        let (stop, stopped) = watch::channel(false);
        let (ready, handle) = watch::channel(None);
        let task = tokio::spawn(run(
            path.to_path_buf(),
            state,
            window,
            options,
            stopped,
            ready,
        ));
        Ok(Self {
            task: Some(task),
            stop,
            handle,
        })
    }

    /// The server's handle, once its session with stock runs: at once for
    /// a known device, and after the enrollment for an installer's config.
    pub fn handle(&self) -> Option<Handle> {
        self.handle.borrow().clone()
    }

    /// Whether the server has ended, by a failure or the end of its
    /// enrollment window; [`Server::finished`] says why.
    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Wait for the server to end, and how it ended: an error, or `Ok` once
    /// stopped. Answers once; later calls find nothing left to wait for.
    pub async fn finished(&mut self) -> Result<()> {
        match self.task.take() {
            Some(task) => task.await.context("the stock server panicked")?,
            None => Ok(()),
        }
    }

    /// Stop the broker, the session and the API, each given a bounded
    /// wait; what is left after it is aborted.
    pub async fn stop(mut self) {
        self.stop.send_replace(true);
        if let Some(mut task) = self.task.take()
            && tokio::time::timeout(STOP_WAIT, &mut task).await.is_err()
        {
            task.abort();
        }
    }
}

/// The running server as its clients reach it: the API's routes, and an
/// installer in the same process.
#[derive(Clone)]
pub struct Handle {
    requests: mpsc::Sender<Request>,
    status: watch::Receiver<Status>,
    /// Config location; the enrollment and the image lease sit beside it.
    config_path: Arc<Path>,
    image_host: IpAddr,
}

/// Why a trial request went unrecorded, or why its outcome is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// Which of two feeders is the owner's cannot be told; nothing is sent.
    #[error("the enrollment is contested: another device tried the broker during the window")]
    Contested,
    #[error(transparent)]
    Rejected(#[from] Rejection),
    /// No answer within [`ANSWER_WAIT`]: the request may yet be recorded.
    #[error("outcome unknown; query status or repeat the SAME request ID")]
    Unanswered,
}

impl Handle {
    /// The status, with the enrollment read beside the config at each
    /// call: the broker records a contest while the server runs. An
    /// unreadable record fails the status: a client must not read it as a
    /// captured device and skip the owner's check.
    pub fn status(&self) -> wellfed_formats::Result<Status> {
        let mut status = self.status.borrow().clone();
        status.enrollment = Enrollment::view(&self.config_path)?;
        Ok(status)
    }

    /// Ask stock once to install the build the image server holds, under
    /// `input`'s request ID. Never retried here: a repeated ID returns the
    /// stored record without a second command.
    pub async fn trial(&self, input: ota::trial::Input) -> Result<ota::trial::Record, Refusal> {
        if Enrollment::contested_path(&self.config_path).exists() {
            return Err(Refusal::Contested);
        }
        let lease = ota::image::current(&self.config_path, self.image_host).await;
        self.operation(|response| Request::OtaTrial(input, lease, response))
            .await
    }

    /// Close the latest uncertain or failed trial from fresh version
    /// evidence; sends the device nothing.
    pub async fn resolve(
        &self,
        input: ota::trial::ResolveInput,
    ) -> Result<ota::trial::Record, Refusal> {
        self.operation(|response| Request::ResolveTrial(input, response))
            .await
    }

    /// The trial's build answered the device API from bank 0; sends the
    /// device nothing.
    pub async fn answered(
        &self,
        input: ota::trial::AnsweredInput,
    ) -> Result<ota::trial::Record, Refusal> {
        self.operation(|response| Request::TrialAnswered(input, response))
            .await
    }

    async fn operation<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<T, Rejection>>) -> Request,
    ) -> Result<T, Refusal> {
        let (send, receive) = oneshot::channel();
        self.requests
            .try_send(build(send))
            .map_err(|_| Rejection::Offline)?;

        let answer = tokio::time::timeout(ANSWER_WAIT, receive)
            .await
            .map_err(|_| Refusal::Unanswered)?
            .map_err(|_| Rejection::Offline)?;
        Ok(answer?)
    }
}

/// The broker, and once a device is known the session with it.
async fn run(
    path: std::path::PathBuf,
    state: State,
    window: Option<Duration>,
    options: Options,
    stop: watch::Receiver<bool>,
    ready: watch::Sender<Option<Handle>>,
) -> Result<()> {
    let (enrolled, enrollment) = watch::channel(false);
    let (listening, heard) = watch::channel(false);
    let (listen, auth, known) = match (state, window) {
        (State::Device(config), _) => (
            config.mqtt_listen,
            DeviceAuth::device(&config),
            Some(*config),
        ),
        (State::Unenrolled(config), Some(window)) => (
            config.mqtt_listen,
            DeviceAuth::enrolling(&config, &path, window).with_handoff(Handoff {
                enrolled,
                listening: heard,
            }),
            None,
        ),
        (State::Unenrolled(_), None) => bail!("no feeder is enrolled"),
    };

    // The broker stops when the server is asked to, or when the session
    // ends on its own (a failure, say): either way through its shutdown,
    // so none of its client tasks outlives the server in a long-lived
    // process.
    let (end, ended) = watch::channel(false);
    let broker_stop = {
        let stop = stop.clone();
        async move {
            tokio::select! {
                () = stopped(stop) => {}
                () = stopped(ended) => {}
            }
        }
    };
    let broker = broker::run(listen, auth, broker_stop);
    tokio::pin!(broker);
    let session = async {
        let config = match (known, window) {
            (Some(config), _) => config,
            (None, Some(window)) => enrolled_device(&path, window, enrollment).await?,
            (None, None) => bail!("no feeder is enrolled"),
        };
        serve(config, &path, options, listening, ready, stop.clone()).await
    };

    let outcome = tokio::select! {
        result = &mut broker => result.context("the broker"),
        result = session => {
            end.send_replace(true);
            let _ = tokio::time::timeout(STOP_WAIT, &mut broker).await;
            result
        }
    };
    match outcome {
        Ok(()) if !*stop.borrow() => bail!("the stock server stopped unexpectedly"),
        outcome => outcome,
    }
}

/// Resolves once the server is asked to stop.
async fn stopped(mut stop: watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// The enrollment window closed with no feeder: the server ended as asked,
/// unlike one whose port was taken or whose records could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no stock feeder connected within {minutes} min; nothing enrolled")]
pub struct NoFeederEnrolled {
    pub minutes: u64,
}

/// With no feeder enrolled, wait for the broker to admit one within
/// `window`, then the config with that device.
async fn enrolled_device(
    path: &Path,
    window: Duration,
    mut enrollment: watch::Receiver<bool>,
) -> Result<Config> {
    let minutes = window.as_secs() / 60;
    info!(
        minutes,
        "no feeder enrolled; the first stock feeder to connect is trusted"
    );
    match tokio::time::timeout(window, enrollment.wait_for(|enrolled| *enrolled)).await {
        Ok(Ok(_)) => config::load(path),
        Ok(Err(_)) => bail!("the broker ended while waiting for a feeder"),
        Err(_) => Err(NoFeederEnrolled { minutes }.into()),
    }
}

/// The session with stock for `config`'s device, and the API when asked,
/// until stopped.
async fn serve(
    config: Config,
    path: &Path,
    options: Options,
    listening: watch::Sender<bool>,
    ready: watch::Sender<Option<Handle>>,
    stop: watch::Receiver<bool>,
) -> Result<()> {
    let config = Arc::new(config);
    let listener = match options.api {
        Api::Loopback => Some(
            TcpListener::bind(config.http_listen)
                .await
                .context("bind loopback HTTP API")?,
        ),
        Api::Off => None,
    };

    let mut controller = Controller::load(
        (*config).clone(),
        controller::ledger_path(path, &config.device_id)?,
        BUILD,
    )?;
    if options.trial == OtaTrial::Enabled {
        controller.enable_ota_trial();
    }

    let (requests, receiver) = mpsc::channel(16);
    let (status, status_rx) = watch::channel(controller.status.clone());
    let handle = Handle {
        requests,
        status: status_rx.clone(),
        config_path: Arc::from(path),
        image_host: config.mqtt_listen.ip(),
    };
    ready.send_replace(Some(handle.clone()));

    let worker = mqtt::run(config.clone(), controller, receiver, status, options.clock);
    // The broker admits an enrolling feeder once the session subscribed.
    let mut connected = status_rx;
    let subscribed = async move {
        if connected
            .wait_for(|status| status.broker_connected)
            .await
            .is_ok()
        {
            listening.send_replace(true);
        }
        std::future::pending::<()>().await;
    };
    let api = async {
        match listener {
            Some(listener) => {
                let app = api::router(ApiState {
                    token: Arc::from(config.api_token.as_str()),
                    handle,
                });
                axum::serve(listener, app)
                    .with_graceful_shutdown(stopped(stop.clone()))
                    .await
                    .context("the loopback API")
            }
            None => std::future::pending().await,
        }
    };

    info!(mqtt=%config.mqtt_listen, http=%config.http_listen, api=?options.api, "local service starting; no traffic redirection applied");
    tokio::select! {
        result = worker => result.context("the MQTT worker"),
        result = api => result,
        () = subscribed => Ok(()),
        () = stopped(stop.clone()) => Ok(()),
    }
}
