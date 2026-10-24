//! FW34 BLE setup: Wi-Fi credentials over Bluetooth LE, for a feeder that
//! cannot reach its network.
//!
//! The C peripheral (`wellfed_ble.c`) serves the service in
//! `wellfed_core::ble_setup`; this task decides when it advertises and
//! answers what a host sends. Like stock, the feeder advertises only once it
//! has been offline for `OFFLINE_MS`, and stops as soon as it is back online.
//!
//! When a host enables control notifications the feeder draws a fresh
//! `request_auth::Session` and sends a hello naming its nonce. The host then
//! sends one signed FW25 `wifi` request under that nonce, which goes to the
//! same queue the MQTT transport uses (`network::request`): the settings
//! writer saves it, and the Wi-Fi task joins with it at its next attempt.
//! Every request must be signed, reads included, since anyone in range can
//! connect; anything but `wifi` is answered `unsupported`. No record carries
//! a credential, and nothing here touches the motor, the emitters or flash.

use core::fmt;

use wellfed_core::ble_setup::{
    CONTROL, MAX_MESSAGE, OFFLINE_MS, PREFIX, SERVICE, hello, parse_prefix, prefix, uuid_le,
};
use wellfed_core::deadline_passed;
use wellfed_core::named_enum;
use wellfed_core::request_auth::{self, Session};
use wellfed_core::transport::{self as mqtt, Buffer, Op};

use crate::net_status;
use crate::network;
use crate::rng;
use crate::sdk::{self, Advertising, BleEvent, BlePoll, BleService};
pub use crate::transport::RequestKind;

/// The C task waits up to 60 s for the radio; allow for the stack after.
pub const READY_MS: u32 = 90_000;
/// A host that has not finished in this long is dropped, so one idle
/// connection cannot hold the only link.
pub const SESSION_MS: u32 = 60_000;
const CONNECTED_MS: u32 = 20;
const IDLE_MS: u32 = 500;
const FAILED_MS: u32 = 60_000;

/// What the peripheral advertises. The name is the scan response's.
pub const SETUP: BleService = BleService {
    name: b"Wellfed",
    service: uuid_le(SERVICE),
    control: uuid_le(CONTROL),
};

named_enum! {
    /// What failed to start the peripheral (`failed stage=`).
    pub enum FailStage {
        Task = "task",
        Radio = "radio",
        Bte = "bte",
        Service = "service",
        Stack = "stack",
        Timeout = "timeout",
    }
}

