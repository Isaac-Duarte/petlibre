//! Protocol tests for the flash lease authority: startup sessions, token
//! mutations, phase and total deadlines, ordering faults, the close fence and
//! an exhaustive reordering and replay search.
//!
//! The property under test: only the lease owner may access flash, and only
//! after quiesce and the other side's park; actuation resumes only after
//! completion and release. A stale token, wrong sender or missed deadline
//! fails or faults; it never grants permission.

use petlibre_core::flash_coord::{
    FlashCoord, FlashFault, FlashLimits, FlashParty, FlashReply, FlashToken,
};

const LIMITS: FlashLimits = FlashLimits {
    quiesce_ms: 10,
    park_ms: 10,
    active_ms: 10,
    release_ms: 10,
    total_ms: 30,
};

fn other(party: FlashParty) -> FlashParty {
    if party == FlashParty::Local {
        FlashParty::Peer
    } else {
        FlashParty::Local
    }
}

fn ready(now: u32) -> FlashCoord {
    let mut c = FlashCoord::new(&LIMITS, 17, now).unwrap();
    assert!(!c.may_actuate(now));
    assert_eq!(c.ready(17, 29, true, now), FlashReply::Accepted);
    assert!(c.may_actuate(now));
    c
}

fn request(c: &mut FlashCoord, party: FlashParty, id: u64, now: u32) -> FlashToken {
    let session = if party == FlashParty::Local { 17 } else { 29 };
    assert_eq!(c.request(party, session, id, now), FlashReply::Accepted);
    assert!(!c.may_actuate(now));
    c.token
}

fn grant(c: &mut FlashCoord, t: &FlashToken, now: u32) {
    assert!(!c.may_access(t, t.owner, now));
    assert_eq!(c.quiesced(t, true, now), FlashReply::Accepted);
    assert!(!c.may_access(t, t.owner, now));
    assert_eq!(c.parked(t, other(t.owner), now), FlashReply::Accepted);
    assert!(c.may_access(t, t.owner, now));
    assert!(!c.may_access(t, other(t.owner), now));
    assert!(!c.may_actuate(now));
}

fn finish(c: &mut FlashCoord, t: &FlashToken, now: u32) {
    assert_eq!(c.complete(t, t.owner, now), FlashReply::Accepted);
    assert!(!c.may_access(t, t.owner, now));
    assert!(!c.may_actuate(now));
    assert_eq!(
        c.released(t, other(t.owner), true, now),
        FlashReply::Accepted
    );
    assert!(c.may_actuate(now));
}

#[test]
fn lifecycle_and_exclusion() {
    let mut c = ready(0);
    for side in 0..2u32 {
        let party = if side == 0 {
            FlashParty::Local
        } else {
            FlashParty::Peer
        };
        let t = request(&mut c, party, 1, side);
        assert_eq!(
            c.request(other(party), if side == 1 { 17 } else { 29 }, 2, side),
            FlashReply::Busy
        );

        grant(&mut c, &t, side);
        finish(&mut c, &t, side);
        assert_eq!(
            c.request(party, if side == 1 { 29 } else { 17 }, 1, side),
            FlashReply::Duplicate
        );
        assert!(!c.may_access(&t, party, side));
    }
}

#[test]
fn startup_requires_current_sessions_and_verified_outputs() {
    let mut c = FlashCoord::new(&LIMITS, 17, 0).unwrap();
    assert_eq!(c.request(FlashParty::Local, 17, 1, 0), FlashReply::Busy);
    assert_eq!(c.ready(16, 29, true, 0), FlashReply::Stale);
    assert!(!c.may_actuate(0));
    assert_eq!(c.ready(17, 29, false, 0), FlashReply::Failed);
    assert!(c.fault == FlashFault::Output && !c.may_actuate(0));

    let mut c = ready(0);
    assert_eq!(c.ready(17, 29, true, 1), FlashReply::Duplicate);
    assert_eq!(c.request(FlashParty::Peer, 30, 1, 1), FlashReply::Stale);
    assert!(c.may_actuate(1));
    assert_eq!(c.ready(17, 30, true, 1), FlashReply::Failed);
    assert!(c.fault == FlashFault::PeerLost && !c.may_actuate(1));

    let mut c = FlashCoord::new(&LIMITS, 18, 2).unwrap(); // Fixture models a fresh boot object.
    assert_eq!(c.ready(17, 29, true, 2), FlashReply::Stale);
    assert!(!c.may_actuate(2));
}

