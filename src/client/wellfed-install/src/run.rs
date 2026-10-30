//! The run: `wellfed_client::install`'s steps, each answered with its IO.
//!
//! The steps make every decision; this module only does what each one
//! asks and tells the steps what came of it. Nothing is retried that the
//! steps say must not be: the install request goes once, and a refusal or
//! an uncertain outcome ends the run; so does the update key's enrollment,
//! which an unanswered one judges by the signer list.
//!
//! The put of stock's plans goes once in a run, an unanswered one judged by
//! reading the plans back. Its base is on disk before it leaves, so a later run
//! knows one may have left, and sends it again only if the plans' generation
//! still equals that base; otherwise it stops with nothing sent. An empty table
//! is no proof, since the owner may have emptied it; every change moves the
//! generation. A first put arriving late is refused by the same base.
//!
//! Reads (a status, a session opening) are tried again until their wait
//! runs out, since they change nothing.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use wellfed_client::install::{
    CONTAINER_HEADER, FeederTrust, Install, InstallError, Options, PairRoute, PlansPut, Promotion,
    Provision, Replacement, Requested, Step, StockBackup, StockNotReady, UpdateKey, plans,
};
use wellfed_client::setup::Credentials;
use wellfed_client::update::{Image, InstallError as UpdateError};
use wellfed_client::update_key::{EnrollError, Enrolled};
use wellfed_formats::bench::prompt::Prompt;
use wellfed_formats::manifest::Provisioning;
use wellfed_formats::stock::TrialPhase;
use wellfed_model::{Answer, Generation, Op, PLANS, Plan as FeedingPlan, Plans};
use wellfed_proto::api::PORT;
use wellfed_signing::{Frozen, Key, Motion, highest_signed};
use wellfed_stock::ota::trial::{AnsweredInput, Input};
use wellfed_stock::server::Refusal;

use crate::event::{Enrollment, Event, PlansResult, Wait};
use crate::platform::{Bluetooth, Discovery, Keys, Prompter, Report};
use crate::push::{Pushed, push};
use crate::review::Review;
use crate::stock::{Stock, StockOptions, Unready, latest_report, view};
use crate::track::{Stop, Track, Unsettled};
use crate::{Error, backup, pairing, session::random, storage, update_key};

/// How often the run looks at stock's status and the replacement.
const POLL: Duration = Duration::from_secs(5);
/// Stock joins the owner's Wi-Fi and reports within a minute or two of the
/// frame; the enrollment window bounds this wait.
const STOCK_GRACE: Duration = Duration::from_secs(5 * 60);
/// Stock reports its version in the burst it sends as it connects, and
/// otherwise only heartbeats. The broker holds an enrolling feeder's
/// CONNACK until the server subscribed, so the burst reaches it; past this,
/// should it still be lost (the server slower than the broker's bound), a
/// restart replays it, once.
const VERSION_WAIT: Duration = Duration::from_secs(90);
/// Stock downloads in about 40 s, then restarts; generous, as on the bench.
const INSTALL_WAIT: Duration = Duration::from_secs(10 * 60);
/// From stock's restart until the replacement answers the device API.
const REPLACEMENT_WAIT: Duration = Duration::from_secs(5 * 60);
/// The stock server refuses to close its record while stock's telemetry is
/// current (90 s after its last report).
const CLOSE_WAIT: Duration = Duration::from_secs(150);
/// Bank 1's trial boot confirms within a few minutes of activation.
const CONFIRM_WAIT: Duration = Duration::from_secs(6 * 60);
/// Reading the plans back may wait for a session to open again.
const PLANS_WAIT: Duration = Duration::from_secs(60);
/// From the end of Bluetooth setup until the feeder, rebooted onto the
/// owner's Wi-Fi, answers the device API.
const JOIN_WAIT: Duration = Duration::from_secs(5 * 60);
/// How long the setup frame's answer is waited for; it is never resent. Stock
/// gives its broker about 20 s once it joins the Wi-Fi, and a phone with one
/// Wi-Fi radio cannot go home until this is over.
const FRAME_ANSWER: Duration = Duration::from_secs(2);
/// From the frame until stock reaches the broker: its Wi-Fi join and those
/// 20 s, with room. Past this it has gone back to its setup network.
const FRAME_ARRIVAL: Duration = Duration::from_secs(2 * 60);
/// How long the backup's read keeps coming back to a feeder that dropped it.
const BACKUP_RECONNECT: Duration = Duration::from_secs(3 * 60);
/// How the feeder lists the installer's update key.
const UPDATE_KEY_LABEL: &str = "installer";

/// Where an install starts.
#[derive(Debug, Clone)]
pub enum Start {
    /// From stock, reached as `Provision` says: by a DNS override, the
    /// setup frame, or already.
    Stock(Provision),
    /// The replacement is already on bank 0 from an earlier run: start at
    /// pairing, and ask stock nothing.
    Bank0,
}

/// What a run does about a feeder an earlier run gave Wi-Fi to, when it does not
/// answer on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unjoined {
    /// Wait for it to join: it may be restarting still.
    Wait,
    /// Go through Bluetooth setup again with `Plan::home`: the owner says the
    /// Wi-Fi it was given was wrong, and has put it back in setup mode.
    SetUpAgain,
}

/// What the promotion into bank 1 is signed with.
pub enum Signer {
    /// An update manifest for the build, signed elsewhere at `version`,
    /// promoted as signed. Its signer is the manifest's own: no key is
    /// enrolled for it.
    Manifest { manifest: Vec<u8>, version: u32 },
    /// The verified review signed here with `key`, at a version above the
    /// feeder's floor and above `above` (the highest this owner signed so
    /// far); `key` is enrolled first unless the image builds it in.
    Key {
        key: Box<Key>,
        frozen: Box<Frozen>,
        above: u32,
    },
}

/// Everything a run needs, read and checked before the feeder is touched.
pub struct Plan {
    pub review: Review,
    pub start: Start,
    pub signer: Signer,
    /// The owner's Wi-Fi, which Bluetooth setup gives a consumer image.
    pub home: Option<Credentials>,
    /// A copy of stock 1.1.15's image the owner holds, kept beside the
    /// backups before the promotion once the feeder's identity is known.
    pub backup: Option<StockBackup>,
    /// Promote even if stock's stored plans cannot be read.
    pub without_stock_plans: bool,
    /// Stock's MQTT client ID as the owner already knows it: the feeder
    /// check passes without asking for exactly this device, and stops the
    /// run for another.
    pub expect_device: Option<String>,
    /// The feeder's identity, as a hex prefix, when the keys know more
    /// than one.
    pub feeder: Option<String>,
    /// The replacement's address; where it answered or stock connected
    /// from otherwise.
    /// What to do about a paired feeder that never joined.
    pub unjoined: Unjoined,
    pub address: Option<SocketAddr>,
    /// The owner's data directory: one subdirectory per feeder, and the
    /// resume record ([`crate::storage`]).
    pub backups: PathBuf,
    pub stock: StockOptions,
}

/// What the platform gives the run.
pub struct Platform<P, B, D, K, R> {
    pub prompter: P,
    pub bluetooth: B,
    pub discovery: D,
    pub keys: K,
    pub report: R,
}

/// Run `plan` to its end, until `stop` resolves, or until the steps end it.
/// However it ends, stock's server stops before this returns. A stop ends
/// it with [`Error::Stopped`], which names the step and any write whose
/// outcome had not settled.
///
/// The returned future is not `Send` (rustc cannot prove it across a
/// generic state machine this deep), so a platform drives it with
/// `block_on`, on a thread of its own: the CLI on its main thread, the app
/// on a thread it starts for the install. Stock's server and image server,
/// which it spawns, are `Send` tasks on that same runtime: on its workers for
/// the CLI's, and between the run's own awaits on the app's current-thread one.
pub async fn run<P, B, D, K, R>(
    plan: Plan,
    platform: Platform<P, B, D, K, R>,
    stop: impl Future<Output = Stop>,
) -> Result<(), Error>
where
    P: Prompter,
    B: Bluetooth,
    D: Discovery,
    K: Keys,
    R: Report,
{
    let (mut run, mut install) = Run::new(plan, platform)?;
    let ended = tokio::select! {
        result = run.drive(&mut install) => Ok(result),
        by = stop => Err(by),
    };
    run.stock.stop().await;

    match ended {
        Ok(result) => result,
        Err(by) => Err(Error::Stopped(run.track.stopped(by))),
    }
}

