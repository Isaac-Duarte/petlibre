//! Putting stock back from the owner's backup ([rollback]), the one driver
//! `wellfed restore-stock` and the Android app run.
//!
//! A feeder takes stock only with a downgrade statement bound to its
//! identity, the exact manifest and its floor, so nothing can be signed ahead
//! of time. So the owner's route makes an update key when the restore starts,
//! enrolls it with a Dispense press at the feeder (the physical consent) and
//! signs here: [`Frozen::stock`] as the image, which only the pinned container
//! passes, at a version above the floor the feeder reports. The bench's route
//! gives a key the feeder already trusts, and enrolls nothing.
//!
//! The key file lives beside the backup only while it can still matter. An
//! enrollment that went through, with the run ending before its offer, leaves
//! it there, so the next run finds the key listed, sends no enrollment and
//! asks for no second press. The file goes before the offer, with the manifest
//! and statement already signed in memory: a run killed after that leaves
//! nothing that could offer again with no press, and a feeder that never
//! answered the offer costs a second press. It goes too once the feeder says
//! the key is not enrolled. A spent key stays listed on the feeder with its
//! private half gone, and signs nothing more.
//!
//! The press is consent to this restore, not to one days later: a file older
//! than [`KEY_LIFE`] is spent like the others, and the run enrolls a new key.
//!
//! The push is the installer's own, from either bank: on bank 1 the offer is
//! recorded and bank 1 gives way, then bank 0 takes the same manifest and
//! statement (both banks read one store and one list of enrolled keys, and the
//! floor stays where it was) and writes stock into bank 1. Within the push the
//! shared installer offers again where the update protocol needs it, but
//! nothing offers once the push has returned: a refusal ends the restore, and
//! a feeder that stops answering after an offer went out leaves the result
//! uncertain, since stock serves no device API to ask.
//!
//! [rollback]: ../../../../docs/firmware/consumer-installation.md#rollback

use std::cell::Cell;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::time::Instant;
use wellfed_client::install::{InstallError, StockBackup};
use wellfed_client::update::{Image, InstallError as UpdateError};
use wellfed_client::update_key::{EnrollError, Enrolled};
use wellfed_connect::Connection;
use wellfed_formats::credentials::KeyScope;
use wellfed_model::{Answer, ClientRole, Op, UpdatePhase, UpdateView};
use wellfed_session::noise::Keypair;
use wellfed_signing::{Frozen, Key, Motion};

use crate::event::{Enrollment, Event};
use crate::platform::{Discovery, Prompter, Report};
use crate::push::{Pushed, push};
use crate::session::random;
use crate::track::Stop;
use crate::{Error, storage, update_key};

/// How feeders list a restore's update key.
const LABEL: &str = "restore";
/// How long the feeder's signers are read after an unanswered enrollment.
const SIGNERS_WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(5);

/// What signs the restore.
pub enum Signing {
    /// An update key made for this restore, enrolled with a Dispense press.
    Fresh,
    /// A key the feeder already trusts; nothing is enrolled.
    Given(Box<Key>),
}

/// Which feeder a restore is for, and how it signs.
pub struct Target {
    pub identity: [u8; 32],
    /// Where it answers, tried before discovery.
    pub address: Option<SocketAddr>,
    /// The owner's data directory, where a fresh key lives for the restore.
    pub backups: PathBuf,
    pub signing: Signing,
    /// The version the manifest carries; one above the floor when not given.
    pub version: Option<u32>,
}

/// A restore, its image checked before the feeder is touched.
pub struct Plan {
    image: Frozen,
    target: Target,
}

impl Plan {
    /// Restore `backup` as the stock image: only the pinned container passes.
    pub fn new(backup: &StockBackup, target: Target) -> Result<Self, Error> {
        Ok(Self {
            image: Frozen::stock(backup.bytes().to_vec())?,
            target,
        })
    }

    /// [`Self::new`] without the pin, for tests and simulators, which cannot
    /// hold the vendor's image. Never in a build that ships.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn synthetic(backup: &StockBackup, target: Target) -> Self {
        Self {
            image: Frozen::synthetic_stock(backup.bytes().to_vec()),
            target,
        }
    }
}

/// What the platform gives a restore.
pub struct Platform<P, D, R> {
    pub prompter: P,
    pub discovery: D,
    pub keypair: Keypair,
    pub report: R,
    pub offering: Offering,
}

/// Whether an offer may be out, for a caller that outlives the run: a thread
/// that panicked says nothing else.
#[derive(Clone, Debug, Default)]
pub struct Offering(Arc<AtomicBool>);

