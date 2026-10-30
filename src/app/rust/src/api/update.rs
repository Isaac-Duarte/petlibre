//! Updating a Wellfed feeder from the image this APK carries: the driver the
//! install's promotion and the CLI share, on a thread of its own, with what it
//! reports on one stream. A failure is a value on that stream, for the reason
//! [`crate::api::install`] gives.

use std::cell::Cell;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flutter_rust_bridge::frb;
use wellfed_client::update::{Image, InstallError};
use wellfed_connect::Connection;
use wellfed_connect::update::Pushed;
use wellfed_formats::hex;
use wellfed_install::event::Event;
use wellfed_install::push::push;
use wellfed_install::{Discovery, Review};
use wellfed_model::{Answer, Op, Readiness, Status, UpdateView};
use wellfed_session::noise::Keypair;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::feeder::FeederRefusal;
use crate::api::install::{ImageCheck, InstallAnswer, InstallAsk, InstallUpdate};
use crate::api::keys::ClientKey;
use crate::api::model::UpdateStop;
use crate::frb_generated::StreamSink;
use crate::install::{AppDiscovery, Asks, UPDATE_MANIFEST, check_image, guarded, pin};
use crate::requests::refusal_of;

/// The driver's future is deep, as an install's is.
const STACK: usize = 16 * 1024 * 1024;
/// From activation until the restart has begun.
const RESTART: Duration = Duration::from_secs(20);
/// As long as an install waits for its build to confirm.
const CONFIRM_WAIT: Duration = Duration::from_secs(6 * 60);
const POLL: Duration = Duration::from_secs(5);

pub enum UpdateProgress {
    /// Bank 0 took this much of the image.
    Sending {
        written: u32,
        length: u32,
    },
    /// The connection dropped; chunks resume where the feeder says.
    Reconnecting,
    /// Where the feeder is: answer by id with [`FirmwareUpdate::answer`].
    Ask {
        id: u32,
        ask: InstallAsk,
    },
    /// The feeder restarts onto the new version and tries it.
    Activated,
    Ended {
        end: UpdateEnd,
    },
}

/// How an update ended.
pub enum UpdateEnd {
    /// The new version runs, and has confirmed itself.
    Confirmed,
    /// It already ran this build; nothing was sent.
    AlreadyRunning,
    Refused {
        refusal: FeederRefusal,
    },
    /// Bank 0 stopped the install before activating it.
    Stopped {
        stop: UpdateStop,
    },
    /// Bank 0 is not installing this update any more.
    Lost,
    /// The offer went out and the feeder stopped answering: only the feeder
    /// can tell what happened, and the offer is not sent again.
    Uncertain,
    /// The feeder never answered, so nothing was offered.
    NotOffered,
    /// The new version was activated, and has not confirmed in the time an
    /// install gives it.
    Unconfirmed,
    /// The new version was activated, and the feeder runs bank 0's copy, which
    /// may be older than what it ran before the update: a trial that failed.
    WentBack,
    /// Something of this phone's before any offer: the image, or its address.
    Failed {
        detail: String,
    },
}

#[frb(opaque)]
pub struct FirmwareUpdate {
    keys: Keypair,
    feeder: [u8; 32],
    address: Option<String>,
    image_dir: String,
    asks: Arc<Asks>,
}

impl FirmwareUpdate {
    #[frb(sync)]
    pub fn new(
        key: &ClientKey,
        feeder: Vec<u8>,
        address: Option<String>,
        image_dir: String,
    ) -> Result<Self, BridgeError> {
        let feeder = <[u8; 32]>::try_from(feeder)
            .map_err(|_| BridgeError::new(ErrorKind::Key, "a feeder's key is 32 bytes"))?;
        Ok(Self {
            keys: key.keypair().clone(),
            feeder,
            address,
            image_dir,
            asks: Arc::new(Asks::new()),
        })
    }

    /// Answer the ask `id`. An answer nobody waits for any more is dropped.
    #[frb(sync)]
    pub fn answer(&self, id: u32, answer: InstallAnswer) {
        self.asks.answer(id, answer);
    }

    /// Offer the image once, and report until the update ends. The stream
    /// closes with the run.
    pub fn run(&self, updates: StreamSink<UpdateProgress>) {
        let keys = self.keys.clone();
        let (feeder, address) = (self.feeder, self.address.clone());
        let image_dir = self.image_dir.clone();
        let asks = self.asks.clone();

        // The driver's futures are not Send, so it keeps a thread.
        let thread = std::thread::Builder::new()
            .name("wellfed-update".into())
            .stack_size(STACK)
            .spawn(move || {
                let updates = Arc::new(updates);
                let heard = updates.clone();
                *asks.deliver() = Some(Box::new(move |update| match update {
                    InstallUpdate::Ask { id, ask } => {
                        heard.add(UpdateProgress::Ask { id, ask }).is_ok()
                    }
                    _ => false,
                }));
                let report = |progress| {
                    // Nobody listening: the screen went, and the feeder still
                    // wants its chunks.
                    let _ = updates.add(progress);
                };
                // A panic says nothing of an offer that may be out.
                let end = guarded(
                    || match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime.block_on(update(
                            &keys,
                            feeder,
                            address.as_deref(),
                            &image_dir,
                            asks.clone(),
                            &report,
                        )),
                        Err(error) => UpdateEnd::Failed {
                            detail: error.to_string(),
                        },
                    },
                    |_| UpdateEnd::Uncertain,
                );
                report(UpdateProgress::Ended { end });
                *asks.deliver() = None;
            });
        // With no thread the sink dropped with the closure, which closes Dart's
        // stream: the screen reads that as a run with no end.
        drop(thread);
    }
}