/// Where the run looks for a feeder by its identity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    /// Only where this run knows its feeder is: the given address, or
    /// where stock connected from.
    Known,
    /// Over the platform's discovery too: for an identity this run learned
    /// from the feeder itself, which no other feeder answers to.
    Discovered,
}

/// Where a run looks for a feeder this client's key already opens,
/// before Bluetooth setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lookup {
    /// This install's own record: the identity its feeder proved, looked
    /// for where it last answered, where stock connected from and by
    /// discovery, where only that identity completes a session. Its feeder
    /// not answering stops the run, since it knows this key and setup would
    /// not start; unless it never answered yet, when it may still be joining.
    Recorded {
        identity: [u8; 32],
        last: Option<SocketAddr>,
    },
    /// The key's known feeder, which may be another of the owner's: tried
    /// only where this run knows its feeder is, never by discovery, and
    /// setup follows if it does not answer.
    KeyFile { identity: [u8; 32] },
    /// Nothing to try: Bluetooth setup.
    Setup,
}

/// The lookup for a run whose broker enrolled `device`: the record when it
/// is this install's (and `prefix`, if given, names its feeder), else the
/// keys' feeder, else setup.
fn lookup(
    record: Option<&storage::Paired>,
    device: Option<&str>,
    prefix: Option<&str>,
    known: Option<[u8; 32]>,
) -> Lookup {
    let ours = record.filter(|record| {
        let identity = wellfed_formats::hex::encode(&record.identity.0);
        device == Some(record.device.as_str()) && prefix.is_none_or(|p| identity.starts_with(p))
    });
    match (ours, known) {
        (Some(record), _) => Lookup::Recorded {
            identity: record.identity.0,
            last: record.address,
        },
        (None, Some(identity)) => Lookup::KeyFile { identity },
        (None, None) => Lookup::Setup,
    }
}

/// What a run does after its lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Looked {
    /// Paired already, and answering there.
    Paired([u8; 32], SocketAddr),
    /// Paired over Bluetooth by a run that never saw it join: it may be
    /// restarting still, so this run waits as that one would have.
    Rejoining([u8; 32]),
    /// Bluetooth setup.
    SetUp,
}

/// What a lookup that found its feeder `answered` (or not) means. This
/// install's own feeder no longer answering is a stop, unless it never
/// answered yet.
fn looked_up(
    lookup: Lookup,
    answered: Option<SocketAddr>,
    unjoined: Unjoined,
) -> Result<Looked, Error> {
    match (lookup, answered) {
        (Lookup::Recorded { identity, .. } | Lookup::KeyFile { identity }, Some(address)) => {
            Ok(Looked::Paired(identity, address))
        }
        (
            Lookup::Recorded {
                identity,
                last: None,
            },
            None,
        ) => Ok(match unjoined {
            Unjoined::Wait => Looked::Rejoining(identity),
            Unjoined::SetUpAgain => Looked::SetUp,
        }),
        (Lookup::Recorded { identity, .. }, None) => Err(Error::NotAnswering { feeder: identity }),
        (Lookup::KeyFile { .. } | Lookup::Setup, _) => Ok(Looked::SetUp),
    }
}

/// What the one request came to, and why when the server refused it: refused
/// only on the server's word and a ledger read that shows no record; anything
/// unread or unanswered is uncertain, since a refused call may still have
/// recorded the request (a dropped broker session marks it uncertain).
fn judged<T, E>(
    answered: &Result<T, Refusal>,
    recorded: &Result<Option<TrialPhase>, E>,
) -> (Requested, Option<Refusal>) {
    match (answered, recorded) {
        (Ok(_), _) | (_, Ok(Some(TrialPhase::Submitted))) => (Requested::Sent, None),
        (Err(refusal @ (Refusal::Contested | Refusal::Rejected(_))), Ok(None)) => {
            (Requested::Refused, Some(*refusal))
        }
        _ => (Requested::Uncertain, None),
    }
}

/// The error a failed step ends the run with: a refused request keeps the
/// server's reason.
fn failure(error: InstallError, refused: Option<Refusal>) -> Error {
    match (error, refused) {
        (InstallError::RequestRefused, Some(refusal)) => Error::RequestRefused(refusal),
        (error, _) => Error::Steps(error),
    }
}

/// Whether a session answering from `bank` with `build` shows the
/// promotion already activated. During a promotion that started on bank 0,
/// bank 1 can only run the served build once bank 0 activated it.
fn activated(bank: u8, build: &str, served: &str) -> bool {
    bank == 1 && build.eq_ignore_ascii_case(served)
}

/// Whether the owner recognizes the feeder the broker trusted. Trust on
/// first use admits whichever stock-shaped client reached the broker first;
/// a host on the LAN that won that race could otherwise be walked through
/// the whole install, so the owner checks the device ID (on the feeder's
/// label or in the vendor app) and the address (the router's client list)
/// before anything is sent.
///
/// With `expected` the owner has answered ahead: the device they pinned
/// passes without a question, and any other stops the run. The address is
/// then not checked by anyone, which the owner accepts by pinning: the
/// device ID is what stock's credentials are bound to.
async fn recognized(
    prompter: &mut impl Prompter,
    report: &impl Report,
    trust: FeederTrust,
    expected: Option<&str>,
) -> Result<bool, Error> {
    let (device, address) = match trust {
        FeederTrust::Enrolled { device, address } => (device, address),
        // The bench: its credentials came from a capture of this feeder.
        FeederTrust::Captured => return Ok(true),
        FeederTrust::Contested => return Ok(false),
    };
    match expected {
        Some(expected) => {
            wellfed_formats::require(expected == device, || Error::OtherDevice {
                expected: expected.to_string(),
                enrolled: device.clone(),
                address: address.clone(),
            })?;
            report.event(Event::DevicePinned { device, address });
            Ok(true)
        }
        None => prompter
            .yes(Prompt::RecognizeFeeder { device, address })
            .await
            .map_err(Error::Platform),
    }
}

/// Guide the owner through the DNS override, then look the name up from
/// here and show what it found, before the feeder restarts. The lookup is
/// advice only: this computer may ask another resolver than the feeder
/// does, so the run waits for stock at the broker whatever it shows.
async fn point_dns(
    prompter: &mut impl Prompter,
    name: String,
    broker: IpAddr,
) -> Result<Vec<IpAddr>, Error> {
    let port = wellfed_client::install::STOCK_BROKER_PORT;
    let lookup = format!("{name}:{port}");
    prompter
        .done(Prompt::SetOverride { name, broker })
        .await
        .map_err(Error::Platform)?;

    Ok(tokio::net::lookup_host(lookup)
        .await
        .map(|addresses| addresses.map(|address| address.ip()).collect())
        .unwrap_or_default())
}

/// Ask the owner to restart the feeder, with the broker already listening.
async fn restart_feeder(prompter: &mut impl Prompter, report: &impl Report) -> Result<(), Error> {
    prompter
        .done(Prompt::RestartFeeder)
        .await
        .map_err(Error::Platform)?;
    report.event(Event::AwaitingStock);
    Ok(())
}

/// Keep the put's base, then send the put. Nothing else sends it, so a put on the
/// wire means its base is on disk, and no base means no put left; if the base
/// cannot be kept, the put does not go out.
async fn put_once(
    connection: &mut wellfed_connect::Connection,
    kept: &std::path::Path,
    base: Generation,
    plans: [Option<FeedingPlan>; PLANS],
) -> Result<PlansPut, Error> {
    storage::keep_put_base(kept, base).map_err(|error| Error::KeepPutBase(Box::new(error)))?;

    let op = Op::PutPlans { base, plans };
    Ok(match connection.ask(random()?, &op).await {
        Ok(Answer::Done { .. }) => PlansPut::Done,
        Ok(Answer::Refused(refusal)) => PlansPut::Refused(refusal),
        Ok(_) | Err(_) => PlansPut::Unanswered,
    })
}

/// Unix milliseconds now, which decides whether a stock skip still holds.
fn now_ms() -> Result<u64, Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| Error::Clock)?;
    u64::try_from(now.as_millis()).map_err(|_| Error::Clock)
}