named_enum! {
    /// Why a link was dropped (`drop reason=`).
    pub enum DropReason {
        /// The image has no request key, so no request could be checked.
        NoKey = "no_key",
        Notify = "notify",
        TooLong = "too_long",
        Malformed = "malformed",
        Timeout = "timeout",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Record {
    Ready {
        heap_free: u32,
    },
    Failed {
        stage: FailStage,
    },
    Advertising {
        on: bool,
    },
    Connect {
        interval: u32,
    },
    Disconnect {
        cause: i32,
    },
    Mtu {
        mtu: u32,
    },
    /// The event ring overwrote events before this task read them.
    Lost,
    /// One answered request; `ok` as the reply says.
    Request {
        kind: RequestKind,
        ok: bool,
    },
    Drop {
        reason: DropReason,
    },
}

/// The body of a `PLBLE1` line.
pub fn describe(record: Record, f: &mut impl fmt::Write) -> fmt::Result {
    match record {
        Record::Ready { heap_free } => write!(f, "ready heap_free={heap_free}"),
        Record::Failed { stage } => write!(f, "failed stage={stage}"),
        Record::Advertising { on } => write!(f, "advertising on={}", u8::from(on)),
        Record::Connect { interval } => write!(f, "connect interval={interval}"),
        Record::Disconnect { cause } => write!(f, "disconnect cause=0x{cause:x}"),
        Record::Mtu { mtu } => write!(f, "mtu value={mtu}"),
        Record::Lost => write!(f, "events lost=1"),
        Record::Request { kind, ok } => {
            write!(f, "request op={} ok={}", kind_name(kind), u8::from(ok))
        }
        Record::Drop { reason } => write!(f, "drop reason={reason}"),
    }
}

fn kind_name(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Health => "health",
        RequestKind::Ping => "ping",
        RequestKind::Time => "time",
        RequestKind::Settings => "settings",
        RequestKind::Wifi => "wifi",
        RequestKind::Play => "play",
        RequestKind::Lock => "lock",
        RequestKind::Unauthenticated => "unauthenticated",
        RequestKind::Unsupported => "unsupported",
        RequestKind::Malformed => "malformed",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step { delay_ms, record }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Start,
    /// Until the stack reports ready, or `deadline`.
    Starting {
        deadline: u32,
    },
    Running,
    Failed,
}

/// The connected host's session: its verifier and the message being read.
#[derive(Debug)]
struct Link {
    since: u32,
    session: Option<Session>,
    message: [u8; PREFIX + MAX_MESSAGE],
    received: usize,
}

#[derive(Debug)]
pub struct BleSetup {
    stage: Stage,
    /// The next event to read.
    seq: u32,
    link: Option<Link>,
    /// When the feeder was last online (or the task started).
    online_at: u32,
    advertising: Advertising,
}

impl Default for BleSetup {
    fn default() -> Self {
        Self {
            stage: Stage::Start,
            seq: 1,
            link: None,
            online_at: 0,
            advertising: Advertising::Off,
        }
    }
}

/// Send `body` as one message: its prefix, then the bytes.
fn send(body: &[u8]) -> bool {
    let Some(prefix) = prefix(body.len()) else {
        return false;
    };
    let mut framed = [0u8; PREFIX + MAX_MESSAGE];
    framed[..PREFIX].copy_from_slice(&prefix);
    framed[PREFIX..PREFIX + body.len()].copy_from_slice(body);
    sdk::ble_notify(&framed[..PREFIX + body.len()])
}

fn drop_link(reason: DropReason) -> Option<Record> {
    sdk::ble_disconnect();
    Some(Record::Drop { reason })
}

/// Answer one signed request with what the MQTT transport would reply.
fn answer(session: &mut Session, message: &[u8]) -> Option<Record> {
    let Some(signed) = request_auth::split(message) else {
        return drop_link(DropReason::Malformed);
    };
    let mut unsigned = [0u8; MAX_MESSAGE];
    let len = signed.prefix.len();
    unsigned[..len].copy_from_slice(signed.prefix);
    unsigned[len] = b'}';
    let Ok(request) = mqtt::parse_request(&unsigned[..=len]) else {
        return drop_link(DropReason::Malformed);
    };

    let authentic = request
        .auth
        .is_some_and(|(nonce, seq)| session.verify(&signed, nonce, seq));
    let mut body = [0u8; MAX_MESSAGE];
    let mut writer = Buffer::new(&mut body);
    let (kind, ok, written) = match (authentic, request.op, request.wifi) {
        (false, _, _) => (
            RequestKind::Unauthenticated,
            false,
            mqtt::write_auth_refusal(&mut writer, &request),
        ),
        (true, Ok(Op::Wifi), Some(fields)) => {
            let result = network::request(fields);
            (
                RequestKind::Wifi,
                matches!(result, mqtt::WifiResult::Current { .. }),
                mqtt::write_wifi_reply(&mut writer, &request, result),
            )
        }
        // Only `wifi` is served over BLE.
        (true, _, _) => {
            let other = mqtt::Request {
                op: Err("ble"),
                ..request
            };
            (
                RequestKind::Unsupported,
                false,
                mqtt::write_reply(&mut writer, &other, sdk::uptime_ms()),
            )
        }
    };
    if written.is_err() {
        return drop_link(DropReason::TooLong);
    }

    let len = writer.len();
    if !send(&body[..len]) {
        return drop_link(DropReason::Notify);
    }
    Some(Record::Request { kind, ok })
}

impl BleSetup {
    /// One event's effect, and its record.
    fn on_event(&mut self, event: BleEvent, now: u32) -> Option<Record> {
        match event.kind {
            BleEvent::READY => {
                let Stage::Starting { .. } = self.stage else {
                    return None;
                };
                let stage = match event.result {
                    0 => {
                        self.stage = Stage::Running;
                        self.online_at = now;
                        return Some(Record::Ready {
                            heap_free: sdk::ble_stats().heap_free,
                        });
                    }
                    BleEvent::READY_WIFI => FailStage::Radio,
                    BleEvent::READY_BTE => FailStage::Bte,
                    BleEvent::READY_SERVICE => FailStage::Service,
                    _ => FailStage::Stack,
                };
                self.stage = Stage::Failed;
                Some(Record::Failed { stage })
            }
            BleEvent::ADVERTISING => Some(Record::Advertising {
                on: event.value != 0,
            }),
            BleEvent::CONNECT => {
                self.link = Some(Link {
                    since: now,
                    session: None,
                    message: [0; PREFIX + MAX_MESSAGE],
                    received: 0,
                });
                Some(Record::Connect {
                    interval: event.value,
                })
            }
            BleEvent::DISCONNECT => {
                self.link = None;
                Some(Record::Disconnect {
                    cause: event.result,
                })
            }
            BleEvent::MTU => Some(Record::Mtu { mtu: event.value }),
            BleEvent::NOTIFY if event.value != 0 => {
                let link = self.link.as_mut()?;
                let Some(key) = sdk::transport_config().and_then(|config| config.request_key)
                else {
                    return drop_link(DropReason::NoKey);
                };

                // The generator (FW45), or the timer mix if it never started.
                let mut entropy = [0u8; 52];
                for (chunk, word) in entropy.chunks_mut(4).zip(sdk::entropy()) {
                    chunk.copy_from_slice(&word.to_le_bytes());
                }
                entropy[16..20].copy_from_slice(&now.to_le_bytes());
                rng::fill(&mut entropy[20..]);
                let session = Session::new(key, &entropy);
                link.received = 0;
                let hello = hello(&session.nonce_hex());
                link.session = Some(session);
                if !send(&hello) {
                    return drop_link(DropReason::Notify);
                }
                None
            }
            _ => None,
        }
    }

    /// Advertise only after `OFFLINE_MS` offline; stop once back online.
    fn follow_network(&mut self, now: u32) {
        if net_status::read().dns_ok {
            self.online_at = now;
        }
        let wanted = if now.wrapping_sub(self.online_at) >= OFFLINE_MS {
            Advertising::On
        } else {
            Advertising::Off
        };
        if wanted != self.advertising && sdk::ble_advertise(wanted) {
            self.advertising = wanted;
        }
    }

    /// Read the host's bytes; answer each complete message.
    fn serve(&mut self, now: u32) -> Option<Record> {
        let link = self.link.as_mut()?;
        if now.wrapping_sub(link.since) >= SESSION_MS {
            self.link = None;
            return drop_link(DropReason::Timeout);
        }
        let session = link.session.as_mut()?;

        let received = link.received;
        link.received += sdk::ble_control_read(&mut link.message[received..]);
        if link.received < PREFIX {
            return None;
        }
        let Some(len) = parse_prefix([link.message[0], link.message[1]]) else {
            self.link = None;
            return drop_link(DropReason::TooLong);
        };
        if link.received < PREFIX + len {
            return None;
        }

        // One request per hello: a fresh nonce for every exchange.
        let record = answer(session, &link.message[PREFIX..PREFIX + len]);
        link.received = 0;
        link.session = None;
        record
    }

    pub fn step(&mut self) -> Step {
        let now = sdk::uptime_ms();
        match sdk::ble_event(self.seq) {
            BlePoll::Event(event) => {
                self.seq = self.seq.wrapping_add(1);
                if let Some(record) = self.on_event(event, now) {
                    return next(1, Some(record));
                }
            }
            BlePoll::Lost => {
                // Skip to the oldest event the ring still holds.
                while let BlePoll::Lost = sdk::ble_event(self.seq) {
                    self.seq = self.seq.wrapping_add(1);
                }
                return next(1, Some(Record::Lost));
            }
            BlePoll::Pending => {}
        }

        match self.stage {
            Stage::Start => {
                if !sdk::ble_start(&SETUP) {
                    self.stage = Stage::Failed;
                    return next(
                        1,
                        Some(Record::Failed {
                            stage: FailStage::Task,
                        }),
                    );
                }
                self.stage = Stage::Starting {
                    deadline: now.wrapping_add(READY_MS),
                };
                next(IDLE_MS, None)
            }
            Stage::Starting { deadline } => {
                if deadline_passed(now, deadline) {
                    self.stage = Stage::Failed;
                    return next(
                        1,
                        Some(Record::Failed {
                            stage: FailStage::Timeout,
                        }),
                    );
                }
                next(IDLE_MS, None)
            }
            Stage::Running => {
                self.follow_network(now);
                let record = self.serve(now);
                let delay = if self.link.is_some() {
                    CONNECTED_MS
                } else {
                    IDLE_MS
                };
                next(if record.is_some() { 1 } else { delay }, record)
            }
            Stage::Failed => next(FAILED_MS, None),
        }
    }
}