impl Offering {
    pub fn may_be_out(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn set(&self, may_be_out: bool) {
        self.0.store(may_be_out, Ordering::SeqCst);
    }
}

/// How a restore that reached its offer ended. Every way it ends before an
/// offer goes out is an [`Error`], and the feeder keeps the replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restored {
    /// Bank 1 is activated; the feeder restarts into stock.
    Activated,
    /// The feeder refused, or the install stopped. The offer is not sent
    /// again, and the feeder keeps running the replacement from bank 0 (bank
    /// 1 may be partly written).
    Refused(UpdateError),
    /// The feeder may run stock already, or still the replacement. Nothing
    /// is sent again.
    Uncertain(Uncertainty),
}

/// Why a restore's end is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uncertainty {
    /// The feeder stopped answering after the offer went out.
    Gone,
    /// The owner stopped the run once an offer may have gone out; it may not
    /// have, since the mark goes up before anything is sent.
    Stopped(Stop),
    /// The run's own failure, with these words, once an offer may have gone
    /// out.
    Failed(String),
}

/// Run one restore to its end. `stop` ends it through its cleanup.
pub async fn restore<P, D, R>(
    plan: Plan,
    platform: Platform<P, D, R>,
    stop: impl Future<Output = Stop>,
) -> Result<Restored, Error>
where
    P: Prompter,
    D: Discovery,
    R: Report,
{
    let Plan { image, target } = plan;
    let Platform {
        mut prompter,
        discovery,
        keypair,
        report,
        offering,
    } = platform;
    let absent = Cell::new(false);

    let ended = tokio::select! {
        ended = drive(&image, &target, &mut prompter, &discovery, &keypair, &report, &absent, &offering) => settled(ended, &offering),
        by = stop => if offering.may_be_out() {
            Ok(Restored::Uncertain(Uncertainty::Stopped(by)))
        } else {
            Err(Error::RestoreStopped(by))
        },
    };

    if matches!(target.signing, Signing::Fresh) && absent.get() {
        let file = storage::restore_key_path(&target.backups, &target.identity);
        if let Err(error) = std::fs::remove_file(&file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            report.event(Event::RestoreKeyKept {
                error: error.to_string(),
            });
        }
    }
    ended
}

/// `ended`, as it reads with what `offering` shows: a failure once an offer
/// may be out says nothing about what the feeder did with it.
fn settled(ended: Result<Restored, Error>, offering: &Offering) -> Result<Restored, Error> {
    match ended {
        Err(error) if offering.may_be_out() => Ok(Restored::Uncertain(Uncertainty::Failed(
            crate::text(&error),
        ))),
        other => other,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the run's parts, taken apart once so `restore` can clean up after it"
)]
async fn drive(
    image: &Frozen,
    target: &Target,
    prompter: &mut impl Prompter,
    discovery: &impl Discovery,
    keypair: &Keypair,
    report: &impl Report,
    absent: &Cell<bool>,
    offering: &Offering,
) -> Result<Restored, Error> {
    let sessions = Sessions {
        identity: target.identity,
        keypair,
        discovery,
        address: Cell::new(target.address),
    };
    let mut session = sessions.open().await?;
    wellfed_formats::require(session.capabilities().update, || Error::RestoreNoUpdates)?;

    let fresh;
    let key: &Key = match &target.signing {
        Signing::Given(key) => key,
        Signing::Fresh => {
            fresh = fresh_key(&storage::restore_key_path(
                &target.backups,
                &target.identity,
            ))?;
            enroll(&mut session, &sessions, &fresh, prompter, report, absent).await?;
            &fresh
        }
    };

    // The statement is bound to this floor, and the push's own offers come
    // before anything can raise it.
    let floor = clear_floor(&mut session, report).await?;
    report.event(Event::Floor(floor));
    let version = match target.version {
        Some(version) => version,
        None => floor
            .checked_add(1)
            .ok_or(Error::Steps(InstallError::NoVersionAbove { floor }))?,
    };
    let manifest = image.offer(key, version, Motion::Refused)?;
    let downgrade = key.downgrade(&manifest, &target.identity, floor)?;
    report.event(Event::RestoreSigned {
        version,
        floor,
        key: key.id(),
    });
    drop(session);

    // Before anything is sent: a key file that outlived an offer would let a
    // later run offer with no press. One that will not go stops the run here.
    if matches!(target.signing, Signing::Fresh) {
        let file = storage::restore_key_path(&target.backups, &target.identity);
        std::fs::remove_file(&file).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                Error::RestoreKeyGone
            } else {
                Error::Remove { path: file, source }
            }
        })?;
    }

    report.event(Event::Restoring);
    offering.set(true);
    let pushed = push(
        Image {
            manifest: &manifest,
            downgrade: Some(&downgrade),
            payload: image.payload(),
        },
        async || sessions.open().await,
        // Stock serves no device API: a session that opens is never it.
        |_, _| false,
        report,
    )
    .await?;

    match pushed {
        Pushed::Activated | Pushed::AlreadyRunning => Ok(Restored::Activated),
        Pushed::Failed(error) => Ok(Restored::Refused(error)),
        Pushed::Gone { offered: true } => Ok(Restored::Uncertain(Uncertainty::Gone)),
        Pushed::Gone { offered: false } => {
            offering.set(false);
            Err(Error::NotAnswering {
                feeder: target.identity,
            })
        }
    }
}