/// The run's IO state beside the steps.
struct Run<P, B, D, K, R> {
    io: Platform<P, B, D, K, R>,
    review: Review,
    stock: Stock,
    /// The signed update manifest: given, or once the run signed it.
    manifest: Option<Vec<u8>>,
    /// The verified review and the key, when the run signs the promotion
    /// itself; the key is also the one it enrolls.
    signing: Option<(Box<Frozen>, Box<Key>)>,
    home: Option<Credentials>,
    expect_device: Option<String>,
    feeder: Option<String>,
    address: Option<SocketAddr>,
    unjoined: Unjoined,
    backups: PathBuf,
    /// The one request's id, once made.
    request: Option<uuid::Uuid>,
    /// Where the feeder connected from (the image lease's peer).
    peer: Option<IpAddr>,
    /// Why the server refused the request, when it did.
    refused: Option<Refusal>,
    /// Where the feeder answered after joining the owner's Wi-Fi, or where
    /// a lookup found it.
    found: Option<SocketAddr>,
    /// The feeder's identity, once its key is known.
    identity: Option<[u8; 32]>,
    /// The owner's stock copy, kept before promotion once the feeder's
    /// identity is known.
    given: Option<StockBackup>,
    /// Where the run is, for the report of a stop that ends it.
    track: Track,
    /// When this run's setup frame went out.
    frame_out: Option<Instant>,
}

