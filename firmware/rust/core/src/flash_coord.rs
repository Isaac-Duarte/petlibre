//! Flash lease authority on the actuator-owning core.
//!
//! All calls, including GPIO permission checks, must share the platform's
//! serialization. No SDK/transport or physical parking is implemented here.
//! Messages are API values, not a native-struct wire format. Boot sessions must
//! be fresh, nonzero challenges.

use crate::{HALF_RANGE, interval_valid};

c_enum!(FlashParty { LOCAL = 0, PEER = 1, FENCE = 2 });
c_enum!(FlashPhase {
    BOOT = 0,
    IDLE = 1,
    QUIESCING = 2,
    PARKING = 3,
    ACTIVE = 4,
    RELEASING = 5,
    CLOSING = 6,
    CLOSED = 7,
    FAULT = 8,
});
c_enum!(FlashFault {
    OK = 0,
    CLOCK = 1,
    TIMEOUT = 2,
    OUTPUT = 3,
    ORDER = 4,
    EXHAUSTED = 5,
    TRANSPORT = 6,
    PEER_LOST = 7,
});
c_enum!(FlashReply { ACCEPTED = 0, DUPLICATE = 1, STALE = 2, BUSY = 3, INVALID = 4, FAILED = 5 });

impl FlashParty {
    /// A requesting or acknowledging core, as opposed to the closing fence.
    pub fn is_core(self) -> bool {
        self == Self::LOCAL || self == Self::PEER
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlashLimits {
    pub quiesce_ms: u32,
    pub park_ms: u32,
    pub active_ms: u32,
    pub release_ms: u32,
    pub total_ms: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlashToken {
    pub authority_session: u64,
    pub peer_session: u64,
    pub lease: u64,
    pub owner: FlashParty,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlashCoord {
    pub limits: FlashLimits,
    pub token: FlashToken,
    pub last_request: [u64; 2],
    pub observed_at: u32,
    pub phase_at: u32,
    pub requested_at: u32,
    pub phase: FlashPhase,
    pub fault: FlashFault,
    pub initialized: bool,
}

impl FlashCoord {
    /// Fresh, stopped boot/recovery only: never reinitialize a live/faulted
    /// authority to clear its interlock. Peer/transport teardown must be
    /// established first.
    pub fn new(l: &FlashLimits, session: u64, now: u32) -> Option<Self> {
        let valid = session != 0
            && interval_valid(l.quiesce_ms)
            && interval_valid(l.park_ms)
            && interval_valid(l.active_ms)
            && interval_valid(l.release_ms)
            && interval_valid(l.total_ms)
            && l.quiesce_ms <= l.total_ms
            && l.park_ms <= l.total_ms
            && l.active_ms <= l.total_ms
            && l.release_ms <= l.total_ms;
        valid.then(|| Self {
            limits: *l,
            token: FlashToken {
                authority_session: session,
                ..FlashToken::default()
            },
            observed_at: now,
            phase_at: now,
            requested_at: now,
            phase: FlashPhase::BOOT,
            initialized: true,
            ..Self::default()
        })
    }

    pub fn trip(&mut self, reason: FlashFault) {
        if !self.initialized
            || self.phase == FlashPhase::CLOSED
            || reason == FlashFault::OK
            || reason.0 > FlashFault::PEER_LOST.0
        {
            return;
        }
        if self.fault == FlashFault::OK {
            self.fault = reason;
        }
        self.phase = FlashPhase::FAULT;
    }

    fn set_phase(&mut self, next: FlashPhase, now: u32) {
        self.phase = next;
        self.phase_at = now;
    }

    pub fn poll(&mut self, now: u32) -> bool {
        if !self.initialized || self.fault != FlashFault::OK || self.phase == FlashPhase::CLOSED {
            return false;
        }
        if now.wrapping_sub(self.observed_at) >= HALF_RANGE {
            self.trip(FlashFault::CLOCK);
            return false;
        }
        self.observed_at = now;
        let limit = match self.phase {
            FlashPhase::BOOT | FlashPhase::PARKING => self.limits.park_ms,
            FlashPhase::QUIESCING => self.limits.quiesce_ms,
            FlashPhase::ACTIVE => self.limits.active_ms,
            FlashPhase::RELEASING | FlashPhase::CLOSING => self.limits.release_ms,
            FlashPhase::IDLE => return true,
            _ => {
                self.trip(FlashFault::ORDER);
                return false;
            }
        };
        if now.wrapping_sub(self.phase_at) >= limit
            || (self.phase != FlashPhase::BOOT
                && now.wrapping_sub(self.requested_at) >= self.limits.total_ms)
        {
            self.trip(FlashFault::TIMEOUT);
            return false;
        }
        true
    }

    fn message(&mut self, t: &FlashToken, now: u32) -> FlashReply {
        if !self.initialized {
            return FlashReply::INVALID;
        }
        if !self.poll(now) {
            return FlashReply::FAILED;
        }
        if t.lease == 0 || *t != self.token {
            return FlashReply::STALE;
        }
        FlashReply::ACCEPTED
    }

    fn order(&mut self) -> FlashReply {
        self.trip(FlashFault::ORDER);
        FlashReply::FAILED
    }

    fn output(&mut self) -> FlashReply {
        self.trip(FlashFault::OUTPUT);
        FlashReply::FAILED
    }

    /// Must echo the fresh authority challenge and establish the current peer
    /// boot session through a real handshake, after verifying motor and
    /// infrared off.
    pub fn ready(&mut self, session: u64, peer: u64, inactive: bool, now: u32) -> FlashReply {
        if !self.initialized || session == 0 || peer == 0 {
            return FlashReply::INVALID;
        }
        if !self.poll(now) {
            return FlashReply::FAILED;
        }
        if session != self.token.authority_session {
            return FlashReply::STALE;
        }
        if self.phase != FlashPhase::BOOT {
            if peer == self.token.peer_session {
                return FlashReply::DUPLICATE;
            }
            self.trip(FlashFault::PEER_LOST);
            return FlashReply::FAILED;
        }
        if !inactive {
            return self.output();
        }
        self.token.peer_session = peer;
        self.set_phase(FlashPhase::IDLE, now);
        FlashReply::ACCEPTED
    }

    fn next_token(&mut self) -> bool {
        if self.token.lease == u64::MAX {
            self.trip(FlashFault::EXHAUSTED);
            return false;
        }
        self.token.lease += 1;
        true
    }

    pub fn request(
        &mut self,
        requester: FlashParty,
        session: u64,
        id: u64,
        now: u32,
    ) -> FlashReply {
        if !self.initialized || !requester.is_core() || id == 0 {
            return FlashReply::INVALID;
        }
        if !self.poll(now) {
            return FlashReply::FAILED;
        }
        let expected = if requester == FlashParty::LOCAL {
            self.token.authority_session
        } else {
            self.token.peer_session
        };
        if session != expected || session == 0 {
            return FlashReply::STALE;
        }
        let last = &self.last_request[requester.0 as usize];
        if id == *last && self.token.owner == requester {
            return FlashReply::DUPLICATE;
        }
        if id <= *last {
            return FlashReply::STALE;
        }
        if self.phase != FlashPhase::IDLE {
            return FlashReply::BUSY;
        }
        if !self.next_token() {
            return FlashReply::FAILED;
        }
        self.token.owner = requester;
        self.last_request[requester.0 as usize] = id;
        self.requested_at = now;
        self.set_phase(FlashPhase::QUIESCING, now);
        FlashReply::ACCEPTED
    }

    pub fn quiesced(&mut self, t: &FlashToken, inactive: bool, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::ACCEPTED {
            return r;
        }
        if !inactive {
            return self.output();
        }
        match self.phase {
            FlashPhase::QUIESCING => {
                self.set_phase(FlashPhase::PARKING, now);
                FlashReply::ACCEPTED
            }
            FlashPhase::PARKING | FlashPhase::ACTIVE | FlashPhase::RELEASING => {
                FlashReply::DUPLICATE
            }
            _ => self.order(),
        }
    }

    /// Acknowledgement must come from the OTHER party, executing from verified
    /// RAM with no path to flash until this lease completes. A sleep bit is not ACK.
    pub fn parked(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::ACCEPTED {
            return r;
        }
        if !who.is_core() || who == self.token.owner {
            return self.order();
        }
        match self.phase {
            FlashPhase::PARKING => {
                self.set_phase(FlashPhase::ACTIVE, now);
                FlashReply::ACCEPTED
            }
            FlashPhase::ACTIVE | FlashPhase::RELEASING | FlashPhase::IDLE => FlashReply::DUPLICATE,
            _ => self.order(),
        }
    }

    pub fn complete(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::ACCEPTED {
            return r;
        }
        if who != self.token.owner || who == FlashParty::FENCE {
            return self.order();
        }
        match self.phase {
            FlashPhase::ACTIVE => {
                self.set_phase(FlashPhase::RELEASING, now);
                FlashReply::ACCEPTED
            }
            FlashPhase::RELEASING | FlashPhase::IDLE => FlashReply::DUPLICATE,
            _ => self.order(),
        }
    }

    /// Completion keeps outputs blocked until the parked party's release ACK
    /// and a new inactive-output verification. No automatic release on timeout.
    pub fn released(
        &mut self,
        t: &FlashToken,
        who: FlashParty,
        inactive: bool,
        now: u32,
    ) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::ACCEPTED {
            return r;
        }
        if !who.is_core() || who == self.token.owner {
            return self.order();
        }
        if self.phase == FlashPhase::IDLE {
            return FlashReply::DUPLICATE;
        }
        if self.phase != FlashPhase::RELEASING {
            return self.order();
        }
        if !inactive {
            return self.output();
        }
        self.set_phase(FlashPhase::IDLE, now);
        FlashReply::ACCEPTED
    }

    pub fn may_access(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> bool {
        self.poll(now)
            && self.phase == FlashPhase::ACTIVE
            && who == self.token.owner
            && who != FlashParty::FENCE
            && *t == self.token
    }

    /// GPIO enable must check this and apply its output under the SAME lock.
    pub fn may_actuate(&mut self, now: u32) -> bool {
        self.poll(now) && !self.outputs_blocked()
    }

    /// State interlock for the actual GPIO write, under authority
    /// serialization. It does not read a clock or grant/extend a lease.
    /// Non-IDLE always blocks.
    pub fn outputs_blocked(&self) -> bool {
        !self.initialized || self.fault != FlashFault::OK || self.phase != FlashPhase::IDLE
    }

    pub fn close(&mut self, now: u32) -> FlashReply {
        if !self.poll(now) {
            return FlashReply::FAILED;
        }
        if self.phase == FlashPhase::CLOSING {
            return FlashReply::DUPLICATE;
        }
        if self.phase != FlashPhase::IDLE {
            return FlashReply::BUSY;
        }
        if !self.next_token() {
            return FlashReply::FAILED;
        }
        self.token.owner = FlashParty::FENCE;
        self.requested_at = now;
        self.set_phase(FlashPhase::CLOSING, now);
        FlashReply::ACCEPTED
    }

    pub fn closed(
        &mut self,
        t: &FlashToken,
        peer_disabled: bool,
        inactive: bool,
        now: u32,
    ) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::ACCEPTED {
            return r;
        }
        if self.phase != FlashPhase::CLOSING || self.token.owner != FlashParty::FENCE {
            return self.order();
        }
        if !peer_disabled {
            return FlashReply::BUSY;
        }
        if !inactive {
            return self.output();
        }
        self.set_phase(FlashPhase::CLOSED, now);
        FlashReply::ACCEPTED
    }
}