/// How long a key file left by an earlier run still stands for the press
/// that enrolled it.
const KEY_LIFE: Duration = Duration::from_secs(60 * 60);

/// Whether `file` was written within [`KEY_LIFE`]. A time that cannot be read,
/// or one ahead of the clock, proves nothing, so it does not stand.
fn key_is_recent(file: &Path) -> bool {
    std::fs::metadata(file)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|written| written.elapsed().ok())
        .is_some_and(|age| age < KEY_LIFE)
}

/// The key an earlier run of this restore left, or a new one. A file too old
/// to stand for its press is replaced, so a new press enrolls the new key. So
/// is a file that does not load, which was never enrolled, since enrolling
/// takes a loaded key.
fn fresh_key(file: &Path) -> Result<Key, Error> {
    if file.exists() {
        if key_is_recent(file)
            && let Ok(key) = Key::load(file)
        {
            return Ok(key);
        }
        std::fs::remove_file(file).map_err(|source| Error::Remove {
            path: file.to_path_buf(),
            source,
        })?;
    }
    if let Some(directory) = file.parent() {
        std::fs::create_dir_all(directory).map_err(|source| Error::Create {
            path: directory.to_path_buf(),
            source,
        })?;
    }
    wellfed_signing::keygen(file, KeyScope::Owner).map_err(Error::RestoreKey)?;
    Key::load(file).map_err(Error::RestoreKey)
}

/// Forget what a restore left for the feeder `identity`, as an owner leaving
/// it does: a live key with no expiry should not outlast their interest. The
/// feeder still lists it, and nothing can sign with it.
pub fn forget_key(backups: &Path, identity: &[u8; 32]) -> Result<(), Error> {
    let file = storage::restore_key_path(backups, identity);
    match std::fs::remove_file(&file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(Error::Remove {
            path: file,
            source: error,
        }),
        _ => Ok(()),
    }
}

/// Enroll `key` at most once. An enrollment with no answer may have gone
/// through all the same, so the feeder's signers decide, on a new session.
async fn enroll<D: Discovery>(
    session: &mut Connection,
    sessions: &Sessions<'_, D>,
    key: &Key,
    prompter: &mut impl Prompter,
    report: &impl Report,
    absent: &Cell<bool>,
) -> Result<(), Error> {
    let outcome =
        update_key::enroll(session, key.public(), key.id(), LABEL, prompter, report).await?;

    let shown = |outcome| {
        report.event(Event::KeyEnrollment {
            key: key.id(),
            outcome,
        });
    };
    match outcome {
        Ok(enrolled) => {
            shown(Enrollment::Enrolled(enrolled));
            Ok(())
        }
        Err(EnrollError::Uncertain) => {
            shown(Enrollment::Unanswered);
            *session = signed_up(sessions, key, report)
                .await
                .ok_or(Error::RestoreEnroll(EnrollError::Uncertain))?;
            shown(Enrollment::Enrolled(Enrolled::Now));
            Ok(())
        }
        Err(EnrollError::ListUnanswered) => Err(Error::RestoreEnroll(EnrollError::ListUnanswered)),
        Err(error) => {
            absent.set(true);
            Err(Error::RestoreEnroll(error))
        }
    }
}

/// A session once the feeder lists `key` as a signer; `None` if it does not,
/// or could not be read in time. A read changes nothing, so it goes again.
async fn signed_up<D: Discovery>(
    sessions: &Sessions<'_, D>,
    key: &Key,
    report: &impl Report,
) -> Option<Connection> {
    let deadline = Instant::now() + SIGNERS_WAIT;
    loop {
        let read = async {
            let mut session = sessions.open().await?;
            let clients = update_key::clients(&mut session).await?;
            Ok::<_, Error>((session, clients))
        };
        match read.await {
            Ok((session, clients)) => {
                let listed = clients
                    .iter()
                    .any(|c| c.key.0 == key.public() && c.role == ClientRole::UpdateSigner);
                return listed.then_some(session);
            }
            Err(error) if Instant::now() >= deadline => {
                report.event(Event::SignersUnread {
                    error: crate::text(&error),
                });
                return None;
            }
            Err(_) => tokio::time::sleep(POLL).await,
        }
    }
}