impl<P, B, D, K, R> Run<P, B, D, K, R>
where
    P: Prompter,
    B: Bluetooth,
    D: Discovery,
    K: Keys,
    R: Report,
{
    fn new(plan: Plan, io: Platform<P, B, D, K, R>) -> Result<(Self, Install), Error> {
        let route = match plan.review.provisioning() {
            Provisioning::Consumer => PairRoute::Setup,
            Provisioning::Bench => PairRoute::Recovery,
        };
        let (manifest, promotion, update_key, signing) = match plan.signer {
            Signer::Manifest { manifest, version } => (
                Some(manifest),
                Promotion::Signed(version),
                UpdateKey::BuiltIn,
                None,
            ),
            Signer::Key { key, frozen, above } => {
                let update_key = if plan.review.builds_in(&key) {
                    UpdateKey::BuiltIn
                } else {
                    UpdateKey::Enroll(key.public())
                };
                (
                    None,
                    Promotion::Sign { above },
                    update_key,
                    Some((frozen, key)),
                )
            }
        };
        let options = Options {
            route,
            update_key,
            promotion,
        };

        let build = plan.review.build().to_string();
        let mut install = match plan.start {
            Start::Stock(provision) => Install::new(provision, &build, options)?,
            Start::Bank0 => Install::from_bank0(&build, options),
        };
        if plan.without_stock_plans {
            install.without_stock_plans();
        }
        if let Some(backup) = &plan.backup {
            install.backup(backup.verified().ok_or(Error::Unverified)?);
        }

        let run = Self {
            io,
            review: plan.review,
            stock: Stock::new(plan.stock),
            manifest,
            signing,
            home: plan.home,
            expect_device: plan.expect_device,
            feeder: plan.feeder,
            address: plan.address,
            unjoined: plan.unjoined,
            backups: plan.backups,
            request: None,
            peer: None,
            refused: None,
            found: None,
            identity: None,
            given: plan.backup,
            track: Track::new(),
            frame_out: None,
        };
        Ok((run, install))
    }

    async fn drive(&mut self, install: &mut Install) -> Result<(), Error> {
        loop {
            let step = install.step();
            self.track.enter(install.phase());
            match step {
                Step::PointDns(dns) => {
                    let (name, broker) = (dns.name().to_string(), dns.broker());
                    let resolved = point_dns(&mut self.io.prompter, name.clone(), broker).await?;
                    self.io.report.event(Event::DnsChecked {
                        name,
                        broker,
                        check: dns.check(&resolved),
                    });
                    // The enrollment window opens only now, just before
                    // stock restarts into it.
                    self.serve()?;
                    restart_feeder(&mut self.io.prompter, &self.io.report).await?;
                    install.dns_pointed();
                }
                Step::ConfirmReset => {
                    // Warned up front: the Reset hold erases stock's plans
                    // before the installer can read them.
                    let accepted = self
                        .io
                        .prompter
                        .yes(Prompt::EraseStockPlans)
                        .await
                        .map_err(Error::Platform)?;
                    if accepted {
                        install.reset_accepted();
                    } else {
                        install.reset_declined();
                    }
                }
                Step::SendFrame(frame) => {
                    self.serve()?;
                    self.send_frame(frame.bytes()).await?;
                    self.frame_out = Some(Instant::now());
                    install.frame_sent();
                }
                Step::AwaitStock => {
                    self.serve()?;
                    self.await_stock(install).await?;
                }
                Step::ConfirmFeeder(trust) => {
                    let expected = self.expect_device.as_deref();
                    if recognized(&mut self.io.prompter, &self.io.report, trust, expected).await? {
                        install.feeder_confirmed();
                    } else {
                        install.feeder_rejected();
                    }
                }
                Step::RequestInstall => {
                    let outcome = self.request().await?;
                    install.requested(outcome);
                }
                Step::AwaitInstall => self.await_install(install).await,
                Step::Pair(route) => self.pair(install, route).await?,
                Step::AwaitJoin => self.await_join(install).await?,
                Step::ReadStatus => {
                    self.kept_promotion(install);
                    let (build, bank) = self.status().await?;
                    self.io.report.event(Event::ReplacementAnswers {
                        build: build.clone(),
                        bank,
                    });
                    install.replacement(Replacement {
                        build: &build,
                        bank,
                    });
                }
                Step::CloseTrial => {
                    self.close_trial(install.build()).await;
                    install.trial_closed();
                }
                Step::EnrollKey(_) => {
                    let outcome = self.enroll_key().await?;
                    install.key_enrolled(outcome);
                }
                Step::AwaitEnroll => return Err(Error::EnrollmentEnded),
                Step::CheckSigners => self.check_signers(install).await,
                Step::Backup => {
                    let backup = self
                        .backup()
                        .await
                        .map_err(|error| Error::Backup(Box::new(error)))?;
                    install.backup(backup.verified().ok_or(Error::Unverified)?);
                }
                Step::ReadStockPlans => self.stock_plans(install).await?,
                Step::ReadFloor => self.read_floor(install).await,
                Step::Sign { version } => {
                    self.sign(version)?;
                    install.signed();
                }
                Step::Promote => {
                    self.keep_given()?;
                    let outcome = self.promote(install.build()).await?;
                    install.promoted(outcome);
                }
                Step::AwaitConfirm => {
                    self.kept_plans(install)?;
                    self.earlier_put(install);
                    self.await_confirm(install).await?;
                }
                Step::ReadPlans => self.read_plans(install).await,
                Step::PutPlans { base, plans } => {
                    let outcome = self.put_plans(base, plans).await?;
                    install.plans_put(outcome);
                }
                Step::AwaitPut => return Err(Error::PutEnded),
                Step::Done => {
                    self.done(install)?;
                    return Ok(());
                }
                Step::Failed(error) => return Err(failure(error, self.refused.take())),
            }
        }
    }

    /// The end: what was installed, and what became of stock's plans.
    fn done(&mut self, install: &Install) -> Result<(), Error> {
        let report = &self.io.report;
        report.event(Event::Installed {
            build: install.build().to_string(),
        });
        let result = match (install.import(), install.plans_skipped()) {
            (_, Some(skipped)) => PlansResult::Skipped(skipped.clone()),
            (None, None) => PlansResult::NotRead,
            (Some(import), None) if import.plans.iter().any(Option::is_some) => PlansResult::Set,
            (Some(import), None) if import.outcomes.is_empty() => PlansResult::None,
            (Some(_), None) => PlansResult::NoneConverted,
        };
        let applied = result == PlansResult::Set;
        report.event(Event::Plans(result));
        if applied {
            self.kept_plans_applied()?;
        }
        // Last, and never a failure: the feeder is installed, and a record
        // left behind only keys a rerun of this same install to the feeder
        // it already confirmed. The put's base goes after the kept plans it
        // judges, which a rerun cannot put without.
        let cleared = self.clear_paired().and_then(|()| match self.identity {
            Some(identity) => {
                storage::forget_put_base(&storage::put_base_path(&self.backups, &identity))
            }
            None => Ok(()),
        });
        if let Err(error) = cleared {
            self.io.report.event(Event::ResumeRecordKept {
                error: crate::text(&error),
            });
        }
        Ok(())
    }

    /// Stock's server with the trial enabled and the enrollment window
    /// open; started once, before stock is pointed at it.
    fn serve(&mut self) -> Result<(), Error> {
        if self.stock.start()? {
            self.io.report.event(Event::StockServerStarted);
        }
        Ok(())
    }

    async fn send_frame(&mut self, frame: &[u8]) -> Result<(), Error> {
        // Reset held about 5 s on an unlocked stock panel starts its factory
        // restore, which leaves stock in setup mode (observed).
        self.io
            .prompter
            .done(Prompt::SetupMode)
            .await
            .map_err(Error::Handover)?;
        let to = wellfed_client::install::SETUP_ADDRESS;
        let socket = tokio::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
            .await
            .map_err(Error::UdpSocket)?;
        socket
            .send_to(frame, to)
            .await
            .map_err(|source| Error::FrameSend { to, source })?;
        let mut answer = [0; 256];
        let answer = match tokio::time::timeout(FRAME_ANSWER, socket.recv_from(&mut answer)).await {
            Ok(Ok((n, _))) => answer.get(..n).map(<[u8]>::to_vec),
            _ => None,
        };
        self.io.report.event(Event::FrameSent { answer });
        self.io
            .prompter
            .done(Prompt::JoinBack)
            .await
            .map_err(Error::Handover)
    }

    /// Until stock reports ready, or the window and its grace run out.
    async fn await_stock(&mut self, install: &mut Install) -> Result<(), Error> {
        let deadline = Instant::now() + self.stock.enroll() + STOCK_GRACE;
        let mut said = None;
        let mut silent_since = None;
        let mut restarted = false;
        let mut arrived = false;
        loop {
            let wait = match self.stock.status().await {
                Ok(status) => match install.stock(&view(&status)) {
                    Ok(()) => return Ok(()),
                    Err(reason) => Wait::Stock(reason),
                },
                Err(Unready::NoFeeder) => Wait::NoFeeder,
                Err(Unready::Unreadable(error)) => Wait::Unreadable { error },
                // Nothing will report now: the window closed with no feeder.
                Err(Unready::WindowClosed(error)) => {
                    self.io.report.event(Event::StockServerEnded { error });
                    install.gave_up();
                    return Ok(());
                }
                // Not the same as no feeder coming: its port was taken, say.
                Err(Unready::Failed(error)) => {
                    self.io.report.event(Event::StockServerEnded {
                        error: error.clone(),
                    });
                    return Err(Error::StockServerFailed(error));
                }
            };

            // Once only: a second silence is not the lost burst.
            if wait != Wait::Stock(StockNotReady::NoVersion) {
                silent_since = None;
            } else if !restarted
                && silent_since.get_or_insert_with(Instant::now).elapsed() >= VERSION_WAIT
            {
                self.io.report.event(Event::VersionLost);
                restart_feeder(&mut self.io.prompter, &self.io.report).await?;
                restarted = true;
                silent_since = None;
            }

            // Enrolled by an earlier run, a feeder that never took this frame
            // reads as offline, not as no feeder.
            let absent = matches!(wait, Wait::NoFeeder | Wait::Stock(StockNotReady::Offline));
            arrived |= !absent && !matches!(wait, Wait::Unreadable { .. });
            if said.as_ref() != Some(&wait) {
                self.io.report.event(Event::Waiting(wait.clone()));
                said = Some(wait);
            }
            if !arrived
                && self
                    .frame_out
                    .is_some_and(|sent| sent.elapsed() >= FRAME_ARRIVAL)
            {
                install.frame_not_taken();
                return Ok(());
            }
            if Instant::now() >= deadline {
                install.gave_up();
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Serve the image and ask once. What the server answered is read back
    /// from its ledger ([`judged`]).
    async fn request(&mut self) -> Result<Requested, Error> {
        let lease = self.stock.images(&self.review.directory).await?;
        let id = uuid::Uuid::new_v4();
        self.request = Some(id);
        self.peer = Some(IpAddr::V4(lease.peer));

        self.io.report.event(Event::Requesting {
            build: lease.build.clone(),
            request: id,
        });
        let input = Input {
            request_id: id,
            server_token: lease.token,
        };
        self.track.sent(Unsettled::Request(id));
        let answered = self.stock.trial(input).await;

        let recorded = self
            .stock
            .status()
            .await
            .map(|status| status.trial(&id.to_string()).map(|trial| trial.phase));
        let (requested, refused) = judged(&answered, &recorded);
        self.refused = refused;
        Ok(requested)
    }

    /// Stock's reports on this run's request, until its session ends.
    async fn await_install(&mut self, install: &mut Install) {
        let id = self.request.map(|id| id.to_string()).unwrap_or_default();
        let deadline = Instant::now() + INSTALL_WAIT;
        let mut shown = None;
        while Instant::now() < deadline {
            if let Ok(status) = self.stock.status().await {
                let report = latest_report(&status, &id);
                if let Some(progress) = report.and_then(|report| report.progress) {
                    install.stock_progress(progress);
                }
                if install.progress() != shown {
                    shown = install.progress();
                    self.io
                        .report
                        .event(Event::StockProgress(shown.unwrap_or(0)));
                }
                match status.trial(&id).map(|trial| trial.phase) {
                    Some(TrialPhase::ReportedError) => {
                        install.stock_refused(report.and_then(|report| report.error_code));
                        return;
                    }
                    _ if !status.device_online && status.last_seen.is_some() => {
                        self.io.report.event(Event::StockRestarting);
                        install.stock_gone();
                        return;
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(POLL).await;
        }
        install.gave_up();
    }

    /// The replacement's address: given, where it answered, or where stock
    /// connected from, as the image lease or (a resumed run has no lease) the
    /// enrollment says.
    fn address(&self) -> Result<SocketAddr, Error> {
        if let Some(address) = self.address {
            return Ok(address);
        }
        if let Some(found) = self.found {
            return Ok(found);
        }
        let stock = self
            .peer
            .or_else(|| self.stock.leased_peer())
            .or_else(|| self.stock.enrolled_peer());
        match stock {
            Some(ip) => Ok(SocketAddr::new(ip, PORT)),
            None => Err(Error::NoAddress),
        }
    }

    /// Become the replacement's owner: a key it already knows answers at
    /// once; otherwise, by `route`, Bluetooth setup or the recovery window.
    async fn pair(&mut self, install: &mut Install, route: PairRoute) -> Result<(), Error> {
        match route {
            PairRoute::Recovery => self.recover(install).await,
            PairRoute::Setup => self.set_up(install).await,
        }
    }

    /// The owner opens the recovery window and keys in the code, unless the
    /// replacement knows this client's key. It may still be starting, so
    /// a session is tried until it answers.
    async fn recover(&mut self, install: &mut Install) -> Result<(), Error> {
        // A resumed run: where this install's feeder last answered.
        if self.found.is_none()
            && let Some(device) = self.stock.device()?
            && let Some(record) = storage::Paired::load(&self.backups)?
            && record.device == device
        {
            self.found = record.address;
        }
        let address = self.address()?;
        if let Some(identity) = self.io.keys.feeder(self.feeder.as_deref()) {
            let deadline = Instant::now() + REPLACEMENT_WAIT;
            loop {
                match wellfed_connect::Connection::open(
                    self.io.keys.keypair(),
                    identity,
                    address,
                    random()?,
                )
                .await
                {
                    Ok(_) => {
                        self.io.report.event(Event::KeyKnown);
                        self.identity = Some(identity);
                        self.record_paired(identity, Some(address))?;
                        install.paired();
                        return Ok(());
                    }
                    Err(wellfed_connect::Error::UnknownKey) => break,
                    Err(_) if Instant::now() < deadline => tokio::time::sleep(POLL).await,
                    // Another identity answers there (a store reset since),
                    // or none: the recovery window settles which.
                    Err(error) => {
                        self.io.report.event(Event::KnownKeyRefused {
                            error: error.to_string(),
                        });
                        break;
                    }
                }
            }
        }

        // The window is opened on a running replacement: wait for its API.
        let deadline = Instant::now() + REPLACEMENT_WAIT;
        while tokio::net::TcpStream::connect(address).await.is_err() {
            if Instant::now() >= deadline {
                install.gave_up();
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
        self.io
            .prompter
            .done(Prompt::OpenRecoveryWindow)
            .await
            .map_err(Error::Platform)?;
        let (identity, outcome) =
            pairing::recover(&mut self.io.keys, address, &mut self.io.prompter).await?;
        self.io.report.event(Event::Recovered { identity, outcome });
        self.identity = Some(identity);
        self.record_paired(identity, Some(address))?;
        install.paired();
        Ok(())
    }

    /// Bluetooth setup, unless the replacement already answers this
    /// client's key (a resumed run, the feeder on the owner's Wi-Fi). A
    /// consumer image with no network starts setup by itself, so the run
    /// looks for it advertising until it does; the exchange then goes once,
    /// with the code the owner keys in.
    async fn set_up(&mut self, install: &mut Install) -> Result<(), Error> {
        let device = self.stock.device()?;
        let record = storage::Paired::load(&self.backups)?;
        let prefix = self.feeder.as_deref();
        let lookup = lookup(
            record.as_ref(),
            device.as_deref(),
            prefix,
            self.io.keys.feeder(prefix),
        );
        let answered = match lookup {
            Lookup::Recorded { identity, last } => {
                self.answering(identity, Look::Discovered, last).await
            }
            Lookup::KeyFile { identity } => self.answering(identity, Look::Known, None).await,
            Lookup::Setup => None,
        };
        match looked_up(lookup, answered, self.unjoined)? {
            Looked::Paired(identity, address) => {
                self.io
                    .report
                    .event(Event::KeyKnownAt { identity, address });
                self.paired_at(identity, address)?;
                install.paired();
                return Ok(());
            }
            Looked::Rejoining(identity) => {
                self.io.report.event(Event::Rejoining { identity });
                self.identity = Some(identity);
                install.set_up();
                return Ok(());
            }
            Looked::SetUp => {}
        }

        let Some(wifi) = self.home.clone() else {
            return Err(Error::NoHomeNetwork);
        };
        self.io.report.event(Event::SetupStarting);
        let deadline = Instant::now() + REPLACEMENT_WAIT;
        let link = loop {
            match self.io.bluetooth.open().await {
                Ok(link) => break link,
                Err(error) if Instant::now() >= deadline => {
                    self.io.report.event(Event::NoSetupFeeder {
                        error: crate::text(&*error),
                    });
                    install.gave_up();
                    return Ok(());
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        };
        let (identity, outcome) =
            pairing::over_link(&mut self.io.keys, Some(wifi), link, &mut self.io.prompter)
                .await
                .map_err(|error| Error::Setup(Box::new(error)))?;
        // Whichever feeder advertises first takes setup, and the code only proves
        // the owner is at its panel. Setting this install's feeder up again must
        // not move the install, and its backup and promotion, onto another.
        if let Lookup::Recorded {
            identity: expected, ..
        } = lookup
            && expected != identity
        {
            return Err(Error::OtherFeeder {
                expected,
                found: identity,
            });
        }
        self.io.report.event(Event::SetUp { identity, outcome });
        self.identity = Some(identity);
        self.record_paired(identity, None)?;
        install.set_up();
        Ok(())
    }

    /// Until the feeder, rebooted after setup, answers on the owner's
    /// network: found by discovery by its identity, or where stock
    /// connected from (the same feeder on the same network).
    async fn await_join(&mut self, install: &mut Install) -> Result<(), Error> {
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let deadline = Instant::now() + JOIN_WAIT;
        while Instant::now() < deadline {
            if let Some(address) = self.answering(identity, Look::Discovered, None).await {
                self.io.report.event(Event::Joined { address });
                self.paired_at(identity, address)?;
                install.joined();
                return Ok(());
            }
            tokio::time::sleep(POLL).await;
        }
        install.gave_up();
        Ok(())
    }

    /// The paired feeder answered at `address`: use it, and record it.
    fn paired_at(&mut self, identity: [u8; 32], address: SocketAddr) -> Result<(), Error> {
        self.identity = Some(identity);
        self.found = Some(address);
        self.record_paired(identity, Some(address))
    }

    /// Record the paired feeder beside the backups, keyed to this install's
    /// stock device, as soon as pairing succeeds: a resumed run may find no
    /// image lease (the image server removes it when it stops), and this
    /// install is one to pick up from now on. With no enrolled device there
    /// is nothing to key it to, and nothing is recorded.
    fn record_paired(&self, identity: [u8; 32], address: Option<SocketAddr>) -> Result<(), Error> {
        let Some(device) = self.stock.device()? else {
            return Ok(());
        };
        storage::Paired {
            device,
            identity: wellfed_model::Hex(identity),
            address,
        }
        .keep(&self.backups)
    }

    /// The install is done: its record is no longer anyone's to resume.
    fn clear_paired(&self) -> Result<(), Error> {
        match self.stock.device()? {
            Some(device) => storage::Paired::clear(&self.backups, &device),
            None => Ok(()),
        }
    }

    /// Where the feeder with `identity` opens a session with this client's
    /// key now, if anywhere: the given address; else `last` (where it
    /// answered before), then (by `look`) by discovery, then where it
    /// answered this run or stock connected from. Opening a session sends
    /// nothing.
    async fn answering(
        &self,
        identity: [u8; 32],
        look: Look,
        last: Option<SocketAddr>,
    ) -> Option<SocketAddr> {
        let mut candidates = Vec::new();
        if self.address.is_none() {
            candidates.extend(last);
        }
        if self.address.is_none() && look == Look::Discovered {
            candidates.extend(self.io.discovery.locate(identity).await);
        }
        candidates.extend(self.address().ok());
        for address in candidates {
            let opened = wellfed_connect::Connection::open(
                self.io.keys.keypair(),
                identity,
                address,
                random().ok()?,
            )
            .await;
            if opened.is_ok() {
                return Some(address);
            }
        }
        None
    }

    /// Enroll the installer's update key once, the owner pressing Dispense.
    async fn enroll_key(&mut self) -> Result<Result<Enrolled, EnrollError>, Error> {
        let mut connection = self.session(PLANS_WAIT).await?;
        let Some((_, key)) = &self.signing else {
            return Err(Error::NoUpdateKey);
        };
        self.track.sent(Unsettled::UpdateKey);
        let outcome = update_key::enroll(
            &mut connection,
            key.public(),
            key.id(),
            UPDATE_KEY_LABEL,
            &mut self.io.prompter,
            &self.io.report,
        )
        .await?;
        let shown = match &outcome {
            Ok(enrolled) => Some(Enrollment::Enrolled(*enrolled)),
            Err(EnrollError::Uncertain) => Some(Enrollment::Unanswered),
            Err(_) => None,
        };
        if let Some(outcome) = shown {
            self.io.report.event(Event::KeyEnrollment {
                key: key.id(),
                outcome,
            });
        }
        Ok(outcome)
    }

    /// The feeder's clients after an unanswered enrollment; a read changes
    /// nothing, so a session is opened again until [`PLANS_WAIT`] runs out.
    async fn check_signers(&self, install: &mut Install) {
        let deadline = Instant::now() + PLANS_WAIT;
        loop {
            let read = async {
                let mut connection = self.connect().await?;
                update_key::clients(&mut connection).await
            };
            match read.await {
                Ok(clients) => return install.signers(&clients),
                Err(error) if Instant::now() >= deadline => {
                    self.io.report.event(Event::SignersUnread {
                        error: crate::text(&error),
                    });
                    return install.signers_unread();
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    /// The feeder's floor, its highest confirmed version.
    async fn read_floor(&self, install: &mut Install) {
        let deadline = Instant::now() + PLANS_WAIT;
        loop {
            let read = async {
                let mut connection = self.connect().await?;
                match connection.ask(random()?, &Op::UpdateState).await? {
                    Answer::Update(view) => Ok(view.min_version),
                    other => Err(Error::Answered {
                        request: "update state",
                        answer: Box::new(other),
                    }),
                }
            };
            match read.await {
                Ok(floor) => {
                    self.io.report.event(Event::Floor(floor));
                    return install.floor(floor);
                }
                Err(error) if Instant::now() >= deadline => {
                    self.io.report.event(Event::FloorUnread {
                        error: crate::text(&error),
                    });
                    return install.gave_up();
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    /// Sign the reviewed build at `version` with the update key, and keep
    /// the manifest beside the feeder's backup before it is offered: a run
    /// resumed after the activation reads its version there.
    fn sign(&mut self, version: u32) -> Result<(), Error> {
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let Some((frozen, key)) = &self.signing else {
            return Err(Error::NoSigner);
        };
        let manifest = frozen.offer(key, version, Motion::Refused)?;

        let path = storage::promotion_path(&self.backups, &identity, version);
        storage::write_new(&path, &manifest)?;
        self.io.report.event(Event::Signed {
            build: frozen.build_id(),
            version,
            key: key.id(),
            path,
        });
        self.manifest = Some(manifest.to_vec());
        Ok(())
    }

    /// The version an earlier run promoted for this feeder, from the
    /// manifests kept beside its backup, for a run that signs for itself
    /// and finds bank 1 already activated.
    ///
    /// The highest kept one is the right one: every run signs above all
    /// versions signed so far, and keeps its manifest before offering it,
    /// so the last promotion is always the highest. A lower kept manifest
    /// is an earlier promotion bank 1 has already confirmed past; and if
    /// the highest was kept but never activated, bank 1 confirming the
    /// served build cannot reach it, so the run waits and then times out
    /// rather than reporting a confirmation it did not see.
    fn kept_promotion(&self, install: &mut Install) {
        let Some(identity) = self.identity else {
            return;
        };
        let directory = storage::feeder_directory(&self.backups, &identity);
        if let Some((version, _)) = highest_signed(&directory) {
            install.promoted_before(version);
        }
    }

    /// A session with the paired feeder; opening one sends nothing, so it
    /// is tried again until `wait` runs out.
    async fn session(&self, wait: Duration) -> Result<wellfed_connect::Connection, Error> {
        let deadline = Instant::now() + wait;
        loop {
            match self.connect().await {
                Ok(connection) => return Ok(connection),
                Err(error) if Instant::now() >= deadline => {
                    return Err(Error::NoSession(Box::new(error)));
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    /// Keep the owner's own stock copy as a backup read from the feeder is
    /// kept, before the promotion overwrites the feeder's own.
    fn keep_given(&mut self) -> Result<(), Error> {
        let Some(backup) = self.given.take() else {
            return Ok(());
        };
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let path = storage::backup_path(&self.backups, &identity);
        storage::keep_backup(&path, &backup).map_err(|error| Error::KeepGiven(Box::new(error)))?;
        self.io.report.event(Event::GivenBackupKept { path });
        Ok(())
    }

    /// The stock backup for the paired feeder: kept from an earlier run, or
    /// read from bank 1 now and kept.
    /// Bank 1 read and kept. A read changes nothing on the feeder, so a
    /// connection that drops (the restart that ends Bluetooth setup, say) only
    /// starts the read again.
    async fn backup(&self) -> Result<StockBackup, Error> {
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let (_, backup) = again_while_lost(&self.io.report, async || {
            let mut connection = self.connect().await?;
            backup::fetch(&mut connection, &self.backups, &identity, &self.io.report).await
        })
        .await?;
        Ok(backup)
    }

    /// Stock's plan sectors from the bank-0 role: kept beside the backup,
    /// then decoded and converted, and the import shown to the owner.
    async fn stock_plans(&self, install: &mut Install) -> Result<(), Error> {
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let mut connection = self.connect().await?;
        // A feeder that predates the backup would end the session on it.
        wellfed_formats::require(connection.capabilities().backup, || Error::NoPlansServed)?;

        let sectors = match connection.ask(random()?, &Op::ReadStockPlans).await? {
            Answer::StockPlans(sectors) => sectors,
            Answer::Refused(refusal) => {
                install.stock_plans_unread(refusal);
                return Ok(());
            }
            other => {
                return Err(Error::Answered {
                    request: "stock plans",
                    answer: Box::new(other),
                });
            }
        };
        let path = storage::plans_path(&self.backups, &identity);
        storage::keep_plans(&path, &plans::kept(sectors.count, &sectors.entries))
            .map_err(|error| Error::KeepPlans(Box::new(error)))?;
        self.io.report.event(Event::PlansKept { path });

        install.stock_plans(sectors.count, &sectors.entries, now_ms()?);
        if let Some(import) = install.import() {
            self.io.report.event(Event::StockPlans {
                import: import.clone(),
                kept: None,
            });
        }
        if let Some(skipped) = install.plans_skipped() {
            self.io.report.event(Event::WithoutStockPlans {
                skipped: skipped.clone(),
            });
        }
        Ok(())
    }

    /// What an earlier run's put left on record. One that cannot be read counts
    /// as a put that may have left.
    fn earlier_put(&self, install: &mut Install) {
        let Some(identity) = self.identity else {
            return;
        };
        let path = storage::put_base_path(&self.backups, &identity);
        match storage::kept_put_base(&path) {
            Ok(None) => {}
            Ok(Some(base)) => {
                self.io.report.event(Event::EarlierPut { base });
                install.earlier_put(base);
            }
            Err(error) => {
                self.io.report.event(Event::PlansUnread {
                    error: crate::text(&error),
                });
                install.earlier_put_unread();
            }
        }
    }

    /// A run that reaches the confirmation without reading stock's
    /// sectors (resumed after the promotion) takes the copy an earlier
    /// run kept beside the backup, if there is one.
    fn kept_plans(&self, install: &mut Install) -> Result<(), Error> {
        if install.import().is_some() || install.plans_skipped().is_some() {
            return Ok(());
        }
        let Some(identity) = self.identity else {
            return Ok(());
        };
        let path = storage::plans_path(&self.backups, &identity);
        let Some(kept) = storage::kept_plans(&path)? else {
            return Ok(());
        };
        install.kept_stock_plans(&kept, now_ms()?);
        if let Some(import) = install.import() {
            self.io.report.event(Event::StockPlans {
                import: import.clone(),
                kept: Some(path),
            });
        }
        Ok(())
    }

    /// Set the kept copy aside once its plans are on the replacement, as
    /// `stock-plans.applied.bin` (or a name with the time if one is there
    /// already), so a later resumed run does not put them back over plans
    /// the owner has changed since.
    fn kept_plans_applied(&self) -> Result<(), Error> {
        let Some(identity) = self.identity else {
            return Ok(());
        };
        let kept = storage::plans_path(&self.backups, &identity);
        if let Some(path) = storage::set_aside(&kept, now_ms()?)? {
            self.io.report.event(Event::PlansSetAside { path });
        }
        Ok(())
    }

    /// The replacement's plans for the steps. A read changes nothing, so a
    /// session that fails is opened again until [`PLANS_WAIT`] runs out.
    async fn read_plans(&self, install: &mut Install) {
        let deadline = Instant::now() + PLANS_WAIT;
        loop {
            match self.plans().await {
                Ok(view) => return install.plans(&view),
                Err(error) if Instant::now() >= deadline => {
                    self.io.report.event(Event::PlansUnread {
                        error: crate::text(&error),
                    });
                    return install.gave_up();
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    async fn plans(&self) -> Result<Plans, Error> {
        let mut connection = self.connect().await?;
        match connection.ask(random()?, &Op::GetPlans).await? {
            Answer::Plans(view) => Ok(view),
            other => Err(Error::Answered {
                request: "plans",
                answer: Box::new(other),
            }),
        }
    }

    /// Put the import once. Anything but an answer is unanswered, which
    /// the steps judge by reading the plans back; this run does not send it
    /// again ([`put_once`]).
    async fn put_plans(
        &mut self,
        base: Generation,
        plans: [Option<FeedingPlan>; PLANS],
    ) -> Result<PlansPut, Error> {
        self.io.report.event(Event::PuttingPlans);
        // Opening a session sends nothing, so it may be tried again; the
        // put itself goes once.
        let mut connection = self
            .session(PLANS_WAIT)
            .await
            .map_err(|error| Error::PlansNotSent(Box::new(error)))?;
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        let kept = storage::put_base_path(&self.backups, &identity);
        self.track.sent(Unsettled::Plans);
        put_once(&mut connection, &kept, base, plans).await
    }

    async fn connect(&self) -> Result<wellfed_connect::Connection, Error> {
        let identity = self.identity.ok_or(Error::NoIdentity)?;
        Ok(wellfed_connect::Connection::open(
            self.io.keys.keypair(),
            identity,
            self.address()?,
            random()?,
        )
        .await?)
    }

    /// The build and bank the replacement reports.
    async fn status(&self) -> Result<(String, u8), Error> {
        let mut connection = self.connect().await?;
        match connection.ask(random()?, &Op::Status).await? {
            Answer::Status(status) => Ok((status.build, status.bank)),
            other => Err(Error::Answered {
                request: "status",
                answer: Box::new(other),
            }),
        }
    }

    /// Tell the stock server the served build answered from bank 0. It
    /// refuses while stock's telemetry is still current, so it is asked
    /// again for a while; a record left open only makes a later run wait.
    async fn close_trial(&self, build: &str) {
        let Some(id) = self.request else {
            return;
        };
        let answer = AnsweredInput {
            request_id: id,
            build: build.to_string(),
            bank: 0,
            note: format!(
                "wellfed install: build {} answered from bank 0",
                &build[..build.len().min(12)]
            ),
        };
        let deadline = Instant::now() + CLOSE_WAIT;
        loop {
            match self.stock.answered(answer.clone()).await {
                Ok(_) => return self.io.report.event(Event::TrialClosed),
                Err(error) if Instant::now() >= deadline => {
                    return self.io.report.event(Event::TrialOpen {
                        error: error.to_string(),
                    });
                }
                Err(_) => tokio::time::sleep(POLL).await,
            }
        }
    }

    /// Push the signed update from bank 0 until bank 1 is activated, or the
    /// update's own error ([`push`]). If bank 1 answers a reopened session
    /// with the served build, the activation's answer was lost and the
    /// feeder already restarted into its trial boot: offering the manifest
    /// there would interrupt the trial, so the promotion counts as
    /// activated instead.
    async fn promote(&mut self, served: &str) -> Result<Result<(), UpdateError>, Error> {
        let payload = self
            .review
            .container
            .get(CONTAINER_HEADER..)
            .ok_or(Error::ShortImage)?;
        let manifest = self.manifest.as_deref().ok_or(Error::NoManifest)?;
        let image = Image {
            manifest,
            downgrade: None,
            payload,
        };

        self.io.report.event(Event::Promoting);
        self.track.sent(Unsettled::Promotion);
        let pushed = push(
            image,
            async || self.connect().await,
            |bank, build| activated(bank, build, served),
            &self.io.report,
        )
        .await?;
        Ok(match pushed {
            Pushed::Activated | Pushed::AlreadyRunning => Ok(()),
            Pushed::Failed(error) => Err(error),
            Pushed::Gone { .. } => Err(UpdateError::Lost),
        })
    }

    /// Until bank 1 runs the build and reports its version confirmed.
    async fn await_confirm(&self, install: &mut Install) -> Result<(), Error> {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let deadline = Instant::now() + CONFIRM_WAIT;
        while Instant::now() < deadline {
            if let Ok(mut connection) = self.connect().await
                && let Ok(Answer::Status(status)) = connection.ask(random()?, &Op::Status).await
                && let Ok(Answer::Update(view)) = connection.ask(random()?, &Op::UpdateState).await
            {
                install.confirmed(
                    Replacement {
                        build: &status.build,
                        bank: status.bank,
                    },
                    view.min_version,
                    status.readiness,
                );
                if !matches!(install.step(), Step::AwaitConfirm) {
                    return Ok(());
                }
            }
            tokio::time::sleep(POLL).await;
        }
        install.gave_up();
        Ok(())
    }
}

/// `read` until it ends with anything but a lost connection, for as long as
/// [`BACKUP_RECONNECT`]. Only for what changes nothing on the feeder.
async fn again_while_lost<T>(
    report: &impl Report,
    mut read: impl AsyncFnMut() -> Result<T, Error>,
) -> Result<T, Error> {
    let deadline = tokio::time::Instant::now() + BACKUP_RECONNECT;
    loop {
        match read().await {
            Err(error) if error.is_connection_lost() && tokio::time::Instant::now() < deadline => {
                report.event(Event::SessionDropped {
                    error: crate::text(&error),
                });
                tokio::time::sleep(POLL).await;
            }
            read => return read,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scripted;
    use wellfed_client::update::{Installer, Next};
    use wellfed_feeder_sim::flash::Flash;
    use wellfed_feeder_sim::harness::Running;
    use wellfed_session::noise::Keypair;
    use wellfed_stock::controller::Rejection;

    /// A session lost after the last chunk, reopened once the feeder
    /// restarted into its trial boot, is the activation: the manifest is not
    /// offered to bank 1 (that would read as a failure, or give way and
    /// abandon the trial).
    #[test]
    fn bank_1_running_the_served_build_on_reopen_is_the_activation() {
        let served = "ab".repeat(32);
        assert!(activated(1, &served.to_ascii_uppercase(), &served));
        assert!(
            !activated(0, &served, &served),
            "bank 0 resumes the install"
        );
        assert!(!activated(1, &"cd".repeat(32), &served));

        // What the installer would do instead: from Receiving, a session on
        // bank 1 goes back to offering, which is what this avoids.
        let manifest = [0u8; 4];
        let payload = [0u8; 4];
        let mut installer = Installer::new(Image {
            manifest: &manifest,
            downgrade: None,
            payload: &payload,
        });
        installer.opened(1);
        assert!(matches!(
            installer.poll(),
            Next::Send(wellfed_client::update::Sent::Offer, _)
        ));
    }

    /// A dropped connection reads again, and is said each time. What the read
    /// itself found ends it at once, and a feeder that stays gone ends it when
    /// the time is up.
    #[tokio::test(start_paused = true)]
    async fn a_read_goes_again_only_while_the_connection_is_lost() {
        let lost = || Error::Platform("connection reset".into());
        let said = std::cell::Cell::new(0);
        let report = |event| {
            assert!(matches!(event, Event::SessionDropped { .. }), "{event:?}");
            said.set(said.get() + 1);
        };

        let mut reads = 0;
        let read = again_while_lost(&report, async || {
            reads += 1;
            if reads < 3 { Err(lost()) } else { Ok(reads) }
        });
        assert_eq!(read.await.unwrap(), 3);
        assert_eq!(said.get(), 2);

        let mut reads = 0;
        let read = again_while_lost(&report, async || -> Result<(), Error> {
            reads += 1;
            Err(Error::Unverified)
        });
        assert!(matches!(read.await, Err(Error::Unverified)));
        assert_eq!(reads, 1);

        let began = tokio::time::Instant::now();
        let read = again_while_lost(&report, async || -> Result<(), Error> { Err(lost()) });
        assert!(read.await.unwrap_err().is_connection_lost());
        assert!(began.elapsed() >= BACKUP_RECONNECT);
    }

    /// An expected device answers the feeder check ahead: the pinned
    /// device passes without a question, any other stops the run naming
    /// both, and without it a person is asked.
    #[tokio::test]
    async fn an_expected_device_answers_the_feeder_check_or_stops_the_run() {
        let enrolled = || FeederTrust::Enrolled {
            device: "AF_FEEDER".into(),
            address: "10.42.103.172".into(),
        };
        // A prompter with nobody behind it: any question it put would fail.
        let mut prompter = Scripted::default();
        let report = |_| {};

        assert!(
            recognized(&mut prompter, &report, enrolled(), Some("AF_FEEDER"))
                .await
                .unwrap()
        );

        let error = recognized(&mut prompter, &report, enrolled(), Some("AF_OTHER"))
            .await
            .unwrap_err();
        match &error {
            Error::OtherDevice {
                expected,
                enrolled,
                address,
            } => assert_eq!(
                (expected.as_str(), enrolled.as_str(), address.as_str()),
                ("AF_OTHER", "AF_FEEDER", "10.42.103.172")
            ),
            other => panic!("{other:?}"),
        }

        let error = recognized(&mut prompter, &report, enrolled(), None)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Platform(_)), "{error:?}");
        assert_eq!(
            prompter.asked,
            [Prompt::RecognizeFeeder {
                device: "AF_FEEDER".into(),
                address: "10.42.103.172".into()
            }]
        );

        // A contested enrollment is no feeder, pinned or not.
        assert!(
            !recognized(
                &mut prompter,
                &report,
                FeederTrust::Contested,
                Some("AF_FEEDER")
            )
            .await
            .unwrap()
        );
    }

    fn on_a_feeder(test: impl std::future::Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(tokio::task::LocalSet::new().run_until(test));
    }

    fn a_meal() -> [Option<FeedingPlan>; PLANS] {
        let mut plans = [None; PLANS];
        plans[0] = Some(FeedingPlan {
            minute: 600,
            portions: 1,
            days: wellfed_model::Weekdays::ONCE,
            enabled: false,
            call: 0,
        });
        plans
    }

    async fn read(connection: &mut wellfed_connect::Connection) -> Plans {
        match connection.ask(random().unwrap(), &Op::GetPlans).await {
            Ok(Answer::Plans(plans)) => plans,
            other => panic!("{other:?}"),
        }
    }

    /// With the base not kept, the put never reaches the feeder. Kept, it is
    /// there by the time the put is answered, and a later run finds it.
    #[test]
    fn the_base_is_kept_before_the_put_or_the_put_does_not_go() {
        on_a_feeder(async {
            let keys = Keypair::from_secret([0x51; 32]);
            let sim = Running::start(&Flash::memory(), Some(*keys.public())).await;
            let dir = tempfile::tempdir().unwrap();
            let mut connection = wellfed_connect::Connection::open(
                &keys,
                sim.identity,
                sim.address,
                random().unwrap(),
            )
            .await
            .unwrap();
            let before = read(&mut connection).await;

            // A file where its directory should be.
            std::fs::write(dir.path().join("blocked"), b"").unwrap();
            let blocked = dir.path().join("blocked/stock-plans.base.json");
            let refused = put_once(&mut connection, &blocked, before.generation, a_meal()).await;
            assert!(matches!(refused, Err(Error::KeepPutBase(_))), "{refused:?}");
            assert_eq!(read(&mut connection).await, before, "the put went out");

            let kept = dir.path().join("stock-plans.base.json");
            assert_eq!(storage::kept_put_base(&kept).unwrap(), None);
            let put = put_once(&mut connection, &kept, before.generation, a_meal())
                .await
                .unwrap();
            assert_eq!(put, PlansPut::Done);
            assert_eq!(
                storage::kept_put_base(&kept).unwrap(),
                Some(before.generation)
            );

            sim.stop().await;
        });
    }

    /// Two puts from one base cannot both land: a first put arriving after the
    /// read, or after the second, is refused by it, and the table moves once.
    /// The owner emptying the table afterwards moves it on again, so a table
    /// that reads empty still does not read as the base.
    #[test]
    fn a_late_first_put_is_refused_by_its_base() {
        on_a_feeder(async {
            let keys = Keypair::from_secret([0x51; 32]);
            let sim = Running::start(&Flash::memory(), Some(*keys.public())).await;
            let dir = tempfile::tempdir().unwrap();
            let kept = dir.path().join("stock-plans.base.json");
            let open = || async {
                wellfed_connect::Connection::open(
                    &keys,
                    sim.identity,
                    sim.address,
                    random().unwrap(),
                )
                .await
                .unwrap()
            };
            let mut again = open().await;
            let mut first = open().await;
            let base = read(&mut again).await.generation;

            // The run after the stop goes first, against the same base.
            let put = put_once(&mut again, &kept, base, a_meal()).await.unwrap();
            assert_eq!(put, PlansPut::Done);

            // The stopped run's put, still on its way, arrives.
            let late = Op::PutPlans {
                base,
                plans: a_meal(),
            };
            let landed = read(&mut again).await;
            assert_ne!(landed.generation, base);
            assert_eq!(landed.plans, a_meal());
            assert_eq!(
                first.ask(random().unwrap(), &late).await.unwrap(),
                Answer::Refused(wellfed_model::Refusal::Stale {
                    current: landed.generation
                })
            );
            assert_eq!(read(&mut again).await, landed);

            // The owner empties the table: empty, and not at the base.
            let empty = Op::PutPlans {
                base: landed.generation,
                plans: [None; PLANS],
            };
            assert!(matches!(
                again.ask(random().unwrap(), &empty).await.unwrap(),
                Answer::Done { .. }
            ));
            let emptied = read(&mut again).await;
            assert_eq!(emptied.plans, [None; PLANS]);
            assert_ne!(
                Some(emptied.generation),
                storage::kept_put_base(&kept).unwrap()
            );

            sim.stop().await;
        });
    }

    /// A run takes the paired record only when its broker enrolled the
    /// same stock device: another feeder's record, left by an earlier
    /// install, never answers for this one. The key's feeder is tried only
    /// where this run knows its feeder is, and setup follows when it does
    /// not answer.
    #[test]
    fn a_run_takes_only_its_own_installs_record() {
        let recorded = storage::Paired {
            device: "AF_A".into(),
            identity: wellfed_model::Hex([0xab; 32]),
            address: Some("192.168.4.20:7100".parse().unwrap()),
        };

        let known = Some([0xcd; 32]);
        let ours = Lookup::Recorded {
            identity: [0xab; 32],
            last: recorded.address,
        };
        assert_eq!(lookup(Some(&recorded), Some("AF_A"), None, known), ours);
        assert_eq!(
            lookup(Some(&recorded), Some("AF_A"), Some("abab"), known),
            ours
        );
        // A prefix naming another passes the record over.
        assert_eq!(
            lookup(Some(&recorded), Some("AF_A"), Some("cd"), known),
            Lookup::KeyFile {
                identity: [0xcd; 32]
            }
        );

        // A stale record from another stock device: never this run's.
        assert_eq!(
            lookup(Some(&recorded), Some("AF_B"), None, known),
            Lookup::KeyFile {
                identity: [0xcd; 32]
            }
        );
        assert_eq!(
            lookup(Some(&recorded), Some("AF_B"), None, None),
            Lookup::Setup
        );
        // No enrolled device (a captured config): the record is no one's.
        assert_eq!(lookup(Some(&recorded), None, None, None), Lookup::Setup);

        // This install's feeder not answering stops the run.
        let here: SocketAddr = "192.168.4.31:7100".parse().unwrap();
        let wait = Unjoined::Wait;
        assert!(matches!(
            looked_up(ours, None, wait),
            Err(Error::NotAnswering { .. })
        ));
        assert_eq!(
            looked_up(ours, Some(here), wait).unwrap(),
            Looked::Paired([0xab; 32], here)
        );
        // Setting it up again is for a feeder that never joined, and one that
        // answers needs none.
        assert!(matches!(
            looked_up(ours, None, Unjoined::SetUpAgain),
            Err(Error::NotAnswering { .. })
        ));
        let never_joined = Lookup::Recorded {
            identity: [0xab; 32],
            last: None,
        };
        assert_eq!(
            looked_up(never_joined, None, wait).unwrap(),
            Looked::Rejoining([0xab; 32])
        );
        assert_eq!(
            looked_up(never_joined, None, Unjoined::SetUpAgain).unwrap(),
            Looked::SetUp
        );
        assert_eq!(
            looked_up(never_joined, Some(here), Unjoined::SetUpAgain).unwrap(),
            Looked::Paired([0xab; 32], here)
        );
        // The key's feeder not answering falls through to setup.
        let key = Lookup::KeyFile {
            identity: [0xcd; 32],
        };
        assert_eq!(looked_up(key, None, wait).unwrap(), Looked::SetUp);
        assert_eq!(
            looked_up(key, Some(here), wait).unwrap(),
            Looked::Paired([0xcd; 32], here)
        );
        assert_eq!(looked_up(Lookup::Setup, None, wait).unwrap(), Looked::SetUp);
    }

    /// Refused is the server's word with no record in its ledger, and keeps
    /// the server's reason to the end of the run; anything else that was not
    /// plainly sent is uncertain, and is never asked again.
    #[test]
    fn a_request_is_refused_only_on_the_servers_word_and_an_empty_ledger() {
        let offline = Refusal::Rejected(Rejection::Offline);
        let refused: Result<(), Refusal> = Err(offline);
        let no_record: Result<_, ()> = Ok(None);

        assert_eq!(
            judged(&refused, &no_record),
            (Requested::Refused, Some(offline))
        );
        assert_eq!(
            judged(&Err::<(), _>(Refusal::Contested), &no_record),
            (Requested::Refused, Some(Refusal::Contested))
        );

        // The call's own answer, or the record it left, shows it went.
        assert_eq!(judged(&Ok(()), &Err::<_, ()>(())), (Requested::Sent, None));
        assert_eq!(
            judged(&refused, &Ok::<_, ()>(Some(TrialPhase::Submitted))),
            (Requested::Sent, None)
        );

        // A record in another phase, an unread ledger, or no answer at all.
        assert_eq!(
            judged(&refused, &Ok::<_, ()>(Some(TrialPhase::Uncertain))),
            (Requested::Uncertain, None)
        );
        assert_eq!(
            judged(&refused, &Err::<_, ()>(())),
            (Requested::Uncertain, None)
        );
        assert_eq!(
            judged(&Err::<(), _>(Refusal::Unanswered), &no_record),
            (Requested::Uncertain, None)
        );

        // The steps' refusal ends the run with the reason, or plainly with none.
        assert!(matches!(
            failure(InstallError::RequestRefused, Some(offline)),
            Error::RequestRefused(reason) if reason == offline
        ));
        assert!(matches!(
            failure(InstallError::RequestRefused, None),
            Error::Steps(InstallError::RequestRefused)
        ));
        assert!(matches!(
            failure(InstallError::Contested, Some(offline)),
            Error::Steps(InstallError::Contested)
        ));
    }
}
