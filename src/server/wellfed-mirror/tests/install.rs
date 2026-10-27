//! An install on the long lane against the simulator's update path: the
//! firmware's own update stage on two simulated banks. Bank 1 gives way,
//! the feeder restarts into bank 0 and the task reconnects on its own; the
//! transfer then runs a chunk per step, with feeds and status reads
//! answered between the steps, and resumes across a cut session.

mod support;

use std::cell::{Cell, RefCell};
use std::time::Duration;

use wellfed_api::{ClientRole, Connection as Link};
use wellfed_mirror::{MirrorEvent, Pushed, Update};
use wellfed_model::{Answer, Op};
use wellfed_server_testkit::feeder::{self, Banks, Relay};
use wellfed_server_testkit::release::{self, MANIFEST_LEN, SECTOR};
use wellfed_server_testkit::{TempStore, WAIT, local, until};

use support::{ID, mirrored_as, next};

const BUILD: [u8; 32] = [0x77; 32];
const LEN: usize = 16 * SECTOR;

/// Wait until the transfer has written at least `bytes`.
async fn written_at_least(written: &Cell<u32>, bytes: u32) {
    while written.get() < bytes {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Wait for the feeder to restart into the release, as it does once bank 0
/// has activated bank 1.
async fn restarted_into_the_release(banks: &RefCell<Banks>) {
    until("the feeder restarts into the release", || async {
        banks.borrow().bank1_build == BUILD
    })
    .await;
}

#[test]
fn a_feed_and_a_status_read_are_answered_during_the_transfer() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;
        let file = release::signed(&release::project(), 7, BUILD, LEN);
        let written = Cell::new(0);

        let mut operation = mirror.operation(ID).await.unwrap();
        let update = Update {
            file: &file,
            build: BUILD,
        };
        let install = operation.install(update, |view| written.set(view.written));
        let shorts = async {
            written_at_least(&written, 1).await;
            let fed = mirror.ask(ID, [0xf1; 16], Op::Feed { portions: 1 }).await;
            let status = mirror.ask(ID, [0xf2; 16], Op::Status).await;
            // The simulator has no feeding owner, so it refuses the feed
            // (`unavailable`): an answer all the same.
            assert!(fed.is_ok(), "{fed:?}");
            assert!(matches!(status, Ok(Answer::Status(_))), "{status:?}");
            written.get()
        };
        let (pushed, answered_at) = tokio::join!(install, shorts);

        assert_eq!(pushed, Ok(Pushed::Activated));
        assert!(
            usize::try_from(answered_at).unwrap() < LEN,
            "both answered mid-transfer, at {answered_at} of {LEN} bytes"
        );
        restarted_into_the_release(&banks).await;
        assert_eq!(&banks.borrow().bank1()[..LEN], &file[MANIFEST_LEN..]);
        drop(operation);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// The session is cut a sector into the transfer, the feeder still
/// running: the task reconnects, the driver reads where bank 0 stands and
/// sends what it still wants, and the install activates.
#[test]
fn a_session_cut_mid_transfer_resumes_and_activates() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let relay = Relay::start(sim.address).await;
        let address = relay.address.to_string();
        let (mirror, mut updates) =
            mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;
        let file = release::signed(&release::project(), 7, BUILD, LEN);
        let written = Cell::new(0);

        let mut operation = mirror.operation(ID).await.unwrap();
        let update = Update {
            file: &file,
            build: BUILD,
        };
        let install = operation.install(update, |view| written.set(view.written));
        let cut = async {
            written_at_least(&written, u32::try_from(SECTOR).unwrap()).await;
            // Only what the cut does from here: the feeder's restart into
            // bank 0 came before the first byte was written.
            updates = updates.resubscribe();
            relay.cut();
            let cut_at = written.get();
            while next(&mut updates).await != MirrorEvent::Link(Link::Connecting) {}
            cut_at
        };
        let (pushed, cut_at) = tokio::join!(install, cut);

        assert_eq!(pushed, Ok(Pushed::Activated));
        assert!(
            usize::try_from(cut_at).unwrap() < LEN,
            "cut mid-transfer, at {cut_at} of {LEN} bytes"
        );
        restarted_into_the_release(&banks).await;
        assert_eq!(&banks.borrow().bank1()[..LEN], &file[MANIFEST_LEN..]);
        drop(operation);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}

/// Once the feeder has restarted into the release, installing it again
/// finds bank 1 running its build on the first status read: the driver's
/// check for an activation whose answer was lost, here before any offer,
/// so nothing is sent and the install says the build was already running.
#[test]
fn installing_the_running_build_again_sends_nothing() {
    local(async {
        let temp = TempStore::new();
        let store = temp.open().await;
        let banks = Banks::new();
        let sim = feeder::start_updatable(&banks).await;
        let address = sim.address.to_string();
        let (mirror, _updates) = mirrored_as(&store, &sim, &address, WAIT, ClientRole::Owner).await;
        let file = release::signed(&release::project(), 7, BUILD, LEN);
        let update = Update {
            file: &file,
            build: BUILD,
        };
        let mut operation = mirror.operation(ID).await.unwrap();

        let first = operation.install(update, |_| {}).await;
        assert_eq!(first, Ok(Pushed::Activated));
        restarted_into_the_release(&banks).await;

        let progressed = Cell::new(false);
        let again = operation.install(update, |_| progressed.set(true)).await;

        assert_eq!(again, Ok(Pushed::AlreadyRunning));
        assert!(!progressed.get(), "no update request was answered");
        assert_eq!(&banks.borrow().bank1()[..LEN], &file[MANIFEST_LEN..]);
        drop(operation);
        mirror.shutdown().await;
        store.close().await;
        sim.stop().await;
    });
}
