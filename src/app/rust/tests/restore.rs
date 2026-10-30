//! A restore's bridging, with no Flutter: the run on its own thread, the
//! press and the lookup answered through the same updates the screens hear,
//! and the simulated feeder on the far end. What it proves is the plumbing and
//! how each end reads on the screens. The steps' decisions are
//! `wellfed_install::restore`'s tests.

use std::cell::RefCell;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::LocalSet;
use wellfed_app_bridge::api::install::{InstallAnswer, InstallAsk, InstallEvent};
use wellfed_app_bridge::api::restore::{
    BackupCheck, RestoreEnd, RestoreSpec, RestoreStop, RestoreUpdate, UncertainWhy,
};
use wellfed_app_bridge::restore::{RestoreThread, check_backup};
use wellfed_client::install::{StockBackup, StockPins};
use wellfed_core::ota::{MARKER, SECTOR};
use wellfed_feeder_sim::banks::Banks;
use wellfed_feeder_sim::flash::Flash;
use wellfed_feeder_sim::harness::Running;
use wellfed_formats::credentials::KeyScope;
use wellfed_install::restore::{Plan, Signing, Target};
use wellfed_session::noise::Keypair;
use wellfed_signing::Key;

const WAIT: Duration = Duration::from_secs(30);
const OWNER: [u8; 32] = [0x31; 32];

fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(LocalSet::new().run_until(test));
}

fn synthetic() -> StockBackup {
    let mut bank: Vec<u8> = (0..3 * SECTOR).map(|i| (i % 251) as u8).collect();
    bank[..MARKER.len()].copy_from_slice(&MARKER);
    let pins = StockPins::of_payload(&bank);
    StockBackup::from_bank_pinned(bank, &pins).unwrap()
}

struct Rig {
    sim: Running,
    banks: Rc<RefCell<Banks>>,
    dir: tempfile::TempDir,
}

