//! Time in the loop: the time settings applied to the clock, and the NTP
//! exchange ([`wellfed_core::clock::ntp`]) on one UDP socket.
//!
//! Each pass reads the time settings the writer published; a new
//! generation sets the clock's zone rule and the server to ask. The server
//! is resolved against this boot's lease (the gateway, or a set address)
//! and asked again at once whenever it changes. Nothing is sent before DHCP
//! gave an address, and only datagrams from the server's address and port
//! are read as answers. Every call is non-blocking, like the device API's
//! sockets.
//!
//! A fix goes to the wall clock, which decides what it may move: with a
//! valid clock an NTP fix only narrows it. A first fix and a jump are
//! recorded in the history (`ClockSync` by the feeder); a fix in step is
//! not, so polling every 17 minutes does not wear the event ring.

use core::fmt;

use wellfed_core::clock::ntp::{Bad, Exchange, Outcome, PACKET};
use wellfed_core::clock::wall::{Jump, SyncError};
use wellfed_device_api::mailbox::{NtpResult, NtpServer, TimeSettingsResult};
use wellfed_proto::api::{EventBody, Source};

use super::events;
use super::platform::{Datagram, Platform};

/// A socket that failed is opened again after this.
const RETRY_MS: u32 = 2_000;
/// Datagrams read per pass.
const DATAGRAMS: usize = 2;

/// What the exchange did, for `PLAPI1 ntp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    /// The settings' server, resolved against the lease (`None`: off, or
    /// no gateway yet).
    Server {
        ip: Option<u32>,
        port: u16,
    },
    Socket {
        open: bool,
    },
    /// An answer's fix, and what the clock made of it.
    Answer {
        result: NtpResult,
        error_ms: u32,
    },
    Refused(Bad),
    Lost,
}

pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Server { ip: Some(ip), port } => {
            let [a, b, c, d] = ip.to_be_bytes();
            write!(f, "ntp server={a}.{b}.{c}.{d}:{port}")
        }
        Record::Server { ip: None, .. } => f.write_str("ntp server=none"),
        Record::Socket { open } => write!(f, "ntp socket open={}", u8::from(open)),
        Record::Answer { result, error_ms } => {
            write!(f, "ntp answer result={} error_ms={error_ms}", name(result))
        }
        Record::Refused(bad) => write!(f, "ntp refused reason={}", bad_name(bad)),
        Record::Lost => f.write_str("ntp lost"),
    }
}

const fn name(result: NtpResult) -> &'static str {
    match result {
        NtpResult::Used => "used",
        NtpResult::Bounded => "bounded",
        NtpResult::Lost => "lost",
        NtpResult::Refused => "refused",
    }
}

const fn bad_name(bad: Bad) -> &'static str {
    match bad {
        Bad::Header => "header",
        Bad::Origin => "origin",
        Bad::Unsynchronized => "unsynchronized",
        Bad::Kiss(_) => "kiss",
        Bad::Time => "time",
        Bad::Loose => "loose",
    }
}

pub struct Time<S> {
    /// The settings generation applied to the clock and the exchange.
    applied: Option<u32>,
    configured: NtpServer,
    /// The server asked, resolved: address and port.
    target: Option<(u32, u16)>,
    socket: Option<S>,
    retry_at: u32,
    exchange: Exchange,
    last: Option<NtpResult>,
    packet: [u8; PACKET],
}

impl<S: Copy> Time<S> {
    pub const fn new() -> Self {
        Self {
            applied: None,
            configured: NtpServer::Off,
            target: None,
            socket: None,
            retry_at: 0,
            exchange: Exchange::new(),
            last: None,
            packet: [0; PACKET],
        }
    }

    /// What the last exchange came to, for the time view.
    pub const fn last(&self) -> Option<NtpResult> {
        self.last
    }