async fn state(session: &mut Connection) -> Result<UpdateView, Error> {
    match session.ask(random()?, &Op::UpdateState).await? {
        Answer::Update(view) => Ok(view),
        other => Err(Error::Answered {
            request: "update state",
            answer: Box::new(other),
        }),
    }
}

/// The floor, with the way clear for this run's offer. A running install is
/// left alone. A candidate a cut or kill left pending refuses every other
/// manifest, and a restore's is always another, so it is cancelled once; a
/// cancel with any doubt about it stops the run, since the one offer is not
/// to be spent on a refusal.
async fn clear_floor(session: &mut Connection, report: &impl Report) -> Result<u32, Error> {
    let view = state(session).await?;
    if view.phase != UpdatePhase::Idle {
        return Err(Error::RestoreInstallRunning { phase: view.phase });
    }
    if view.digest.is_none() {
        return Ok(view.min_version);
    }

    report.event(Event::StaleInstallCancelled);
    let cancelled = session.ask(random()?, &Op::CancelUpdate).await;
    if !matches!(cancelled, Ok(Answer::Update(_))) {
        return Err(Error::RestoreCancelUnknown);
    }
    match state(session).await {
        Ok(view) if view.phase == UpdatePhase::Idle && view.digest.is_none() => {
            Ok(view.min_version)
        }
        _ => Err(Error::RestoreCancelUnknown),
    }
}

/// Sessions with the feeder, wherever it answers now: a restart may move it.
struct Sessions<'a, D> {
    identity: [u8; 32],
    keypair: &'a Keypair,
    discovery: &'a D,
    address: Cell<Option<SocketAddr>>,
}

