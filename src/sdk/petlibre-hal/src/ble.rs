//! FW45 BLE setup mode's link: framed messages over Bluetooth LE.
//!
//! The C peripheral (`petlibre_ble.c`) serves the service in
//! `petlibre_core::ble_setup`. This task starts it, advertises while the
//! firmware asks, and carries length-prefixed messages each way on the
//! control characteristic: a host's write becomes [`BleSetup::message`], and
//! [`BleSetup::send`] notifies one back. What the messages say (a Noise
//! handshake, then sealed setup replies) is the firmware's, so nothing here
//! reads a credential or a key. One host at a time; a link that has not
//! finished within [`SESSION_MS`] is dropped so it cannot hold the radio.
//!
//! The firmware starts this only in setup mode. A normal boot never calls
//! `ble_start`, so the stack's heap and C tasks cost nothing outside setup.

use core::fmt;

use petlibre_core::ble_setup::{
    CONTROL, MAX_MESSAGE, PREFIX, SERVICE, parse_prefix, prefix, uuid_le,
};
use petlibre_core::deadline_passed;
use petlibre_core::named_enum;

use crate::sdk::{self, Advertising, BleEvent, BlePoll, BleService};

/// The C task waits up to 60 s for the radio; allow for the stack after.
pub const READY_MS: u32 = 90_000;
/// A host that has not finished in this long is dropped, so one idle
/// connection cannot hold the only link. Setup waits up to a minute for the
/// owner's Dispense press after the handshake.
pub const SESSION_MS: u32 = 150_000;
const CONNECTED_MS: u32 = 20;
const IDLE_MS: u32 = 500;
const FAILED_MS: u32 = 60_000;

/// What the peripheral advertises. The name is the scan response's.
pub const SETUP: BleService = BleService {
    name: b"PetLibre",
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
        Notify = "notify",
        TooLong = "too_long",
        Timeout = "timeout",
        /// The firmware refused what the host sent.
        Protocol = "protocol",
        /// The exchange is over.
        Done = "done",
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
        Record::Drop { reason } => write!(f, "drop reason={reason}"),
    }
}

/// What the firmware must act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// A host connected and enabled notifications: a new session.
    Opened,
    /// A whole message is waiting in [`BleSetup::message`].
    Message,
    /// The host left or was dropped: forget its session.
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub delay_ms: u32,
    pub record: Option<Record>,
    pub link: Option<Link>,
}

