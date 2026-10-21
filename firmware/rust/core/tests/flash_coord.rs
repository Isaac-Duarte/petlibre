use petlibro_core::flash_coord::{
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
    if party == FlashParty::LOCAL {
        FlashParty::PEER
    } else {
        FlashParty::LOCAL
    }
}

fn ready(now: u32) -> FlashCoord {
    let mut c = FlashCoord::new(&LIMITS, 17, now).unwrap();
    assert!(!c.may_actuate(now));
    assert_eq!(c.ready(17, 29, true, now), FlashReply::ACCEPTED);
    assert!(c.may_actuate(now));
    c
}

fn request(c: &mut FlashCoord, party: FlashParty, id: u64, now: u32) -> FlashToken {
    let session = if party == FlashParty::LOCAL { 17 } else { 29 };
    assert_eq!(c.request(party, session, id, now), FlashReply::ACCEPTED);
    assert!(!c.may_actuate(now));
    c.token
}

fn grant(c: &mut FlashCoord, t: &FlashToken, now: u32) {
    assert!(!c.may_access(t, t.owner, now));
    assert_eq!(c.quiesced(t, true, now), FlashReply::ACCEPTED);
    assert!(!c.may_access(t, t.owner, now));
    assert_eq!(c.parked(t, other(t.owner), now), FlashReply::ACCEPTED);
    assert!(c.may_access(t, t.owner, now));
    assert!(!c.may_access(t, other(t.owner), now));
    assert!(!c.may_actuate(now));
}

fn finish(c: &mut FlashCoord, t: &FlashToken, now: u32) {
    assert_eq!(c.complete(t, t.owner, now), FlashReply::ACCEPTED);
    assert!(!c.may_access(t, t.owner, now));
    assert!(!c.may_actuate(now));
    assert_eq!(
        c.released(t, other(t.owner), true, now),
        FlashReply::ACCEPTED
    );
    assert!(c.may_actuate(now));
}

#[test]
fn lifecycle_and_exclusion() {
    let mut c = ready(0);
    for side in 0..2u32 {
        let party = FlashParty(side);
        let t = request(&mut c, party, 1, side);
        assert_eq!(
            c.request(other(party), if side == 1 { 17 } else { 29 }, 2, side),
            FlashReply::BUSY
        );
        grant(&mut c, &t, side);
        finish(&mut c, &t, side);
        assert_eq!(
            c.request(party, if side == 1 { 29 } else { 17 }, 1, side),
            FlashReply::DUPLICATE
        );
        assert!(!c.may_access(&t, party, side));
    }
}

#[test]
fn startup_requires_current_sessions_and_verified_outputs() {
    let mut c = FlashCoord::new(&LIMITS, 17, 0).unwrap();
    assert_eq!(c.request(FlashParty::LOCAL, 17, 1, 0), FlashReply::BUSY);
    assert_eq!(c.ready(16, 29, true, 0), FlashReply::STALE);
    assert!(!c.may_actuate(0));
    assert_eq!(c.ready(17, 29, false, 0), FlashReply::FAILED);
    assert!(c.fault == FlashFault::OUTPUT && !c.may_actuate(0));
    let mut c = ready(0);
    assert_eq!(c.ready(17, 29, true, 1), FlashReply::DUPLICATE);
    assert_eq!(c.request(FlashParty::PEER, 30, 1, 1), FlashReply::STALE);
    assert!(c.may_actuate(1));
    assert_eq!(c.ready(17, 30, true, 1), FlashReply::FAILED);
    assert!(c.fault == FlashFault::PEER_LOST && !c.may_actuate(1));
    let mut c = FlashCoord::new(&LIMITS, 18, 2).unwrap(); // Fixture models a fresh boot object.
    assert_eq!(c.ready(17, 29, true, 2), FlashReply::STALE);
    assert!(!c.may_actuate(2));
}