impl<D: Discovery> Sessions<'_, D> {
    async fn open(&self) -> Result<Connection, Error> {
        let known = self.address.get();
        let failed = match known {
            Some(address) => match self.open_at(address).await {
                Ok(session) => return Ok(session),
                Err(error) => Some(error),
            },
            None => None,
        };

        match self.discovery.locate(self.identity).await {
            Some(found) if Some(found) != known => {
                let session = self.open_at(found).await?;
                self.address.set(Some(found));
                Ok(session)
            }
            _ => Err(failed.unwrap_or(Error::RestoreNotFound)),
        }
    }

    async fn open_at(&self, address: SocketAddr) -> Result<Connection, Error> {
        Ok(Connection::open(self.keypair, self.identity, address, random()?).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::SystemTime;

    use tokio::task::LocalSet;
    use wellfed_client::install::StockPins;
    use wellfed_core::ota::{MARKER, SECTOR};
    use wellfed_feeder_sim::banks::Banks;
    use wellfed_feeder_sim::flash::Flash;
    use wellfed_feeder_sim::harness::Running;
    use wellfed_feeder_sim::serve::Control;
    use wellfed_formats::bench::prompt::Prompt;
    use wellfed_model::{Refusal, UpdateRefusal};

    use super::*;
    use crate::platform::{Announcement, PlatformError};

    const OWNER: [u8; 32] = [0x21; 32];

    fn paused(test: impl Future<Output = ()>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(LocalSet::new().run_until(test));
    }

    /// A made-up stock: past the 64 KiB a push reports its progress at.
    fn synthetic() -> StockBackup {
        let mut bank: Vec<u8> = (0..20 * SECTOR).map(|i| (i % 251) as u8).collect();
        bank[..MARKER.len()].copy_from_slice(&MARKER);
        let pins = StockPins::of_payload(&bank);
        StockBackup::from_bank_pinned(bank, &pins).unwrap()
    }

    struct Rig {
        sim: Running,
        flash: Rc<Flash>,
        banks: Rc<RefCell<Banks>>,
        dir: tempfile::TempDir,
    }

    impl Rig {
        /// A feeder on bank 1, as after a promotion, with no built-in key.
        async fn start() -> Self {
            let (flash, banks) = (Flash::memory(), Banks::new());
            let sim = Running::start_with(
                &flash,
                &banks,
                None,
                Some(*Keypair::from_secret(OWNER).public()),
                SocketAddr::from(([127, 0, 0, 1], 0)),
            )
            .await;
            sim.type_line("hold lock").unwrap();
            Self {
                sim,
                flash,
                banks,
                dir: tempfile::tempdir().unwrap(),
            }
        }

        /// The same feeder again after [`Control::Quit`], where it was.
        async fn again(self) -> Self {
            let address = self.sim.address;
            self.sim.stop().await;
            let sim = Running::start_with(
                &self.flash,
                &self.banks,
                None,
                Some(*Keypair::from_secret(OWNER).public()),
                address,
            )
            .await;
            sim.type_line("hold lock").unwrap();
            Self { sim, ..self }
        }

        fn target(&self, signing: Signing) -> Target {
            Target {
                identity: self.sim.identity,
                address: Some(self.sim.address),
                backups: self.dir.path().to_path_buf(),
                signing,
                version: None,
            }
        }

        fn key_file(&self) -> PathBuf {
            storage::restore_key_path(self.dir.path(), &self.sim.identity)
        }

        fn plan(&self, backup: &StockBackup, signing: Signing) -> Plan {
            Plan::synthetic(backup, self.target(signing))
        }

        async fn run(
            &self,
            plan: Plan,
            hands: &Hands<'_>,
            report: impl Report,
        ) -> Result<Restored, Error> {
            let platform = Platform {
                prompter: hands,
                discovery: Nowhere,
                keypair: Keypair::from_secret(OWNER),
                report,
                offering: Offering::default(),
            };
            restore(plan, platform, std::future::pending()).await
        }

        async fn bank(&self) -> u8 {
            let keys = Keypair::from_secret(OWNER);
            let mut session = Connection::open(
                &keys,
                self.sim.identity,
                self.sim.address,
                random().unwrap(),
            )
            .await
            .unwrap();
            match session
                .ask(random().unwrap(), &Op::UpdateState)
                .await
                .unwrap()
            {
                Answer::Update(view) => view.bank,
                other => panic!("{other:?}"),
            }
        }
    }

    struct Nowhere;

    impl Discovery for Nowhere {
        async fn locate(&self, _: [u8; 32]) -> Option<SocketAddr> {
            None
        }
    }

    /// The person at the feeder: presses Dispense when asked, if `presses`.
    struct Hands<'a> {
        sim: Option<&'a Running>,
        asked: Cell<u32>,
    }

    impl<'a> Hands<'a> {
        fn pressing(sim: &'a Running) -> Self {
            Self {
                sim: Some(sim),
                asked: Cell::new(0),
            }
        }

        fn away() -> Self {
            Self {
                sim: None,
                asked: Cell::new(0),
            }
        }
    }

    impl Prompter for &Hands<'_> {
        async fn done(&mut self, prompt: Prompt) -> Result<(), PlatformError> {
            Err(format!("a restore asks nothing: {prompt:?}").into())
        }

        async fn yes(&mut self, prompt: Prompt) -> Result<bool, PlatformError> {
            Err(format!("a restore asks nothing: {prompt:?}").into())
        }

        async fn announce(&mut self, announcement: Announcement) -> Result<(), PlatformError> {
            assert!(matches!(announcement, Announcement::PressDispense { .. }));
            self.asked.set(self.asked.get() + 1);
            if let Some(sim) = self.sim {
                sim.type_line("press dispense").unwrap();
            }
            Ok(())
        }

        async fn settle(&mut self) -> Result<(), PlatformError> {
            Ok(())
        }
    }

    /// From bank 1, as after a promotion: a fresh key, one press, bank 1
    /// gives way, bank 0 writes stock, and the key file is gone.
    #[test]
    fn a_fresh_key_enrolls_once_and_stock_goes_in_from_bank_1() {
        paused(async {
            let rig = Rig::start().await;
            assert_eq!(rig.bank().await, 1);
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let signed = Cell::new(None);
            let file = rig.key_file();
            let report = |event| match event {
                Event::RestoreSigned { version, floor, .. } => {
                    signed.set(Some((version, floor)));
                }
                // A kill from here on must leave nothing that offers with no
                // press.
                Event::Restoring => assert!(!file.exists(), "the key outlived the offer"),
                _ => {}
            };

            let plan = rig.plan(&backup, Signing::Fresh);
            let restored = rig.run(plan, &hands, report).await.unwrap();

            assert_eq!(restored, Restored::Activated);
            assert_eq!(hands.asked.get(), 1);
            let (version, floor) = signed.get().unwrap();
            assert_eq!(version, floor + 1, "signed above the floor it read");
            assert_eq!(
                &rig.banks.borrow().bank1()[..backup.payload().len()],
                backup.payload(),
                "bank 1 holds stock"
            );
            assert!(!rig.key_file().exists(), "the key stays only for the run");
            rig.sim.stop().await;
        });
    }

    /// An enrollment that went through, then a feeder gone before any offer:
    /// the key stays, and the next run finds it listed. It sends no second
    /// enrollment and asks for no second press, which would end `Unconfirmed`
    /// here, with nobody at the panel.
    #[test]
    fn a_retry_after_the_enrollment_sends_it_again_nowhere() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let report = |event| {
                if matches!(event, Event::KeyEnrollment { .. }) {
                    sim.control(Control::Quit);
                }
            };

            let plan = rig.plan(&backup, Signing::Fresh);
            let first = rig.run(plan, &hands, report).await;
            assert!(first.is_err(), "{first:?}");
            assert_eq!(hands.asked.get(), 1);
            assert!(rig.key_file().exists(), "kept for the retry");
            assert_eq!(rig.banks.borrow().bank1()[0], MARKER[0], "nothing offered");

            let rig = rig.again().await;
            let hands = Hands::away();
            let already = Cell::new(false);
            let report = |event| {
                if let Event::KeyEnrollment {
                    outcome: Enrollment::Enrolled(Enrolled::Already),
                    ..
                } = event
                {
                    already.set(true);
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let second = rig.run(plan, &hands, report).await.unwrap();

            assert_eq!(second, Restored::Activated);
            assert!(already.get());
            assert_eq!(hands.asked.get(), 0, "no second press");
            assert!(!rig.key_file().exists());
            rig.sim.stop().await;
        });
    }

    /// No press: the feeder refuses the key, the run ends with nothing
    /// offered, and the unenrolled key is not kept.
    #[test]
    fn no_press_ends_it_with_nothing_offered() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::away();
            let plan = rig.plan(&backup, Signing::Fresh);

            let ended = rig.run(plan, &hands, |_| {}).await;

            assert!(
                matches!(ended, Err(Error::RestoreEnroll(EnrollError::Unconfirmed))),
                "{ended:?}"
            );
            assert_eq!(hands.asked.get(), 1);
            assert!(!rig.key_file().exists());
            assert_eq!(rig.bank().await, 1, "the feeder never gave way");
            rig.sim.stop().await;
        });
    }

    /// A key the feeder does not trust: the offer is refused once, and
    /// nothing goes again.
    #[test]
    fn a_refused_offer_ends_it() {
        paused(async {
            let rig = Rig::start().await;
            let file = rig.dir.path().join("key.json");
            wellfed_signing::keygen(&file, KeyScope::Owner).unwrap();
            let key = Key::load(&file).unwrap();
            let backup = synthetic();
            let hands = Hands::away();
            let offers = Cell::new(0);
            let report = |event| {
                if matches!(event, Event::Restoring) {
                    offers.set(offers.get() + 1);
                }
            };

            let plan = rig.plan(&backup, Signing::Given(Box::new(key)));
            let restored = rig.run(plan, &hands, report).await.unwrap();

            assert!(
                matches!(restored, Restored::Refused(UpdateError::Refused(refusal))
                    if refusal != Refusal::Update(UpdateRefusal::Installing)),
                "{restored:?}"
            );
            assert_eq!(offers.get(), 1);
            assert_eq!(hands.asked.get(), 0, "a given key enrolls nothing");
            assert_eq!(rig.bank().await, 1, "the feeder never gave way");
            rig.sim.stop().await;
        });
    }

    /// The feeder gone once bank 0 has taken the offer: uncertain, with the
    /// key file gone and nothing sent again.
    #[test]
    fn gone_after_the_offer_is_uncertain() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let report = |event| {
                if matches!(event, Event::Written { .. }) {
                    sim.control(Control::Quit);
                }
            };

            let plan = rig.plan(&backup, Signing::Fresh);
            let restored = rig.run(plan, &hands, report).await.unwrap();

            assert_eq!(restored, Restored::Uncertain(Uncertainty::Gone));
            assert!(!rig.key_file().exists());
            rig.sim.stop().await;
        });
    }

    /// A stop before the offer says so, and leaves the key: the press it
    /// stopped waiting for may yet have enrolled it.
    #[test]
    fn a_stop_before_the_offer_keeps_the_key() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::away();
            let platform = Platform {
                prompter: &hands,
                discovery: Nowhere,
                keypair: Keypair::from_secret(OWNER),
                report: |_| {},
                offering: Offering::default(),
            };
            let plan = rig.plan(&backup, Signing::Fresh);

            // Stops as the press is waited for, before the 60 s window closes.
            let stop = async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Stop::Terminate
            };
            let ended = restore(plan, platform, stop).await;

            assert!(
                matches!(ended, Err(Error::RestoreStopped(Stop::Terminate))),
                "{ended:?}"
            );
            assert!(rig.key_file().exists(), "a press may yet have enrolled it");
            rig.sim.stop().await;
        });
    }

    /// A stop once the offer may be out is uncertain, and spends the key.
    #[test]
    fn a_stop_after_the_offer_is_uncertain() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let offering = Cell::new(false);
            let platform = Platform {
                prompter: &hands,
                discovery: Nowhere,
                keypair: Keypair::from_secret(OWNER),
                report: |event| {
                    if matches!(event, Event::Restoring) {
                        offering.set(true);
                    }
                },
                offering: Offering::default(),
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let stop = async {
                while !offering.get() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Stop::Terminate
            };

            let restored = restore(plan, platform, stop).await.unwrap();

            assert_eq!(
                restored,
                Restored::Uncertain(Uncertainty::Stopped(Stop::Terminate))
            );
            assert!(!rig.key_file().exists());
            rig.sim.stop().await;
        });
    }

    /// A file a killed run left half written was never enrolled: it is replaced,
    /// and the restore goes on.
    #[test]
    fn a_key_file_that_does_not_load_is_replaced() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            std::fs::create_dir_all(rig.key_file().parent().unwrap()).unwrap();
            std::fs::write(rig.key_file(), b"{\"version\":").unwrap();
            let hands = Hands::pressing(&rig.sim);

            let plan = rig.plan(&backup, Signing::Fresh);
            let restored = rig.run(plan, &hands, |_| {}).await.unwrap();

            assert_eq!(restored, Restored::Activated);
            assert_eq!(hands.asked.get(), 1);
            rig.sim.stop().await;
        });
    }

    /// A press from long ago is not consent to this restore: the listed key's
    /// file is spent by age, and only a new press enrolls a new one.
    #[test]
    fn a_key_older_than_an_hour_takes_a_new_press() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let report = |event| {
                if matches!(event, Event::KeyEnrollment { .. }) {
                    sim.control(Control::Quit);
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            assert!(rig.run(plan, &hands, report).await.is_err());
            let old = Key::load(&rig.key_file()).unwrap().id();

            let written = SystemTime::now() - KEY_LIFE - Duration::from_secs(1);
            std::fs::File::options()
                .write(true)
                .open(rig.key_file())
                .unwrap()
                .set_modified(written)
                .unwrap();

            let rig = rig.again().await;
            let hands = Hands::pressing(&rig.sim);
            let signed = Cell::new(None);
            let report = |event| {
                if let Event::RestoreSigned { key, .. } = event {
                    signed.set(Some(key));
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let second = rig.run(plan, &hands, report).await.unwrap();

            assert_eq!(second, Restored::Activated);
            assert_eq!(hands.asked.get(), 1, "a new press");
            assert_ne!(signed.get(), Some(old), "signed with a new key");
            rig.sim.stop().await;
        });
    }

    /// Stock's activation leaves what the next Wellfed boot acts on, as after
    /// the loader falls back: no clients, so the owner's key no longer opens
    /// a session, and the identity the backup is filed by.
    #[test]
    fn a_boot_after_a_restore_has_no_clients_and_the_same_identity() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let plan = rig.plan(&backup, Signing::Fresh);
            let restored = rig.run(plan, &hands, |_| {}).await.unwrap();
            assert_eq!(restored, Restored::Activated);
            let identity = rig.sim.identity;
            let address = rig.sim.address;
            rig.sim.stop().await;

            let sim = Running::start_with(&rig.flash, &rig.banks, None, None, address).await;

            assert_eq!(sim.identity, identity);
            let opened =
                Connection::open(&Keypair::from_secret(OWNER), identity, sim.address, [7; 32])
                    .await;
            assert!(opened.is_err(), "the owner's key is not listed");
            sim.stop().await;
        });
    }

    /// Power lost and back: the bit that refuses offers after a restart inside
    /// an install goes with it.
    async fn power_cycled(sim: &Running) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        sim.control(Control::Reboot);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    /// A power cut mid-write leaves its manifest pending on bank 0. The next
    /// run's own key makes another manifest, so it cancels the stale one once,
    /// then its one offer goes in.
    #[test]
    fn a_stale_pending_install_is_cancelled_once_and_the_retry_goes_in() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let cut = Cell::new(false);
            let report = |event| {
                if matches!(event, Event::Written { .. }) && !cut.replace(true) {
                    sim.control(Control::Quit);
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let first = rig.run(plan, &hands, report).await.unwrap();
            assert_eq!(first, Restored::Uncertain(Uncertainty::Gone));

            let rig = rig.again().await;
            power_cycled(&rig.sim).await;
            let hands = Hands::pressing(&rig.sim);
            let cancels = Cell::new(0);
            let report = |event| {
                if matches!(event, Event::StaleInstallCancelled) {
                    cancels.set(cancels.get() + 1);
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let second = rig.run(plan, &hands, report).await.unwrap();

            assert_eq!(second, Restored::Activated);
            assert_eq!(cancels.get(), 1);
            assert_eq!(
                &rig.banks.borrow().bank1()[..backup.payload().len()],
                backup.payload()
            );
            rig.sim.stop().await;
        });
    }

    /// A cancel that goes unanswered may or may not have landed: the run
    /// stops, with its one offer unspent.
    #[test]
    fn an_unanswered_cancel_stops_the_run_with_nothing_offered() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let cut = Cell::new(false);
            let report = |event| {
                if matches!(event, Event::Written { .. }) && !cut.replace(true) {
                    sim.control(Control::Quit);
                }
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            rig.run(plan, &hands, report).await.unwrap();

            let rig = rig.again().await;
            power_cycled(&rig.sim).await;
            let hands = Hands::pressing(&rig.sim);
            let sim = &rig.sim;
            let offered = Cell::new(false);
            let report = |event| match event {
                Event::StaleInstallCancelled => sim.control(Control::Quit),
                Event::Restoring => offered.set(true),
                _ => {}
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let ended = rig.run(plan, &hands, report).await;

            assert!(
                matches!(ended, Err(Error::RestoreCancelUnknown)),
                "{ended:?}"
            );
            assert!(!offered.get());
            rig.sim.stop().await;
        });
    }

    /// An install running now is not stale: a second run neither cancels it
    /// nor offers, and the first goes on to the end.
    #[test]
    fn a_running_install_stops_a_second_run_and_is_left_alone() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let key = RefCell::new(None);
            let written = Cell::new(false);
            let file = rig.key_file();
            let report = |event| match event {
                Event::RestoreSigned { .. } => {
                    *key.borrow_mut() = Key::load(&file).ok();
                }
                Event::Written { .. } => written.set(true),
                _ => {}
            };
            let plan = rig.plan(&backup, Signing::Fresh);
            let first = rig.run(plan, &hands, report);

            let second = async {
                while !written.get() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                let key = key.borrow_mut().take().unwrap();
                let plan = rig.plan(&backup, Signing::Given(Box::new(key)));
                let away = Hands::away();
                let report = |event| {
                    assert!(
                        !matches!(event, Event::StaleInstallCancelled | Event::Restoring),
                        "{event:?}"
                    );
                };
                rig.run(plan, &away, report).await
            };
            let (first, second) = tokio::join!(first, second);

            assert!(
                matches!(
                    second,
                    Err(Error::RestoreInstallRunning {
                        phase: UpdatePhase::Receiving
                    })
                ),
                "{second:?}"
            );
            assert_eq!(first.unwrap(), Restored::Activated);
            rig.sim.stop().await;
        });
    }

    /// A key file removed from under the run (an earlier screen's forget) is
    /// its own stop, before anything is offered.
    #[test]
    fn a_key_file_gone_before_the_offer_stops_with_nothing_offered() {
        paused(async {
            let rig = Rig::start().await;
            let backup = synthetic();
            let hands = Hands::pressing(&rig.sim);
            let file = rig.key_file();
            let report = |event| {
                if matches!(event, Event::RestoreSigned { .. }) {
                    std::fs::remove_file(&file).unwrap();
                }
            };

            let plan = rig.plan(&backup, Signing::Fresh);
            let ended = rig.run(plan, &hands, report).await;

            assert!(matches!(ended, Err(Error::RestoreKeyGone)), "{ended:?}");
            assert_eq!(rig.banks.borrow().bank1()[0], MARKER[0], "nothing offered");
            rig.sim.stop().await;
        });
    }

    /// Any failure once an offer may be out is uncertain, in its own words;
    /// before that it stays the failure it is.
    #[test]
    fn a_failure_after_the_offer_is_uncertain() {
        let offering = Offering::default();
        let failure = || Err(Error::RestoreNotFound);

        assert!(matches!(
            settled(failure(), &offering),
            Err(Error::RestoreNotFound)
        ));
        offering.set(true);
        assert!(matches!(
            settled(failure(), &offering),
            Ok(Restored::Uncertain(Uncertainty::Failed(_)))
        ));
    }

    /// Leaving forgets the key; with none there, there is nothing to do.
    #[test]
    fn forgetting_removes_the_key_file_and_minds_no_absence() {
        let dir = tempfile::tempdir().unwrap();
        let file = storage::restore_key_path(dir.path(), &[0xab; 32]);
        forget_key(dir.path(), &[0xab; 32]).unwrap();

        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"key").unwrap();
        forget_key(dir.path(), &[0xab; 32]).unwrap();
        assert!(!file.exists());
    }

    /// Only the pinned image signs, and a file of other bytes is no backup.
    #[test]
    fn a_backup_of_the_wrong_bytes_is_refused() {
        let target = || Target {
            identity: [0xab; 32],
            address: None,
            backups: PathBuf::new(),
            signing: Signing::Fresh,
            version: None,
        };

        assert!(StockBackup::open(b"not stock".to_vec()).is_err());
        let refused = Plan::new(&synthetic(), target());
        assert!(
            matches!(
                refused,
                Err(Error::Signing(wellfed_signing::Error::Frozen(
                    wellfed_signing::FrozenError::NotStock
                )))
            ),
            "{:?}",
            refused.err()
        );
    }
}
