//! What a trial build's confirmation waits for: the feeding owner open on
//! its journal and at rest, every check passing, for `PROOF_MS`; and what
//! makes the trial fail at once instead.

use wellfed_core::feed::food_read::{FOOD_READ_FIRST_MS, FoodReads, FoodStep, FoodView};
use wellfed_fw_feeder::feeding::{InFlight, Opened, PROOF_MS, Proof, food_read_held};
use wellfed_sdk::Feeding;
use wellfed_sdk::hal::owner::{OwnerFault, OwnerState, OwnerStatus};
use wellfed_sdk::hal::supervision::events::EventsFault;

fn resting() -> OwnerStatus {
    OwnerStatus {
        state: OwnerState::Idle,
        stopped: true,
        optics_off: true,
        ..OwnerStatus::default()
    }
}

fn owner(status: &OwnerStatus) -> Opened<'_> {
    Opened::Owner {
        status,
        resolving: false,
    }
}

#[test]
fn an_owner_at_rest_with_its_checks_passing_proves_itself_after_the_window() {
    let mut proof = Proof::default();
    let status = resting();

    assert_eq!(proof.step(Opened::Not, 0), Feeding::Pending);
    assert_eq!(proof.step(owner(&status), 1_000), Feeding::Pending);
    assert_eq!(
        proof.step(owner(&status), 1_000 + PROOF_MS - 1),
        Feeding::Pending
    );
    assert_eq!(
        proof.step(owner(&status), 1_000 + PROOF_MS),
        Feeding::Proven
    );
}

/// Why a trial holds food readings: lit emitters are a lapse in its proof. So
/// the firmware holds them while the build is confirming, the trial proves
/// itself with no reading in its way, and readings start only after.
#[test]
fn a_trial_holds_food_readings_so_it_still_proves_itself() {
    let mut proof = Proof::default();
    let lit = OwnerStatus {
        optics_off: false,
        food_reading: true,
        ..resting()
    };
    assert_eq!(proof.step(owner(&resting()), 0), Feeding::Pending);
    assert_eq!(proof.step(owner(&lit), PROOF_MS - 100), Feeding::Pending);
    assert_eq!(
        proof.step(owner(&resting()), PROOF_MS),
        Feeding::Pending,
        "a reading would start the window again"
    );

    // Confirming: the schedule never asks, however long the trial takes, and
    // the owner's proof runs its window out.
    let mut reads = FoodReads::new();
    let mut proof = Proof::default();
    let held = food_read_held(true, InFlight::Nothing, false, false);
    let view = FoodView::default();
    let mut proven = false;
    for now in (0..2 * PROOF_MS + FOOD_READ_FIRST_MS).step_by(20) {
        assert_eq!(reads.step(now, view, held), FoodStep::Rest);
        proven = proof.step(owner(&resting()), now) == Feeding::Proven;
    }
    assert!(proven);

    // Confirmed: the first reading comes after its usual wait.
    let confirmed = 2 * PROOF_MS + FOOD_READ_FIRST_MS;
    let held = food_read_held(false, InFlight::Nothing, false, false);
    assert_eq!(reads.step(confirmed, view, held), FoodStep::Rest);
    assert_eq!(
        reads.step(confirmed + FOOD_READ_FIRST_MS, view, held),
        FoodStep::Start
    );
}

/// A feed in flight, one being resolved and work queued for a writer each
/// hold readings too.
#[test]
fn a_feed_or_queued_work_holds_food_readings() {
    assert!(food_read_held(false, InFlight::Feed, false, false));
    assert!(food_read_held(false, InFlight::Nothing, true, false));
    assert!(food_read_held(false, InFlight::Nothing, false, true));
    assert!(!food_read_held(false, InFlight::Nothing, false, false));
}

#[test]
fn any_lapse_starts_the_window_again() {
    let ok = resting();
    let lapses = [
        OwnerStatus {
            state: OwnerState::Starting,
            ..ok
        },
        OwnerStatus {
            state: OwnerState::Terminal,
            ..ok
        },
        OwnerStatus {
            stopped: false,
            ..ok
        },
        OwnerStatus {
            optics_off: false,
            ..ok
        },
        OwnerStatus {
            supervisor_fault: EventsFault::Feedback,
            ..ok
        },
    ];
    for lapse in lapses {
        let mut proof = Proof::default();
        proof.step(owner(&ok), 0);
        assert_eq!(proof.step(owner(&lapse), PROOF_MS / 2), Feeding::Pending);
        assert_eq!(proof.step(owner(&ok), PROOF_MS), Feeding::Pending);
        assert_eq!(proof.step(owner(&ok), PROOF_MS * 2), Feeding::Proven);
    }

    // An interrupted feed still being recorded as uncertain.
    let mut proof = Proof::default();
    let resolving = Opened::Owner {
        status: &ok,
        resolving: true,
    };
    assert_eq!(proof.step(resolving, 0), Feeding::Pending);
    assert_eq!(proof.step(resolving, PROOF_MS * 3), Feeding::Pending);
}

#[test]
fn an_owner_that_cannot_open_has_failed() {
    let mut proof = Proof::default();
    assert_eq!(proof.step(Opened::JournalFailed, 0), Feeding::Failed);

    for fault in [
        OwnerFault::Setup,
        OwnerFault::Recovery,
        OwnerFault::Storage,
        OwnerFault::Hardware,
        OwnerFault::Progress,
        OwnerFault::Flash,
    ] {
        let status = OwnerStatus {
            state: OwnerState::Fault,
            fault,
            ..resting()
        };
        assert_eq!(proof.step(owner(&status), 0), Feeding::Failed, "{fault:?}");
    }
}
