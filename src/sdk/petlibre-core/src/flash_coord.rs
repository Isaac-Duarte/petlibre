//! Flash lease authority on the actuator-owning core.
//!
//! All calls, including GPIO permission checks, must share the platform's
//! serialization. No SDK/transport or physical parking is implemented here.
//! Messages are API values, not a native-struct wire format. Boot sessions must
//! be fresh, nonzero challenges.
//!
//! A lease runs IDLE, QUIESCING (the requester's outputs verified inactive),
//! PARKING (the other core acknowledges from RAM), ACTIVE (only the owner may
//! touch flash), RELEASING, then IDLE again. Outputs are blocked in every
//! phase but IDLE, each phase has its own deadline inside the total lease
//! budget, and a fault latches: only a fresh boot authority clears it.

use crate::{HALF_RANGE, interval_valid};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FlashParty {
    #[default]
    Local = 0,
    Peer = 1,
    Fence = 2,
}

impl FlashParty {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FlashPhase {
    #[default]
    Boot = 0,
    Idle = 1,
    Quiescing = 2,
    Parking = 3,
    Active = 4,
    Releasing = 5,
    Closing = 6,
    Closed = 7,
    Fault = 8,
}

impl FlashPhase {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FlashFault {
    #[default]
    Ok = 0,
    Clock = 1,
    Timeout = 2,
    Output = 3,
    Order = 4,
    Exhausted = 5,
    Transport = 6,
    PeerLost = 7,
}

impl FlashFault {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum FlashReply {
    #[default]
    Accepted = 0,
    Duplicate = 1,
    Stale = 2,
    Busy = 3,
    Invalid = 4,
    Failed = 5,
}

impl FlashReply {
    /// The variant's number, as records and reports print it.
    pub const fn code(self) -> u32 {
        self as u32
    }
}

impl FlashParty {
    /// A requesting or acknowledging core, as opposed to the closing fence.
    pub fn is_core(self) -> bool {
        self == Self::Local || self == Self::Peer
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashLimits {
    pub quiesce_ms: u32,
    pub park_ms: u32,
    pub active_ms: u32,
    pub release_ms: u32,
    pub total_ms: u32,
}

impl FlashLimits {
    /// No limits: refused by [`FlashCoord::new`].
    pub const UNSET: Self = Self {
        quiesce_ms: 0,
        park_ms: 0,
        active_ms: 0,
        release_ms: 0,
        total_ms: 0,
    };
}

impl Default for FlashLimits {
    fn default() -> Self {
        Self::UNSET
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashToken {
    pub authority_session: u64,
    pub peer_session: u64,
    pub lease: u64,
    pub owner: FlashParty,
}

impl FlashToken {
    /// No session and no lease.
    pub const UNSET: Self = Self {
        authority_session: 0,
        peer_session: 0,
        lease: 0,
        owner: FlashParty::Local,
    };
}

impl Default for FlashToken {
    fn default() -> Self {
        Self::UNSET
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl Default for FlashCoord {
    fn default() -> Self {
        Self::UNSET
    }
}

impl FlashCoord {
    /// Not initialized: every request is refused until [`Self::new`]
    /// replaces it. A static coordinator starts here.
    pub const UNSET: Self = Self {
        limits: FlashLimits::UNSET,
        token: FlashToken::UNSET,
        last_request: [0; 2],
        observed_at: 0,
        phase_at: 0,
        requested_at: 0,
        phase: FlashPhase::Boot,
        fault: FlashFault::Ok,
        initialized: false,
    };

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
            phase: FlashPhase::Boot,
            initialized: true,
            ..Self::default()
        })
    }

    pub fn trip(&mut self, reason: FlashFault) {
        if !self.initialized || self.phase == FlashPhase::Closed || reason == FlashFault::Ok {
            return;
        }

        if self.fault == FlashFault::Ok {
            self.fault = reason;
        }
        self.phase = FlashPhase::Fault;
    }

    fn set_phase(&mut self, next: FlashPhase, now: u32) {
        self.phase = next;
        self.phase_at = now;
    }

    pub fn poll(&mut self, now: u32) -> bool {
        if !self.initialized || self.fault != FlashFault::Ok || self.phase == FlashPhase::Closed {
            return false;
        }

        if now.wrapping_sub(self.observed_at) >= HALF_RANGE {
            self.trip(FlashFault::Clock);
            return false;
        }
        self.observed_at = now;

        let limit = match self.phase {
            FlashPhase::Boot | FlashPhase::Parking => self.limits.park_ms,
            FlashPhase::Quiescing => self.limits.quiesce_ms,
            FlashPhase::Active => self.limits.active_ms,
            FlashPhase::Releasing | FlashPhase::Closing => self.limits.release_ms,
            FlashPhase::Idle => return true,
            _ => {
                self.trip(FlashFault::Order);
                return false;
            }
        };

        if now.wrapping_sub(self.phase_at) >= limit
            || (self.phase != FlashPhase::Boot
                && now.wrapping_sub(self.requested_at) >= self.limits.total_ms)
        {
            self.trip(FlashFault::Timeout);
            return false;
        }
        true
    }

    fn message(&mut self, t: &FlashToken, now: u32) -> FlashReply {
        if !self.initialized {
            return FlashReply::Invalid;
        }
        if !self.poll(now) {
            return FlashReply::Failed;
        }
        if t.lease == 0 || *t != self.token {
            return FlashReply::Stale;
        }
        FlashReply::Accepted
    }

    fn order(&mut self) -> FlashReply {
        self.trip(FlashFault::Order);
        FlashReply::Failed
    }

    fn output(&mut self) -> FlashReply {
        self.trip(FlashFault::Output);
        FlashReply::Failed
    }

    /// Must echo the fresh authority challenge and establish the current peer
    /// boot session through a real handshake, after verifying motor and
    /// infrared off.
    pub fn ready(&mut self, session: u64, peer: u64, inactive: bool, now: u32) -> FlashReply {
        if !self.initialized || session == 0 || peer == 0 {
            return FlashReply::Invalid;
        }
        if !self.poll(now) {
            return FlashReply::Failed;
        }
        if session != self.token.authority_session {
            return FlashReply::Stale;
        }

        if self.phase != FlashPhase::Boot {
            if peer == self.token.peer_session {
                return FlashReply::Duplicate;
            }
            self.trip(FlashFault::PeerLost);
            return FlashReply::Failed;
        }

        if !inactive {
            return self.output();
        }

        self.token.peer_session = peer;
        self.set_phase(FlashPhase::Idle, now);
        FlashReply::Accepted
    }

    fn next_token(&mut self) -> bool {
        if self.token.lease == u64::MAX {
            self.trip(FlashFault::Exhausted);
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
            return FlashReply::Invalid;
        }
        if !self.poll(now) {
            return FlashReply::Failed;
        }

        let expected = if requester == FlashParty::Local {
            self.token.authority_session
        } else {
            self.token.peer_session
        };
        if session != expected || session == 0 {
            return FlashReply::Stale;
        }

        let last = &self.last_request[requester.code() as usize];
        if id == *last && self.token.owner == requester {
            return FlashReply::Duplicate;
        }
        if id <= *last {
            return FlashReply::Stale;
        }

        if self.phase != FlashPhase::Idle {
            return FlashReply::Busy;
        }
        if !self.next_token() {
            return FlashReply::Failed;
        }

        self.token.owner = requester;
        self.last_request[requester.code() as usize] = id;
        self.requested_at = now;
        self.set_phase(FlashPhase::Quiescing, now);
        FlashReply::Accepted
    }

    pub fn quiesced(&mut self, t: &FlashToken, inactive: bool, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::Accepted {
            return r;
        }
        if !inactive {
            return self.output();
        }

        match self.phase {
            FlashPhase::Quiescing => {
                self.set_phase(FlashPhase::Parking, now);
                FlashReply::Accepted
            }
            FlashPhase::Parking | FlashPhase::Active | FlashPhase::Releasing => {
                FlashReply::Duplicate
            }
            _ => self.order(),
        }
    }

    /// Acknowledgement must come from the OTHER party, executing from verified
    /// RAM with no path to flash until this lease completes. A sleep bit is not ACK.
    pub fn parked(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::Accepted {
            return r;
        }
        if !who.is_core() || who == self.token.owner {
            return self.order();
        }

        match self.phase {
            FlashPhase::Parking => {
                self.set_phase(FlashPhase::Active, now);
                FlashReply::Accepted
            }
            FlashPhase::Active | FlashPhase::Releasing | FlashPhase::Idle => FlashReply::Duplicate,
            _ => self.order(),
        }
    }

    pub fn complete(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::Accepted {
            return r;
        }
        if who != self.token.owner || who == FlashParty::Fence {
            return self.order();
        }

        match self.phase {
            FlashPhase::Active => {
                self.set_phase(FlashPhase::Releasing, now);
                FlashReply::Accepted
            }
            FlashPhase::Releasing | FlashPhase::Idle => FlashReply::Duplicate,
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
        if r != FlashReply::Accepted {
            return r;
        }

        if !who.is_core() || who == self.token.owner {
            return self.order();
        }
        if self.phase == FlashPhase::Idle {
            return FlashReply::Duplicate;
        }
        if self.phase != FlashPhase::Releasing {
            return self.order();
        }
        if !inactive {
            return self.output();
        }

        self.set_phase(FlashPhase::Idle, now);
        FlashReply::Accepted
    }

    pub fn may_access(&mut self, t: &FlashToken, who: FlashParty, now: u32) -> bool {
        self.poll(now)
            && self.phase == FlashPhase::Active
            && who == self.token.owner
            && who != FlashParty::Fence
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
        !self.initialized || self.fault != FlashFault::Ok || self.phase != FlashPhase::Idle
    }

    pub fn close(&mut self, now: u32) -> FlashReply {
        if !self.poll(now) {
            return FlashReply::Failed;
        }
        if self.phase == FlashPhase::Closing {
            return FlashReply::Duplicate;
        }
        if self.phase != FlashPhase::Idle {
            return FlashReply::Busy;
        }
        if !self.next_token() {
            return FlashReply::Failed;
        }

        self.token.owner = FlashParty::Fence;
        self.requested_at = now;
        self.set_phase(FlashPhase::Closing, now);
        FlashReply::Accepted
    }

    pub fn closed(
        &mut self,
        t: &FlashToken,
        peer_disabled: bool,
        inactive: bool,
        now: u32,
    ) -> FlashReply {
        let r = self.message(t, now);
        if r != FlashReply::Accepted {
            return r;
        }

        if self.phase != FlashPhase::Closing || self.token.owner != FlashParty::Fence {
            return self.order();
        }
        if !peer_disabled {
            return FlashReply::Busy;
        }
        if !inactive {
            return self.output();
        }

        self.set_phase(FlashPhase::Closed, now);
        FlashReply::Accepted
    }
}