#[test]
fn token_mutations_cannot_authorize() {
    for field in 0..4 {
        let mut c = ready(0);
        let t = request(&mut c, FlashParty::LOCAL, 1, 0);
        let mut stale = t;
        match field {
            0 => stale.authority_session += 1,
            1 => stale.peer_session += 1,
            2 => stale.lease += 1,
            _ => stale.owner = FlashParty::PEER,
        }
        assert_eq!(c.quiesced(&stale, true, 1), FlashReply::STALE);
        assert_eq!(c.parked(&stale, FlashParty::PEER, 1), FlashReply::STALE);
        assert!(!c.may_access(&stale, FlashParty::LOCAL, 1));
        grant(&mut c, &t, 1);
        assert_eq!(
            c.released(&stale, FlashParty::PEER, true, 1),
            FlashReply::STALE
        );
        assert!(!c.may_actuate(1));
        finish(&mut c, &t, 2);
        let next = request(&mut c, FlashParty::LOCAL, 2, 3);
        assert_ne!(next.lease, t.lease);
        assert_eq!(c.released(&t, FlashParty::PEER, true, 3), FlashReply::STALE);
        assert!(!c.may_actuate(3));
    }
}

#[test]
fn duplicates_never_extend_deadlines() {
    let mut c = ready(0);
    let t = request(&mut c, FlashParty::LOCAL, 1, 0);
    grant(&mut c, &t, 1);
    for time in 2..=10 {
        assert_eq!(
            c.request(FlashParty::LOCAL, 17, 1, time),
            FlashReply::DUPLICATE
        );
        assert_eq!(c.parked(&t, FlashParty::PEER, time), FlashReply::DUPLICATE);
        assert_eq!(c.quiesced(&t, true, time), FlashReply::DUPLICATE);
    }
    assert!(!c.may_access(&t, FlashParty::LOCAL, 11));
    assert!(c.fault == FlashFault::TIMEOUT && !c.may_actuate(11));
    assert_eq!(c.complete(&t, FlashParty::LOCAL, 11), FlashReply::FAILED);
    assert_eq!(
        c.released(&t, FlashParty::PEER, true, 11),
        FlashReply::FAILED
    );
}