impl Rig {
    async fn start() -> Self {
        let banks = Banks::new();
        let sim = Running::start_with(
            &Flash::memory(),
            &banks,
            None,
            Some(*Keypair::from_secret(OWNER).public()),
            SocketAddr::from(([127, 0, 0, 1], 0)),
        )
        .await;
        sim.type_line("hold lock").unwrap();
        Self {
            sim,
            banks,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn target(&self, address: Option<SocketAddr>, signing: Signing) -> Target {
        Target {
            identity: self.sim.identity,
            address,
            backups: self.dir.path().join("backups"),
            signing,
            version: None,
        }
    }
}

fn started(plan: Plan) -> (RestoreThread, mpsc::UnboundedReceiver<RestoreUpdate>) {
    let run = RestoreThread::new();
    let (tell, heard) = mpsc::unbounded_channel();
    run.start_plan(Ok(plan), Keypair::from_secret(OWNER), move |update| {
        tell.send(update).is_ok()
    });
    (run, heard)
}

async fn next(heard: &mut mpsc::UnboundedReceiver<RestoreUpdate>) -> RestoreUpdate {
    tokio::time::timeout(WAIT, heard.recv())
        .await
        .expect("a restore goes quiet")
        .expect("a restore ends with its end")
}

/// The whole of it from bank 1: the press asked for and given, bank 1 giving
/// way, stock written, the stream closed after its end.
#[test]
fn a_restore_presses_writes_and_ends_activated() {
    local(async {
        let rig = Rig::start().await;
        let backup = synthetic();
        let plan = Plan::synthetic(&backup, rig.target(Some(rig.sim.address), Signing::Fresh));
        let (_run, mut heard) = started(plan);

        let (mut pressed, mut promoting) = (0, false);
        let end = loop {
            match next(&mut heard).await {
                RestoreUpdate::Event {
                    event: InstallEvent::PressDispense,
                } => {
                    pressed += 1;
                    rig.sim.type_line("press dispense").unwrap();
                }
                RestoreUpdate::Event {
                    event: InstallEvent::Promoting,
                } => promoting = true,
                RestoreUpdate::Event { .. } => {}
                RestoreUpdate::Ask { ask, .. } => panic!("{ask:?}"),
                RestoreUpdate::Ended { end } => break end,
            }
        };

        assert_eq!(end, RestoreEnd::Activated);
        assert_eq!(pressed, 1);
        assert!(promoting);
        assert_eq!(
            &rig.banks.borrow().bank1()[..backup.payload().len()],
            backup.payload()
        );
        assert!(heard.recv().await.is_none(), "the stream closes at the end");
        rig.sim.stop().await;
    });
}

/// With no address, the run asks Dart where the feeder is; no answer, and it
/// says the feeder does not answer, with nothing offered.
#[test]
fn a_feeder_nobody_can_find_is_asked_for_and_not_answering() {
    local(async {
        let rig = Rig::start().await;
        let plan = Plan::synthetic(&synthetic(), rig.target(None, Signing::Fresh));
        let (run, mut heard) = started(plan);

        let RestoreUpdate::Ask { id, ask } = next(&mut heard).await else {
            panic!("the lookup comes first");
        };
        let InstallAsk::Locate { identity } = ask else {
            panic!("{ask:?}");
        };
        assert_eq!(identity, wellfed_formats::hex::encode(&rig.sim.identity));
        run.answer(id, InstallAnswer::Located { address: None });

        let RestoreUpdate::Ended {
            end: RestoreEnd::Stopped { case, .. },
        } = next(&mut heard).await
        else {
            panic!("it ends");
        };
        assert_eq!(case, RestoreStop::NotAnswering);
        rig.sim.stop().await;
    });
}

/// A refused offer reads as refused, and asks for nothing more.
#[test]
fn a_refused_offer_reads_as_refused() {
    local(async {
        let rig = Rig::start().await;
        let file = rig.dir.path().join("key.json");
        wellfed_signing::keygen(&file, KeyScope::Owner).unwrap();
        let signing = Signing::Given(Box::new(Key::load(&file).unwrap()));
        let plan = Plan::synthetic(&synthetic(), rig.target(Some(rig.sim.address), signing));
        let (_run, mut heard) = started(plan);

        let end = loop {
            if let RestoreUpdate::Ended { end } = next(&mut heard).await {
                break end;
            }
        };
        assert!(
            matches!(
                end,
                RestoreEnd::Stopped {
                    case: RestoreStop::Refused,
                    ..
                }
            ),
            "{end:?}"
        );
        rig.sim.stop().await;
    });
}

/// A stop while the press is waited for reads as cancelled; once the offer
/// may be out, as uncertain.
#[test]
fn a_stop_reads_by_whether_an_offer_may_be_out() {
    local(async {
        let rig = Rig::start().await;
        let plan = Plan::synthetic(
            &synthetic(),
            rig.target(Some(rig.sim.address), Signing::Fresh),
        );
        let (run, mut heard) = started(plan);
        let end = loop {
            match next(&mut heard).await {
                RestoreUpdate::Event {
                    event: InstallEvent::PressDispense,
                } => run.stop(),
                RestoreUpdate::Ended { end } => break end,
                _ => {}
            }
        };
        assert!(
            matches!(
                end,
                RestoreEnd::Stopped {
                    case: RestoreStop::Cancelled,
                    ..
                }
            ),
            "{end:?}"
        );

        // The key the stopped run left: this one needs no press.
        let plan = Plan::synthetic(
            &synthetic(),
            rig.target(Some(rig.sim.address), Signing::Fresh),
        );
        let (run, mut heard) = started(plan);
        let end = loop {
            match next(&mut heard).await {
                RestoreUpdate::Event {
                    event: InstallEvent::PressDispense,
                } => rig.sim.type_line("press dispense").unwrap(),
                RestoreUpdate::Event {
                    event: InstallEvent::Promoting,
                } => run.stop(),
                RestoreUpdate::Ended { end } => break end,
                _ => {}
            }
        };
        assert!(
            matches!(
                end,
                RestoreEnd::Uncertain {
                    why: UncertainWhy::Stopped,
                    ..
                }
            ),
            "{end:?}"
        );
        rig.sim.stop().await;
    });
}

/// What a picked file goes through: a foreign one is refused by the same
/// check as the kept one, before anything starts.
#[test]
fn a_file_of_other_bytes_is_refused_before_anything_starts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("picked.bin");
    assert_eq!(check_backup(&file), BackupCheck::Missing);

    std::fs::write(&file, b"not stock").unwrap();
    assert!(matches!(check_backup(&file), BackupCheck::NotStock { .. }));

    local(async {
        let run = RestoreThread::new();
        let (tell, mut heard) = mpsc::unbounded_channel();
        let spec = RestoreSpec {
            identity: vec![0xab; 32],
            address: None,
            data_dir: Path::new(dir.path()).display().to_string(),
            backup: file.display().to_string(),
        };
        run.start(Some(spec), Keypair::from_secret(OWNER), move |update| {
            tell.send(update).is_ok()
        });
        let RestoreUpdate::Ended {
            end: RestoreEnd::Stopped { case, .. },
        } = next(&mut heard).await
        else {
            panic!("it ends at once");
        };
        assert_eq!(case, RestoreStop::BackupRefused);
    });
}