#[test]
fn token_mutations_cannot_authorize() {
    for field in 0..4 {
        let mut c = ready(0);
        let t = request(&mut c, FlashParty::Local, 1, 0);
        let mut stale = t;
        match field {
            0 => stale.authority_session += 1,
            1 => stale.peer_session += 1,
            2 => stale.lease += 1,
            _ => stale.owner = FlashParty::Peer,
        }

        assert_eq!(c.quiesced(&stale, true, 1), FlashReply::Stale);
        assert_eq!(c.parked(&stale, FlashParty::Peer, 1), FlashReply::Stale);
        assert!(!c.may_access(&stale, FlashParty::Local, 1));

        grant(&mut c, &t, 1);
        assert_eq!(
            c.released(&stale, FlashParty::Peer, true, 1),
            FlashReply::Stale
        );
        assert!(!c.may_actuate(1));

        finish(&mut c, &t, 2);
        let next = request(&mut c, FlashParty::Local, 2, 3);
        assert_ne!(next.lease, t.lease);
        assert_eq!(c.released(&t, FlashParty::Peer, true, 3), FlashReply::Stale);
        assert!(!c.may_actuate(3));
    }
}

#[test]
fn duplicates_never_extend_deadlines() {
    let mut c = ready(0);
    let t = request(&mut c, FlashParty::Local, 1, 0);
    grant(&mut c, &t, 1);
    for time in 2..=10 {
        assert_eq!(
            c.request(FlashParty::Local, 17, 1, time),
            FlashReply::Duplicate
        );
        assert_eq!(c.parked(&t, FlashParty::Peer, time), FlashReply::Duplicate);
        assert_eq!(c.quiesced(&t, true, time), FlashReply::Duplicate);
    }

    assert!(!c.may_access(&t, FlashParty::Local, 11));
    assert!(c.fault == FlashFault::Timeout && !c.may_actuate(11));
    assert_eq!(c.complete(&t, FlashParty::Local, 11), FlashReply::Failed);
    assert_eq!(
        c.released(&t, FlashParty::Peer, true, 11),
        FlashReply::Failed
    );
}

#[test]
fn each_phase_has_an_absolute_cutoff() {
    for stage in 0..5 {
        let mut c = FlashCoord::new(&LIMITS, 17, 0).unwrap();
        if stage > 0 {
            assert_eq!(c.ready(17, 29, true, 0), FlashReply::Accepted);
            request(&mut c, FlashParty::Peer, 1, 0);
        }

        let t = c.token;
        if stage > 1 {
            assert_eq!(c.quiesced(&t, true, 0), FlashReply::Accepted);
        }
        if stage > 2 {
            assert_eq!(c.parked(&t, FlashParty::Local, 0), FlashReply::Accepted);
        }
        if stage > 3 {
            assert_eq!(c.complete(&t, FlashParty::Peer, 0), FlashReply::Accepted);
        }

        assert!(c.poll(9));
        assert!(!c.poll(10) && c.fault == FlashFault::Timeout);
        assert!(!c.may_actuate(10));
    }
}

#[test]
fn total_time_is_not_renewed_by_phase_changes() {
    let mut c = FlashCoord::new(
        &FlashLimits {
            total_ms: 20,
            ..LIMITS
        },
        17,
        0,
    )
    .unwrap();

    assert_eq!(c.ready(17, 29, true, 0), FlashReply::Accepted);
    let t = request(&mut c, FlashParty::Local, 1, 0);
    assert_eq!(c.quiesced(&t, true, 8), FlashReply::Accepted);
    assert_eq!(c.parked(&t, FlashParty::Peer, 16), FlashReply::Accepted);
    assert_eq!(c.complete(&t, FlashParty::Local, 18), FlashReply::Accepted);
    assert_eq!(
        c.released(&t, FlashParty::Peer, true, 20),
        FlashReply::Failed
    );
    assert_eq!(c.fault, FlashFault::Timeout);
}

#[test]
fn wrong_sender_early_release_and_output_loss_fault() {
    for failure in 0..4 {
        let mut c = ready(0);
        let t = request(&mut c, FlashParty::Local, 1, 0);
        if failure == 0 {
            assert_eq!(c.parked(&t, FlashParty::Peer, 0), FlashReply::Failed);
        } else {
            grant(&mut c, &t, 0);
            let reply = match failure {
                1 => c.complete(&t, FlashParty::Peer, 0),
                2 => c.released(&t, FlashParty::Peer, true, 0),
                _ => c.quiesced(&t, false, 0),
            };
            assert_eq!(reply, FlashReply::Failed);
        }

        assert!(!c.may_actuate(0) && !c.may_access(&t, FlashParty::Local, 0));
        assert_eq!(
            c.fault,
            if failure == 3 {
                FlashFault::Output
            } else {
                FlashFault::Order
            }
        );
    }
}