#[test]
fn each_phase_has_an_absolute_cutoff() {
    for stage in 0..5 {
        let mut c = FlashCoord::new(&LIMITS, 17, 0).unwrap();
        if stage > 0 {
            assert_eq!(c.ready(17, 29, true, 0), FlashReply::ACCEPTED);
            request(&mut c, FlashParty::PEER, 1, 0);
        }
        let t = c.token;
        if stage > 1 {
            assert_eq!(c.quiesced(&t, true, 0), FlashReply::ACCEPTED);
        }
        if stage > 2 {
            assert_eq!(c.parked(&t, FlashParty::LOCAL, 0), FlashReply::ACCEPTED);
        }
        if stage > 3 {
            assert_eq!(c.complete(&t, FlashParty::PEER, 0), FlashReply::ACCEPTED);
        }
        assert!(c.poll(9));
        assert!(!c.poll(10) && c.fault == FlashFault::TIMEOUT);
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
    assert_eq!(c.ready(17, 29, true, 0), FlashReply::ACCEPTED);
    let t = request(&mut c, FlashParty::LOCAL, 1, 0);
    assert_eq!(c.quiesced(&t, true, 8), FlashReply::ACCEPTED);
    assert_eq!(c.parked(&t, FlashParty::PEER, 16), FlashReply::ACCEPTED);
    assert_eq!(c.complete(&t, FlashParty::LOCAL, 18), FlashReply::ACCEPTED);
    assert_eq!(
        c.released(&t, FlashParty::PEER, true, 20),
        FlashReply::FAILED
    );
    assert_eq!(c.fault, FlashFault::TIMEOUT);
}

#[test]
fn wrong_sender_early_release_and_output_loss_fault() {
    for failure in 0..4 {
        let mut c = ready(0);
        let t = request(&mut c, FlashParty::LOCAL, 1, 0);
        if failure == 0 {
            assert_eq!(c.parked(&t, FlashParty::PEER, 0), FlashReply::FAILED);
        } else {
            grant(&mut c, &t, 0);
            let reply = match failure {
                1 => c.complete(&t, FlashParty::PEER, 0),
                2 => c.released(&t, FlashParty::PEER, true, 0),
                _ => c.quiesced(&t, false, 0),
            };
            assert_eq!(reply, FlashReply::FAILED);
        }
        assert!(!c.may_actuate(0) && !c.may_access(&t, FlashParty::LOCAL, 0));
        assert_eq!(
            c.fault,
            if failure == 3 {
                FlashFault::OUTPUT
            } else {
                FlashFault::ORDER
            }
        );
    }
}

#[test]
fn clock_wrap_backward_and_transport_fault() {
    let start = u32::MAX - 4;
    let mut c = ready(start);
    let t = request(&mut c, FlashParty::PEER, 1, start);
    grant(&mut c, &t, start + 1);
    finish(&mut c, &t, 1);
    assert!(!c.may_actuate(0) && c.fault == FlashFault::CLOCK);
    let mut c = ready(0);
    c.trip(FlashFault::TRANSPORT);
    c.trip(FlashFault::PEER_LOST);
    assert!(c.fault == FlashFault::TRANSPORT && !c.may_actuate(1));
    assert_eq!(c.close(1), FlashReply::FAILED);
}

#[test]
fn close_requires_current_fence_and_disabled_peer() {
    let mut c = ready(0);
    let old = request(&mut c, FlashParty::LOCAL, 1, 0);
    grant(&mut c, &old, 0);
    finish(&mut c, &old, 0);
    assert_eq!(c.close(1), FlashReply::ACCEPTED);
    let fence = c.token;
    assert!(fence.owner == FlashParty::FENCE && fence.lease != old.lease);
    assert_eq!(c.closed(&old, true, true, 1), FlashReply::STALE);
    assert_eq!(c.closed(&fence, false, true, 1), FlashReply::BUSY);
    assert!(!c.may_actuate(1));
    assert_eq!(c.request(FlashParty::PEER, 29, 1, 1), FlashReply::BUSY);
    assert_eq!(c.closed(&fence, true, true, 2), FlashReply::ACCEPTED);
    assert!(!c.may_actuate(2));
    assert!(!c.may_access(&old, FlashParty::LOCAL, 2));
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
    assert_eq!(c.request(FlashParty::FENCE, 17, 1, 0), FlashReply::INVALID);
    assert_eq!(c.request(FlashParty::LOCAL, 17, 0, 0), FlashReply::INVALID);
    c.token.lease = u64::MAX;
    assert_eq!(c.request(FlashParty::LOCAL, 17, 1, 0), FlashReply::FAILED);
    assert!(c.fault == FlashFault::EXHAUSTED && !c.may_actuate(0));
}

/// Exhaust all six-event traces over five messages, for BOTH flash owners.
/// The oracle records protocol facts independently of the implementation phase.
#[test]
fn exhaustive_reordering_and_replay_cannot_create_permission() {
    for side in 0..2 {
        for trace in 0..15_625u32 {
            let mut c = ready(0);
            let who = FlashParty(side);
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
                    0 => quiet |= c.quiesced(&t, true, step) == FlashReply::ACCEPTED,
                    1 => {
                        if c.parked(&t, other(who), step) == FlashReply::ACCEPTED {
                            assert!(quiet);
                            parked = true;
                        }
                    }
                    2 => {
                        if c.complete(&t, who, step) == FlashReply::ACCEPTED {
                            assert!(quiet && parked);
                            complete = true;
                        }
                    }
                    3 => {
                        if c.released(&t, other(who), true, step) == FlashReply::ACCEPTED {
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
