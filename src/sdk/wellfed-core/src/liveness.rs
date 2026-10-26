//! Whether a joined Wi-Fi link is still worth keeping.
//!
//! The feeder is a local device: its owner's clients reach it over the LAN,
//! and nothing it does needs the internet. So a link is judged only by what
//! the LAN itself shows: the association, the DHCP lease and the gateway
//! the lease named. A network with no route out stays joined; leaving it
//! would only churn rejoins, each of which refuses local feeds while it
//! runs.
//!
//! The C Wi-Fi task samples the link every watch tick and asks
//! [`Liveness::step`] what to do; the machine itself does no IO.
//!
//! - **Association** lost: leave at once ([`Cause::Link`]).
//! - **Lease** gone (lwIP cleared the address after renew and rebind both
//!   went unanswered for the whole lease): leave at once ([`Cause::Lease`]).
//! - **Gateway:** one ICMP echo every [`PROBE_PERIOD_MS`]. Once the gateway
//!   has answered this join, [`GATEWAY_MISSES`] unanswered probes in a row
//!   are a loss ([`Cause::Gateway`]): the association looks up but nothing
//!   gets through, the case the driver's beacon-loss detection misses. A
//!   gateway that has never answered this join is taken to refuse echo
//!   rather than to be gone, since DHCP has just worked over the same link;
//!   the link then rests on association and lease alone, so a router that
//!   drops pings is never left for it.
//!
//! **Settled** is reported once per join, when the gateway first answers,
//! or after [`GATEWAY_MISSES`] probes without one: the shim then resets its
//! rejoin backoff, so a network that keeps dropping the feeder right after
//! DHCP backs off rather than rejoining every second.

/// Time between gateway probes.
pub const PROBE_PERIOD_MS: u32 = 5_000;

/// Unanswered probes in a row, after the gateway has answered once, that
/// make a loss: 11–16 s of silence with a 1 s probe timeout. One lost echo on a busy channel
/// is ordinary; three in a row are not.
pub const GATEWAY_MISSES: u8 = 3;

/// [`Verdict::code`] of a leave is this plus the [`Cause::code`].
pub const LEAVE: i32 = 0x100;

/// One gateway probe's outcome, as the shim reports it with the next step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// No probe ran since the last step.
    None,
    /// The gateway answered the echo.
    Answered,
    /// No answer within the shim's timeout, or the probe could not be sent.
    Silent,
    /// The lease named no gateway, so there is nothing to probe.
    NoGateway,
}

impl Probe {
    /// The shim's code: 0 none, 1 answered, 2 silent, 3 no gateway. An
    /// unknown code carries no observation and reads as none, so a shim
    /// mismatch can never make the feeder leave a working network.
    pub const fn from_code(code: i32) -> Self {
        match code {
            1 => Self::Answered,
            2 => Self::Silent,
            3 => Self::NoGateway,
            _ => Self::None,
        }
    }
}

/// Why a link was left: the `result` of the shim's `lost` event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Cause {
    /// The association dropped.
    Link = 0,
    // 1 was the internet DNS check this machine replaced; never reused, so
    // an old log's `lost result=1` keeps its meaning.
    /// The DHCP lease ran out unrenewed.
    Lease = 2,
    /// The gateway stopped answering.
    Gateway = 3,
}

impl Cause {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Link),
            2 => Some(Self::Lease),
            3 => Some(Self::Gateway),
            _ => None,
        }
    }
}

/// What the shim does after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Keep watching: wait one watch tick and step again.
    Hold,
    /// Probe the gateway now and step again with the result.
    Probe,
    /// As [`Verdict::Hold`], and the link is now known good: reset the
    /// rejoin backoff. Once per join.
    Settled,
    /// Leave the network and rejoin after the backoff.
    Leave(Cause),
}

impl Verdict {
    /// The C return value: 0 hold, 1 probe, 2 settled, [`LEAVE`] plus the
    /// cause for a leave.
    pub const fn code(self) -> i32 {
        match self {
            Self::Hold => 0,
            Self::Probe => 1,
            Self::Settled => 2,
            Self::Leave(cause) => LEAVE + cause.code() as i32,
        }
    }

    /// The verdict a [`Verdict::code`] names, or `None` for any other value.
    pub const fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Hold,
            1 => Self::Probe,
            2 => Self::Settled,
            _ => match Cause::from_code(code.wrapping_sub(LEAVE)) {
                Some(cause) => Self::Leave(cause),
                None => return None,
            },
        })
    }
}

/// One join's liveness; a new join starts a new machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Liveness {
    /// When the last probe was asked for; `None` before the first.
    probed_at: Option<u32>,
    /// The gateway has answered at least once this join.
    answered: bool,
    /// Unanswered probes since the last answer.
    misses: u8,
    settled: bool,
    left: bool,
}

impl Liveness {
    pub const fn new() -> Self {
        Self {
            probed_at: None,
            answered: false,
            misses: 0,
            settled: false,
            left: false,
        }
    }

    /// One watch tick: the link as the shim sees it now, and the result of
    /// the probe the last step asked for, if one ran.
    pub fn step(&mut self, now: u32, associated: bool, leased: bool, probe: Probe) -> Verdict {
        let verdict = self.decide(now, associated, leased, probe);
        if matches!(verdict, Verdict::Leave(_)) {
            self.left = true;
        }
        verdict
    }

    fn decide(&mut self, now: u32, associated: bool, leased: bool, probe: Probe) -> Verdict {
        if !associated {
            return Verdict::Leave(Cause::Link);
        }
        if !leased {
            return Verdict::Leave(Cause::Lease);
        }

        match probe {
            Probe::None => {}
            Probe::Answered => {
                self.answered = true;
                self.misses = 0;
            }
            Probe::Silent => {
                self.misses = self.misses.saturating_add(1);
                if self.answered && self.misses >= GATEWAY_MISSES {
                    return Verdict::Leave(Cause::Gateway);
                }
            }
            // A lease without a router rests on association and lease
            // alone, as a gateway that never answers does.
            Probe::NoGateway if !self.answered => self.misses = self.misses.saturating_add(1),
            Probe::NoGateway => {}
        }

        if !self.settled && (self.answered || self.misses >= GATEWAY_MISSES) {
            self.settled = true;
            return Verdict::Settled;
        }

        let due = self
            .probed_at
            .is_none_or(|at| now.wrapping_sub(at) >= PROBE_PERIOD_MS);
        if due {
            self.probed_at = Some(now);
            return Verdict::Probe;
        }
        Verdict::Hold
    }

    /// The link is known good: settled, not left, and, once the gateway has
    /// answered this join, its latest probe answered too. Trial confirmation
    /// and the broker session wait for this.
    pub const fn live(&self) -> bool {
        self.settled && !self.left && (!self.answered || self.misses == 0)
    }
}
