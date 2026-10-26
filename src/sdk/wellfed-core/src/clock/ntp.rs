//! SNTP (RFC 4330, version 4 packets): the client exchange that feeds the
//! wall clock, sans-IO, and the one-line answer a test responder gives.
//!
//! **What a request says.** Nothing about the feeder's clock: every field
//! is zero except the mode and the transmit timestamp, which carries a
//! random nonce (RFC 9109's data minimization). The answer must echo it as
//! its origin timestamp, so an off-path sender cannot inject a time without
//! seeing the request.
//!
//! **What an answer gives.** The server's transmit time, carried to the
//! moment it arrived by half the round trip less the server's own
//! processing, with an error of that half plus the server's root delay and
//! dispersion. The wall clock decides what the fix may move
//! ([`super::wall`]): with a valid clock an NTP fix only narrows it.
//!
//! **Pacing.** One request at a time, answered within [`ANSWER_MS`].
//! A lost or refused answer retries at [`RETRY_MIN_MS`], backing off to
//! [`RETRY_MAX_MS`]; after a good one the next goes [`POLL_MS`] later,
//! unless the clock needs the time before then ([`Exchange::soon`]). A
//! kiss-o'-death `RATE` waits [`RATE_MS`], and `DENY` or `RSTR` stops asking
//! that server for [`DENIED_MS`].

use super::wall::{Fix, MAX_UNIX_MS, MIN_UNIX_MS, Origin};

/// The standard NTP port; a server setting may name another.
pub const PORT: u16 = 123;
pub const PACKET: usize = 48;
/// An answer later than this is lost.
pub const ANSWER_MS: u32 = 2_000;
pub const RETRY_MIN_MS: u32 = 8_000;
pub const RETRY_MAX_MS: u32 = 64_000;
/// Between successful exchanges.
pub const POLL_MS: u32 = 1_024_000;
pub const RATE_MS: u32 = 3_600_000;
pub const DENIED_MS: u32 = 86_400_000;
/// An answer whose error would exceed this is not used: a fix that loose
/// would only widen the clock's interval.
pub const ERROR_LIMIT_MS: u32 = 10_000;

/// Seconds from 1900 (NTP's era 0) to 1970.
const UNIX_FROM_NTP: u64 = 2_208_988_800;
/// NTP seconds below this belong to era 1 (from 2036): the clock accepts
/// nothing before 2026, which is this many seconds into era 0.
const ERA_PIVOT: u64 = MIN_UNIX_MS / 1000 + UNIX_FROM_NTP;
const ERA: u64 = 1 << 32;

const VERSION: u8 = 4;
const MODE_CLIENT: u8 = 3;
const MODE_SERVER: u8 = 4;
const LEAP_UNSYNCHRONIZED: u8 = 3;

/// Why an answer was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bad {
    /// Not a server's answer, or a version this client does not speak.
    Header,
    /// Too short to hold an origin timestamp, or one that is not the
    /// request's nonce: stale or forged.
    Origin,
    /// The server says it is not synchronized (leap 3, or stratum 16+).
    Unsynchronized,
    /// Kiss-o'-death (stratum 0), with its code.
    Kiss([u8; 4]),
    /// A time outside what the wall clock accepts.
    Time,
    /// The round trip and the server's dispersion add up past
    /// [`ERROR_LIMIT_MS`].
    Loose,
}

/// The request for `nonce`.
pub fn request(nonce: u64) -> [u8; PACKET] {
    let mut packet = [0; PACKET];
    packet[0] = (VERSION << 3) | MODE_CLIENT;
    packet[40..48].copy_from_slice(&nonce.to_be_bytes());
    packet
}

/// An NTP timestamp as Unix milliseconds.
fn unix_ms(timestamp: u64) -> Option<u64> {
    let seconds = timestamp >> 32;
    let seconds = if seconds < ERA_PIVOT {
        seconds + ERA
    } else {
        seconds
    };
    let fraction_ms = ((timestamp & 0xffff_ffff) * 1000) >> 32;
    let unix = (seconds.checked_sub(UNIX_FROM_NTP)? * 1000).checked_add(fraction_ms)?;
    (MIN_UNIX_MS..MAX_UNIX_MS).contains(&unix).then_some(unix)
}