#[test]
fn clock_wrap_backward_and_transport_fault() {
    let start = u32::MAX - 4;
    let mut c = ready(start);
    let t = request(&mut c, FlashParty::Peer, 1, start);
    grant(&mut c, &t, start + 1);
    finish(&mut c, &t, 1);
    assert!(!c.may_actuate(0) && c.fault == FlashFault::Clock);

    let mut c = ready(0);
    c.trip(FlashFault::Transport);
    c.trip(FlashFault::PeerLost);
    assert!(c.fault == FlashFault::Transport && !c.may_actuate(1));
    assert_eq!(c.close(1), FlashReply::Failed);
}

#[test]
fn close_requires_current_fence_and_disabled_peer() {
    let mut c = ready(0);
    let old = request(&mut c, FlashParty::Local, 1, 0);
    grant(&mut c, &old, 0);
    finish(&mut c, &old, 0);
    assert_eq!(c.close(1), FlashReply::Accepted);

    let fence = c.token;
    assert!(fence.owner == FlashParty::Fence && fence.lease != old.lease);
    assert_eq!(c.closed(&old, true, true, 1), FlashReply::Stale);
    assert_eq!(c.closed(&fence, false, true, 1), FlashReply::Busy);
    assert!(!c.may_actuate(1));
    assert_eq!(c.request(FlashParty::Peer, 29, 1, 1), FlashReply::Busy);
    assert_eq!(c.closed(&fence, true, true, 2), FlashReply::Accepted);
    assert!(!c.may_actuate(2));
    assert!(!c.may_access(&old, FlashParty::Local, 2));
}

#[test]
fn configuration_and_counter_exhaustion() {
    assert!(
        FlashCoord::new(
            &FlashLimits {
                total_ms: 9,
                ..LIMITS
            },
            17,
            0
        )
        .is_none()
    );
    assert!(
        FlashCoord::new(
            &FlashLimits {
                park_ms: 0x8000_0000,
                ..LIMITS
            },
            17,
            0
        )
        .is_none()
    );
    assert!(FlashCoord::new(&LIMITS, 0, 0).is_none());

    let mut c = ready(0);
    assert_eq!(c.request(FlashParty::Fence, 17, 1, 0), FlashReply::Invalid);
    assert_eq!(c.request(FlashParty::Local, 17, 0, 0), FlashReply::Invalid);
    c.token.lease = u64::MAX;
    assert_eq!(c.request(FlashParty::Local, 17, 1, 0), FlashReply::Failed);
    assert!(c.fault == FlashFault::Exhausted && !c.may_actuate(0));
}

/// Exhaust all six-event traces over five messages, for BOTH flash owners.
/// The oracle records protocol facts independently of the implementation phase.
#[test]
fn exhaustive_reordering_and_replay_cannot_create_permission() {
    for side in 0..2 {
        for trace in 0..15_625u32 {
            let mut c = ready(0);
            let who = if side == 0 {
                FlashParty::Local
            } else {
                FlashParty::Peer
            };
            let t = request(&mut c, who, 1, 0);
            let stale = FlashToken {
                lease: t.lease + 1,
                ..t
            };
            let (mut quiet, mut parked, mut complete, mut released) = (false, false, false, false);
            let mut events = trace;

            for step in 1..=6 {
                let event = events % 5;
                events /= 5;
                match event {
                    0 => quiet |= c.quiesced(&t, true, step) == FlashReply::Accepted,
                    1 => {
                        if c.parked(&t, other(who), step) == FlashReply::Accepted {
                            assert!(quiet);
                            parked = true;
                        }
                    }
                    2 => {
                        if c.complete(&t, who, step) == FlashReply::Accepted {
                            assert!(quiet && parked);
                            complete = true;
                        }
                    }
                    3 => {
                        if c.released(&t, other(who), true, step) == FlashReply::Accepted {
                            assert!(complete);
                            released = true;
                        }
                    }
                    _ => {
                        c.parked(&stale, other(who), step);
                    }
                }

                if c.may_access(&t, who, step) {
                    assert!(quiet && parked && !complete);
                }
                assert!(!c.may_access(&t, other(who), step));
                if c.may_actuate(step) {
                    assert!(released && complete);
                }
            }
        }
    }
}