async fn update(
    keys: &Keypair,
    feeder: [u8; 32],
    address: Option<&str>,
    image_dir: &str,
    asks: Arc<Asks>,
    report: &impl Fn(UpdateProgress),
) -> UpdateEnd {
    match try_update(keys, feeder, address, image_dir, asks, report).await {
        Ok(end) => end,
        Err(detail) => UpdateEnd::Failed { detail },
    }
}

async fn try_update(
    keys: &Keypair,
    feeder: [u8; 32],
    address: Option<&str>,
    image_dir: &str,
    asks: Arc<Asks>,
    report: &impl Fn(UpdateProgress),
) -> Result<UpdateEnd, String> {
    // The image is the one this build pinned, or nothing goes out.
    let ImageCheck::Ready { version, .. } = check_image(Path::new(image_dir), pin().as_ref())
    else {
        return Err("this app's image did not pass its check".to_string());
    };
    let last = Cell::new(address.and_then(|address| wellfed_connect::parse_address(address).ok()));
    let discovery = AppDiscovery(asks);
    let review = Review::open(Path::new(image_dir)).map_err(|error| error.to_string())?;
    let manifest = std::fs::read(Path::new(image_dir).join(UPDATE_MANIFEST))
        .map_err(|error| error.to_string())?;
    let payload = review.payload().map_err(|error| error.to_string())?;
    let served = review.build().to_owned();

    // Where it last answered, then where NSD says: a restart can move it.
    let connect = async || -> Result<Connection, wellfed_install::Error> {
        if let Some(address) = last.get() {
            let ephemeral = wellfed_install::session::random()?;
            if let Ok(session) = Connection::open(keys, feeder, address, ephemeral).await {
                return Ok(session);
            }
        }
        let found = discovery
            .locate(feeder)
            .await
            .ok_or_else(|| wellfed_connect::Error::NotFound(hex::encode(&feeder[..8])))?;
        let ephemeral = wellfed_install::session::random()?;
        let session = Connection::open(keys, feeder, found, ephemeral).await?;
        last.set(Some(found));
        Ok(session)
    };
    let events = |event: Event| match event {
        Event::Written { written, length } => report(UpdateProgress::Sending { written, length }),
        Event::SessionDropped { .. } => report(UpdateProgress::Reconnecting),
        Event::Activated | Event::AlreadyActivated => report(UpdateProgress::Activated),
        _ => {}
    };

    // The driver's own failure may come after the offer went out, and it does
    // not say which, so it cannot be called nothing.
    let Ok(pushed) = push(
        Image {
            manifest: &manifest,
            downgrade: None,
            payload,
        },
        connect,
        |bank, build| bank == 1 && build.eq_ignore_ascii_case(&served),
        &events,
    )
    .await
    else {
        return Ok(UpdateEnd::Uncertain);
    };

    Ok(match pushed {
        Pushed::Activated => confirm(&connect, &served, version).await,
        Pushed::AlreadyRunning => UpdateEnd::AlreadyRunning,
        Pushed::Failed(InstallError::Refused(refusal)) => UpdateEnd::Refused {
            refusal: refusal_of(refusal),
        },
        Pushed::Failed(InstallError::Stopped(stop)) => UpdateEnd::Stopped { stop },
        Pushed::Failed(InstallError::Lost | InstallError::Unexpected) => UpdateEnd::Lost,
        Pushed::Gone { offered: true } => UpdateEnd::Uncertain,
        Pushed::Gone { offered: false } => UpdateEnd::NotOffered,
    })
}

/// Until the new version runs and has confirmed itself, or the feeder shows it
/// gave way to bank 0. These are reads, so looking again sends nothing it could act on.
async fn confirm(
    connect: &impl AsyncFn() -> Result<Connection, wellfed_install::Error>,
    served: &str,
    version: u32,
) -> UpdateEnd {
    tokio::time::sleep(RESTART).await;
    let deadline = Instant::now() + CONFIRM_WAIT;
    while Instant::now() < deadline {
        if let Some((status, view)) = reading(connect).await {
            if status.bank == 0 || !status.build.eq_ignore_ascii_case(served) {
                return UpdateEnd::WentBack;
            }
            if view.min_version >= version || status.readiness == Some(Readiness::Ready) {
                return UpdateEnd::Confirmed;
            }
        }
        tokio::time::sleep(POLL).await;
    }
    UpdateEnd::Unconfirmed
}

/// What a fresh session says it runs, or `None` while it cannot say.
async fn reading(
    connect: &impl AsyncFn() -> Result<Connection, wellfed_install::Error>,
) -> Option<(Status, UpdateView)> {
    let mut session = connect().await.ok()?;
    let id = wellfed_install::session::random().ok()?;
    let Answer::Status(status) = session.ask(id, &Op::Status).await.ok()? else {
        return None;
    };
    let id = wellfed_install::session::random().ok()?;
    let Answer::Update(view) = session.ask(id, &Op::UpdateState).await.ok()? else {
        return None;
    };
    Some((status, view))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check comes before the first connection and the first ask.
    #[test]
    fn an_update_without_a_pinned_image_sends_nothing() {
        let empty = tempfile::tempdir().unwrap();
        let keys = Keypair::from_secret([7; 32]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let end = runtime.block_on(update(
            &keys,
            [0; 32],
            None,
            &empty.path().to_string_lossy(),
            Arc::new(Asks::new()),
            &|_| panic!("nothing to report"),
        ));

        assert!(matches!(end, UpdateEnd::Failed { .. }));
    }
}