/// Unix milliseconds as an NTP timestamp.
fn ntp_timestamp(unix_ms: u64) -> u64 {
    let seconds = (unix_ms / 1000 + UNIX_FROM_NTP) % ERA;
    // Rounded up, so reading it back to the millisecond gives the same.
    let fraction = ((unix_ms % 1000) << 32).div_ceil(1000);
    (seconds << 32) | fraction
}

fn be64(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_be_bytes(word)
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    u32::from_be_bytes(word)
}

/// A 16.16 duration in whole milliseconds, rounded up.
fn short_ms(value: u32) -> u32 {
    let ms = (u64::from(value) * 1000).div_ceil(1 << 16);
    u32::try_from(ms).unwrap_or(u32::MAX)
}

/// An answer to the request for `nonce`, sent at uptime `sent` and received
/// at uptime `now`, as a fix for the wall clock at `now`.
pub fn answer(bytes: &[u8], nonce: u64, sent: u32, now: u32) -> Result<Fix, Bad> {
    // Anything that does not echo the nonce is not this request's answer:
    // checked first, so a forged datagram of any shape only leaves the
    // exchange waiting for the real one.
    if bytes.len() < PACKET || be64(bytes, 24) != nonce {
        return Err(Bad::Origin);
    }
    let leap = bytes[0] >> 6;
    let version = (bytes[0] >> 3) & 7;
    let mode = bytes[0] & 7;
    if mode != MODE_SERVER || !(3..=4).contains(&version) {
        return Err(Bad::Header);
    }
    let stratum = bytes[1];
    if stratum == 0 {
        let mut code = [0; 4];
        code.copy_from_slice(&bytes[12..16]);
        return Err(Bad::Kiss(code));
    }
    if leap == LEAP_UNSYNCHRONIZED || stratum >= 16 {
        return Err(Bad::Unsynchronized);
    }

    let received = unix_ms(be64(bytes, 32)).ok_or(Bad::Time)?;
    let transmitted = unix_ms(be64(bytes, 40)).ok_or(Bad::Time)?;
    let round_trip = u64::from(now.wrapping_sub(sent));
    let delay = round_trip.saturating_sub(transmitted.saturating_sub(received));
    let half = delay.div_ceil(2);

    // Each uptime reading may be up to 1 ms late, and the server's
    // fractions are truncated to the millisecond.
    let error = half
        + u64::from(short_ms(be32(bytes, 4)).div_ceil(2))
        + u64::from(short_ms(be32(bytes, 8)))
        + 3;
    let error_ms = u32::try_from(error).map_err(|_| Bad::Loose)?;
    if error_ms > ERROR_LIMIT_MS {
        return Err(Bad::Loose);
    }
    Ok(Fix {
        unix_ms: transmitted + delay / 2,
        error_ms,
        origin: Origin::Ntp,
    })
}

/// A server's answer to `request`, received at `received_ms` and sent at
/// `transmit_ms` (Unix), as stratum `stratum` with `dispersion_ms`; `None`
/// for anything but a client's request. For test responders: the bench's
/// and the simulator's.
pub fn serve(
    request: &[u8],
    received_ms: u64,
    transmit_ms: u64,
    stratum: u8,
    dispersion_ms: u32,
) -> Option<[u8; PACKET]> {
    if request.len() < PACKET || request[0] & 7 != MODE_CLIENT {
        return None;
    }
    let version = ((request[0] >> 3) & 7).clamp(3, 4);

    let mut packet = [0; PACKET];
    packet[0] = (version << 3) | MODE_SERVER;
    packet[1] = stratum;
    packet[2] = request[2];
    // Precision: 2^-10 s, a millisecond.
    packet[3] = 0xf6;
    let dispersion = (u64::from(dispersion_ms) << 16) / 1000;
    packet[8..12].copy_from_slice(&u32::try_from(dispersion).unwrap_or(u32::MAX).to_be_bytes());
    packet[12..16].copy_from_slice(b"LOCL");
    packet[16..24].copy_from_slice(&ntp_timestamp(transmit_ms).to_be_bytes());
    packet[24..32].copy_from_slice(&request[40..48]);
    packet[32..40].copy_from_slice(&ntp_timestamp(received_ms).to_be_bytes());
    packet[40..48].copy_from_slice(&ntp_timestamp(transmit_ms).to_be_bytes());
    Some(packet)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The next request goes at `next`.
    Idle { next: u32 },
    /// The server sent a kiss-o'-death: nothing goes before `until`,
    /// however much the clock needs the time (RFC 4330).
    Held { until: u32 },
    /// A request with `nonce` went at `sent`.
    Waiting { nonce: u64, sent: u32 },
}