fn next(delay_ms: u32, record: Option<Record>) -> Step {
    Step {
        delay_ms,
        record,
        link: None,
    }
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

/// The connected host: when it came, and the message being read.
#[derive(Debug)]
struct Connection {
    since: u32,
    /// Notifications are on, so replies can reach the host.
    open: bool,
    message: [u8; PREFIX + MAX_MESSAGE],
    received: usize,
    /// A whole message is held until `consume`.
    held: bool,
}

#[derive(Debug)]
pub struct BleSetup {
    stage: Stage,
    /// The next event to read.
    seq: u32,
    connection: Option<Connection>,
    advertising: Advertising,
}

impl Default for BleSetup {
    fn default() -> Self {
        Self {
            stage: Stage::Start,
            seq: 1,
            connection: None,
            advertising: Advertising::Off,
        }
    }
}

impl BleSetup {
    /// The held message, after [`Link::Message`].
    pub fn message(&self) -> Option<&[u8]> {
        let connection = self.connection.as_ref().filter(|c| c.held)?;
        let len = parse_prefix([connection.message[0], connection.message[1]])?;
        connection.message.get(PREFIX..PREFIX + len)
    }

    /// Done with the held message; read the next. Bytes of a following
    /// message that arrived with it move to the front.
    pub fn consume(&mut self) {
        let Some(connection) = self.connection.as_mut().filter(|c| c.held) else {
            return;
        };
        let len = parse_prefix([connection.message[0], connection.message[1]]).unwrap_or(0);
        let end = (PREFIX + len).min(connection.received);
        connection.message.copy_within(end..connection.received, 0);
        connection.received -= end;
        connection.held = false;
    }

    /// Notify `body` as one framed message.
    pub fn send(&mut self, body: &[u8]) -> Result<(), DropReason> {
        if !self.connection.as_ref().is_some_and(|c| c.open) {
            return Err(DropReason::Notify);
        }
        let prefix = prefix(body.len()).ok_or(DropReason::TooLong)?;
        let mut framed = [0u8; PREFIX + MAX_MESSAGE];
        framed[..PREFIX].copy_from_slice(&prefix);
        framed[PREFIX..PREFIX + body.len()].copy_from_slice(body);
        if sdk::ble_notify(&framed[..PREFIX + body.len()]) {
            Ok(())
        } else {
            Err(DropReason::Notify)
        }
    }

    /// Drop the host. Its disconnect event arrives as [`Link::Closed`].
    pub fn close(&mut self, reason: DropReason) -> Record {
        sdk::ble_disconnect();
        if let Some(connection) = self.connection.as_mut() {
            connection.open = false;
            connection.held = false;
        }
        Record::Drop { reason }
    }

    /// One event's effect, its record and what the firmware must act on.
    fn on_event(&mut self, event: BleEvent, now: u32) -> (Option<Record>, Option<Link>) {
        match event.kind {
            BleEvent::READY => {
                let Stage::Starting { .. } = self.stage else {
                    return (None, None);
                };
                let stage = match event.result {
                    0 => {
                        self.stage = Stage::Running;
                        let heap_free = sdk::ble_stats().heap_free;
                        return (Some(Record::Ready { heap_free }), None);
                    }
                    BleEvent::READY_WIFI => FailStage::Radio,
                    BleEvent::READY_BTE => FailStage::Bte,
                    BleEvent::READY_SERVICE => FailStage::Service,
                    _ => FailStage::Stack,
                };
                self.stage = Stage::Failed;
                (Some(Record::Failed { stage }), None)
            }
            BleEvent::ADVERTISING => {
                let on = event.value != 0;
                (Some(Record::Advertising { on }), None)
            }
            BleEvent::CONNECT => {
                self.connection = Some(Connection {
                    since: now,
                    open: false,
                    message: [0; PREFIX + MAX_MESSAGE],
                    received: 0,
                    held: false,
                });
                let interval = event.value;
                (Some(Record::Connect { interval }), None)
            }
            BleEvent::DISCONNECT => {
                let link = self.connection.take().map(|_| Link::Closed);
                let cause = event.result;
                (Some(Record::Disconnect { cause }), link)
            }
            BleEvent::MTU => (Some(Record::Mtu { mtu: event.value }), None),
            BleEvent::NOTIFY if event.value != 0 => {
                let Some(connection) = self.connection.as_mut() else {
                    return (None, None);
                };
                // A host that resubscribes starts over.
                connection.open = true;
                connection.received = 0;
                connection.held = false;
                (None, Some(Link::Opened))
            }
            _ => (None, None),
        }
    }

    /// Advertise exactly while the firmware wants it and nobody is connected.
    fn advertise(&mut self, wanted: bool) {
        let wanted = if wanted && self.connection.is_none() {
            Advertising::On
        } else {
            Advertising::Off
        };
        if wanted != self.advertising && sdk::ble_advertise(wanted) {
            self.advertising = wanted;
        }
    }

    /// Read the host's bytes until one message is whole.
    fn read(&mut self, now: u32) -> (Option<Record>, Option<Link>) {
        let Some(connection) = self.connection.as_mut() else {
            return (None, None);
        };
        if now.wrapping_sub(connection.since) >= SESSION_MS {
            return (Some(self.close(DropReason::Timeout)), None);
        }
        if !connection.open || connection.held {
            return (None, None);
        }

        let received = connection.received;
        connection.received += sdk::ble_control_read(&mut connection.message[received..]);
        if connection.received < PREFIX {
            return (None, None);
        }
        let Some(len) = parse_prefix([connection.message[0], connection.message[1]]) else {
            return (Some(self.close(DropReason::TooLong)), None);
        };
        if connection.received < PREFIX + len {
            return (None, None);
        }
        connection.held = true;
        (None, Some(Link::Message))
    }

    /// One step; `advertise` is the firmware's wish (setup mode, on mains).
    pub fn step(&mut self, advertise: bool) -> Step {
        let now = sdk::uptime_ms();
        match sdk::ble_event(self.seq) {
            BlePoll::Event(event) => {
                self.seq = self.seq.wrapping_add(1);
                let (record, link) = self.on_event(event, now);
                if record.is_some() || link.is_some() {
                    return Step {
                        delay_ms: 1,
                        record,
                        link,
                    };
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
                    let stage = FailStage::Task;
                    return next(1, Some(Record::Failed { stage }));
                }
                self.stage = Stage::Starting {
                    deadline: now.wrapping_add(READY_MS),
                };
                next(IDLE_MS, None)
            }
            Stage::Starting { deadline } => {
                if deadline_passed(now, deadline) {
                    self.stage = Stage::Failed;
                    let stage = FailStage::Timeout;
                    return next(1, Some(Record::Failed { stage }));
                }
                next(IDLE_MS, None)
            }
            Stage::Running => {
                self.advertise(advertise);
                let (record, link) = self.read(now);
                let delay_ms = match (record, link, &self.connection) {
                    (None, None, None) => IDLE_MS,
                    (None, None, Some(_)) => CONNECTED_MS,
                    _ => 1,
                };
                Step {
                    delay_ms,
                    record,
                    link,
                }
            }
            Stage::Failed => next(FAILED_MS, None),
        }
    }
}
