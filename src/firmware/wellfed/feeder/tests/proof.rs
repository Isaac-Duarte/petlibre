//! What a trial build's confirmation waits for: the feeding owner open on
//! its journal and at rest, every check passing, for `PROOF_MS`; and what
//! makes the trial fail at once instead.

use wellfed_fw_feeder::feeding::{Opened, PROOF_MS, Proof};
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