/// What one exchange step came to, for the caller's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A fix, measured at the step's uptime.
    Fix(Fix),
    /// An answer that was not used; the exchange waits again.
    Refused(Bad),
    /// No answer within [`ANSWER_MS`].
    Lost,
}

/// One server's exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exchange {
    phase: Phase,
    retry_ms: u32,
}

impl Default for Exchange {
    fn default() -> Self {
        Self::new()
    }
}

impl Exchange {
    /// Due at once.
    pub const fn new() -> Self {
        Self {
            phase: Phase::Idle { next: 0 },
            retry_ms: RETRY_MIN_MS,
        }
    }

    /// Ask again at once (a new server, or a new address).
    pub fn restart(&mut self, now: u32) {
        self.phase = Phase::Idle { next: now };
        self.retry_ms = RETRY_MIN_MS;
    }

    /// The clock needs the time: ask within [`RETRY_MAX_MS`] rather than
    /// wait out a poll interval. Lost answers still back off, and a
    /// kiss-o'-death's hold is never shortened.
    pub fn soon(&mut self, now: u32) {
        if let Phase::Idle { next } = self.phase
            && next.wrapping_sub(now) > RETRY_MAX_MS
            && !crate::deadline_passed(now, next)
        {
            self.phase = Phase::Idle { next: now };
        }
    }

    /// Whether a request is due now, so a caller draws a nonce only then.
    pub fn due(&self, now: u32) -> bool {
        match self.phase {
            Phase::Idle { next } | Phase::Held { until: next } => crate::deadline_passed(now, next),
            Phase::Waiting { .. } => false,
        }
    }

    /// The request to send now, if one is due, with a fresh random
    /// `nonce`. The caller sends it once.
    pub fn poll(&mut self, now: u32, nonce: u64) -> Option<[u8; PACKET]> {
        if !self.due(now) {
            return None;
        }
        self.phase = Phase::Waiting { nonce, sent: now };
        Some(request(nonce))
    }

    /// The request could not be sent: try again after the retry interval.
    pub fn unsent(&mut self, now: u32) {
        self.fail(now);
    }

    /// A datagram from the server, at uptime `now`. `None` when no request
    /// is waiting for one.
    pub fn received(&mut self, now: u32, bytes: &[u8]) -> Option<Outcome> {
        let Phase::Waiting { nonce, sent } = self.phase else {
            return None;
        };
        let outcome = match answer(bytes, nonce, sent, now) {
            Ok(fix) => {
                self.retry_ms = RETRY_MIN_MS;
                self.phase = Phase::Idle {
                    next: now.wrapping_add(POLL_MS),
                };
                Outcome::Fix(fix)
            }
            // A stale or forged datagram: keep waiting for the real one.
            Err(bad @ Bad::Origin) => return Some(Outcome::Refused(bad)),
            Err(bad @ Bad::Kiss(code)) => {
                let wait = match &code {
                    b"DENY" | b"RSTR" => DENIED_MS,
                    _ => RATE_MS,
                };
                self.phase = Phase::Held {
                    until: now.wrapping_add(wait),
                };
                Outcome::Refused(bad)
            }
            Err(bad) => {
                self.fail(now);
                Outcome::Refused(bad)
            }
        };
        Some(outcome)
    }

    /// The step's check for a lost answer.
    pub fn expire(&mut self, now: u32) -> Option<Outcome> {
        let Phase::Waiting { sent, .. } = self.phase else {
            return None;
        };
        if now.wrapping_sub(sent) < ANSWER_MS {
            return None;
        }
        self.fail(now);
        Some(Outcome::Lost)
    }

    fn fail(&mut self, now: u32) {
        self.phase = Phase::Idle {
            next: now.wrapping_add(self.retry_ms),
        };
        self.retry_ms = self.retry_ms.saturating_mul(2).min(RETRY_MAX_MS);
    }
}
