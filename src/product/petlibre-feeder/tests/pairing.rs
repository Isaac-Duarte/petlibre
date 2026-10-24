//! FW45's admission rules against synthetic gestures and requests: a press
//! confirms setup and signer offers in time, invites are single-use, expire
//! and never grant more than their issuer holds, and the recovery window
//! needs an unlocked panel and admits one owner.

use petlibre_core::buttons::{Button, Buttons, Gesture};
use petlibre_core::store::clients::Role;
use petlibre_feeder::pairing::{Config, Event, INVITES, Pairing, Path, Refusal};
use petlibre_feeder::panel::Lock;

const CONFIRM: u32 = Config::DEFAULT.confirm_ms;
const INVITE: u32 = Config::DEFAULT.invite_ms;
const RECOVERY: u32 = Config::DEFAULT.recovery_ms;

fn press(button: Button) -> Gesture {
    Gesture::Down {
        chord: Buttons::of(button),
    }
}

fn recovery_hold() -> Gesture {
    Gesture::Hold {
        chord: Buttons::of(Button::Lock).with(Button::Dispense),
        level: 1,
    }
}

fn refused(path: Path, refusal: Refusal) -> Event {
    Event::Refused { path, refusal }
}

fn admit(path: Path, role: Role) -> Event {
    Event::Admit { path, role }
}

#[test]
fn the_configuration_is_valid() {
    assert!(Config::DEFAULT.valid());
    assert!(
        !Config {
            hold_level: 0,
            ..Config::DEFAULT
        }
        .valid()
    );
}

#[test]
fn a_dispense_press_confirms_setup_even_when_locked() {
    let mut p = Pairing::new(Config::DEFAULT);
    assert_eq!(p.setup(0), Event::Confirming { path: Path::Setup });
    assert_eq!(p.setup(10), refused(Path::Setup, Refusal::Busy));
    assert_eq!(p.gesture(100, press(Button::Mic), Lock::Locked), None);
    assert_eq!(
        p.gesture(CONFIRM - 1, press(Button::Dispense), Lock::Locked),
        Some(admit(Path::Setup, Role::Owner))
    );
    assert_eq!(
        p.gesture(CONFIRM, press(Button::Dispense), Lock::Locked),
        None,
        "nothing pending: the press goes to the panel"
    );
}

#[test]
fn an_unanswered_offer_expires() {
    let mut p = Pairing::new(Config::DEFAULT);
    p.setup(0);
    assert_eq!(p.tick(CONFIRM - 1), None);
    assert_eq!(
        p.tick(CONFIRM),
        Some(refused(Path::Setup, Refusal::Unconfirmed))
    );
    assert_eq!(p.tick(CONFIRM + 1), None);

    // A press arriving after the deadline but before the tick still fails.
    p.setup(1_000);
    assert_eq!(
        p.gesture(1_000 + CONFIRM, press(Button::Dispense), Lock::Unlocked),
        Some(refused(Path::Setup, Refusal::Unconfirmed))
    );
}

#[test]
fn only_an_owner_offers_a_signing_key() {
    let mut p = Pairing::new(Config::DEFAULT);
    assert_eq!(
        p.signer(0, Role::Member),
        refused(Path::Signer, Refusal::NotAllowed)
    );
    assert_eq!(
        p.signer(0, Role::Owner),
        Event::Confirming { path: Path::Signer }
    );
    assert_eq!(
        p.gesture(5, press(Button::Dispense), Lock::Unlocked),
        Some(admit(Path::Signer, Role::UpdateSigner))
    );
}

#[test]
fn invites_are_single_use_and_bounded_by_the_issuer() {
    let mut p = Pairing::new(Config::DEFAULT);
    assert_eq!(
        p.invite(0, Role::Member, Role::Owner, [1; 32]),
        refused(Path::Invite, Refusal::NotAllowed)
    );
    assert_eq!(
        p.invite(0, Role::Server, Role::Member, [1; 32]),
        refused(Path::Invite, Refusal::NotAllowed)
    );
    assert_eq!(
        p.invite(0, Role::Owner, Role::UpdateSigner, [1; 32]),
        refused(Path::Invite, Refusal::NotAllowed)
    );

    let Event::Invited { id } = p.invite(0, Role::Member, Role::Member, [7; 32]) else {
        panic!("a member invites a member");
    };
    assert_eq!(p.invite_secret(10, id), Some([7; 32]));
    assert_eq!(p.invite_secret(10, id + 1), None);
    assert_eq!(p.redeem(20, id), admit(Path::Invite, Role::Member));
    assert_eq!(p.redeem(30, id), refused(Path::Invite, Refusal::Unknown));
    assert_eq!(p.invite_secret(30, id), None);
}

#[test]
fn invites_expire_and_are_limited() {
    let mut p = Pairing::new(Config::DEFAULT);
    let ids: Vec<u32> = (0..INVITES)
        .map(|_| match p.invite(0, Role::Owner, Role::Server, [2; 32]) {
            Event::Invited { id } => id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        p.invite(1, Role::Owner, Role::Member, [3; 32]),
        refused(Path::Invite, Refusal::Full)
    );
    assert_eq!(p.invite_secret(INVITE - 1, ids[0]), Some([2; 32]));
    assert_eq!(p.invite_secret(INVITE, ids[0]), None);
    assert_eq!(
        p.redeem(INVITE, ids[1]),
        refused(Path::Invite, Refusal::Unknown)
    );
    assert!(matches!(
        p.invite(INVITE, Role::Owner, Role::Member, [3; 32]),
        Event::Invited { .. }
    ));
}

#[test]
fn recovery_needs_an_unlocked_panel_and_admits_one_owner() {
    let mut p = Pairing::new(Config::DEFAULT);
    assert_eq!(p.recover(0), refused(Path::Recovery, Refusal::Closed));
    assert_eq!(
        p.gesture(0, recovery_hold(), Lock::Locked),
        Some(refused(Path::Recovery, Refusal::Locked))
    );
    assert!(!p.recovery_open());

    assert_eq!(
        p.gesture(0, recovery_hold(), Lock::Unlocked),
        Some(Event::RecoveryOpened)
    );
    assert_eq!(p.recover(RECOVERY - 1), admit(Path::Recovery, Role::Owner));
    assert_eq!(
        p.recover(RECOVERY - 1),
        refused(Path::Recovery, Refusal::Closed)
    );
    assert_eq!(p.tick(RECOVERY), None, "already closed by the admission");
}

#[test]
fn an_unused_recovery_window_closes() {
    let mut p = Pairing::new(Config::DEFAULT);
    p.gesture(0, recovery_hold(), Lock::Unlocked);
    assert_eq!(p.tick(RECOVERY - 1), None);
    assert_eq!(p.tick(RECOVERY), Some(Event::RecoveryClosed));
    assert_eq!(
        p.recover(RECOVERY + 1),
        refused(Path::Recovery, Refusal::Closed)
    );
}

#[test]
fn a_lone_lock_hold_is_the_panels() {
    let mut p = Pairing::new(Config::DEFAULT);
    let hold = Gesture::Hold {
        chord: Buttons::of(Button::Lock),
        level: 1,
    };
    assert_eq!(p.gesture(0, hold, Lock::Unlocked), None);
}