    /// One pass, at `now_ms` on the clock's own count
    /// ([`Platform::clock_ms`]): an answer's error is half the round trip
    /// measured on it, which a count that stopped during the exchange
    /// would understate.
    pub fn step<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        ip: Option<u32>,
        mut record: impl FnMut(Record),
    ) {
        self.apply(platform, now_ms);
        self.resolve(platform, now_ms, ip, &mut record);
        let Some((server_ip, server_port)) = self.target else {
            return;
        };

        if self.socket.is_none() {
            if !wellfed_core::deadline_passed(now_ms, self.retry_at) {
                return;
            }
            self.socket = platform.ntp_open();
            record(Record::Socket {
                open: self.socket.is_some(),
            });
            if self.socket.is_none() {
                self.retry_at = now_ms.wrapping_add(RETRY_MS);
                return;
            }
        }
        let Some(socket) = self.socket else {
            return;
        };

        for _ in 0..DATAGRAMS {
            match platform.ntp_receive(socket, &mut self.packet) {
                Datagram::None => break,
                Datagram::Error => {
                    self.close(platform, now_ms);
                    return;
                }
                Datagram::Got { len, ip, port } => {
                    if ip != server_ip || port != server_port {
                        continue;
                    }
                    let answer = self.exchange.received(now_ms, &self.packet[..len]);
                    self.outcome(platform, now_ms, answer, &mut record);
                }
            }
        }
        let lost = self.exchange.expire(now_ms);
        self.outcome(platform, now_ms, lost, &mut record);
        // Never past a kiss-o'-death's hold: `soon` keeps it.
        if platform.clock_state(now_ms).reading.is_none() {
            self.exchange.soon(now_ms);
        }

        // A nonce costs a generator draw: only when a request is due.
        if !self.exchange.due(now_ms) {
            return;
        }
        let mut random = [0; 32];
        if !platform.random(&mut random) {
            return;
        }
        let mut nonce = [0; 8];
        nonce.copy_from_slice(&random[..8]);
        if let Some(request) = self.exchange.poll(now_ms, u64::from_le_bytes(nonce))
            && !platform.ntp_send(socket, &request, server_ip, server_port)
        {
            self.exchange.unsent(now_ms);
        }
    }

    /// A new settings generation: the zone to the clock, and the server.
    fn apply<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32) {
        let TimeSettingsResult::Current { view, .. } = platform.time_settings() else {
            return;
        };
        if self.applied == Some(view.generation) {
            return;
        }
        self.applied = Some(view.generation);
        platform.set_zone_rule(view.settings.zone.map(|zone| zone.rule()));
        if self.configured != view.settings.ntp {
            self.configured = view.settings.ntp;
            self.exchange.restart(now_ms);
        }
    }

    fn resolve<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        ip: Option<u32>,
        record: &mut impl FnMut(Record),
    ) {
        let target = match (ip, self.configured) {
            (None, _) | (_, NtpServer::Off) => None,
            (Some(_), NtpServer::Gateway { port }) => {
                platform.gateway().map(|gateway| (gateway, port))
            }
            (Some(_), NtpServer::Address { ip, port }) => Some((u32::from_be_bytes(ip), port)),
        };
        if target == self.target {
            return;
        }
        self.target = target;
        self.exchange.restart(now_ms);
        record(Record::Server {
            ip: target.map(|(ip, _)| ip),
            port: target.map_or(0, |(_, port)| port),
        });
        if target.is_none() {
            self.close(platform, now_ms);
        }
    }

    fn outcome<P: Platform<Socket = S>>(
        &mut self,
        platform: &mut P,
        now_ms: u32,
        outcome: Option<Outcome>,
        record: &mut impl FnMut(Record),
    ) {
        match outcome {
            None => {}
            Some(Outcome::Fix(fix)) => {
                let result = match platform.time_fix(now_ms, fix) {
                    Ok(jump) => {
                        let jump_ms = match jump {
                            Jump::First => Some(None),
                            Jump::Jumped(delta) => Some(Some(saturate(delta))),
                            Jump::Step(_) => None,
                        };
                        if let Some(jump_ms) = jump_ms {
                            let body = EventBody::ClockSync {
                                by: Source::Feeder,
                                jump_ms,
                            };
                            events::record(platform, now_ms, body);
                        }
                        NtpResult::Used
                    }
                    Err(SyncError::Bounded) => NtpResult::Bounded,
                    Err(SyncError::Time | SyncError::Offset | SyncError::NextChange) => {
                        NtpResult::Refused
                    }
                };
                self.last = Some(result);
                record(Record::Answer {
                    result,
                    error_ms: fix.error_ms,
                });
            }
            // A stale or forged datagram leaves the exchange waiting.
            Some(Outcome::Refused(Bad::Origin)) => record(Record::Refused(Bad::Origin)),
            Some(Outcome::Refused(bad)) => {
                self.last = Some(NtpResult::Refused);
                record(Record::Refused(bad));
            }
            Some(Outcome::Lost) => {
                self.last = Some(NtpResult::Lost);
                record(Record::Lost);
            }
        }
    }

    fn close<P: Platform<Socket = S>>(&mut self, platform: &mut P, now_ms: u32) {
        if let Some(socket) = self.socket.take() {
            platform.close(socket);
        }
        self.retry_at = now_ms.wrapping_add(RETRY_MS);
    }
}

impl<S: Copy> Default for Time<S> {
    fn default() -> Self {
        Self::new()
    }
}

fn saturate(jump_ms: i64) -> i32 {
    i32::try_from(jump_ms).unwrap_or(if jump_ms < 0 { i32::MIN } else { i32::MAX })
}
