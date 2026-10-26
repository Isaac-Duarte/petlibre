//! [`Platform`]: everything the loop pass reaches outside the product's own
//! state. The PLAF103 firmware implements it over the SDK's drivers and
//! mailboxes, the simulator over its stores in memory and real sockets.
//!
//! Every call returns at once: a store write is queued for the settings
//! writer, which completes it on its own turn, and the loop asks again on a
//! later pass. The calls are generic (`Feeder<P>`), never `dyn`, so the
//! firmware's stack audit follows each one.

use core::fmt;

use wellfed_core::buttons::Gesture;
use wellfed_core::clock::wall::{Jump, Sync, SyncError};
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::tone::Tune;
use wellfed_proto::api::{Capabilities, Power};

use crate::mailbox::{
    ClientChange, KeyResult, PlayResult, Requested, SettingsFields, SettingsResult, SettingsView,
    WifiResult, WifiSource,
};
use crate::store::clients::{CLIENTS, Client};
use crate::store::events::{BODY_BYTES, Read};
use crate::store::net::Credentials;

/// This boot's facts, for the `Boot` event and every status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Boot {
    pub bank: u8,
    pub reset_flags: u32,
    pub boot_count: u32,
}

/// The Wi-Fi link as status reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub rssi: i32,
    pub link_losses: u32,
    /// Losses the link's checks found (its gateway stopped answering, or its
    /// lease went).
    pub check_losses: u32,
}

/// What a non-blocking accept found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accepted<S> {
    Socket(S),
    /// No connection waits.
    None,
    /// The listener failed; open a new one.
    Error,
}

/// What a non-blocking receive found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    Data(usize),
    /// Nothing waiting now.
    Empty,
    /// The peer closed the connection, or the socket failed.
    Closed,
}

/// One mDNS datagram, if one waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Datagram {
    Got { len: usize, ip: u32, port: u16 },
    None,
    Error,
}

/// Which of the event queue's slots a record may take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSlot {
    /// Any but the one kept for a feed's outcome.
    Shared,
    /// A feed's `Fed` event, which may take the kept slot too.
    Outcome,
}

/// How a settled indicator level read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The level holds (read back, or consistent where the pad has no
    /// input readback).
    Held,
    InputMismatch,
    /// Outputs or the pad disagree: stop driving the indicator.
    Abort,
}

/// What setup mode's BLE link asks the loop to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleLink {
    /// A host connected and enabled notifications: a new session.
    Opened,
    /// A whole message waits in [`Platform::ble_message`].
    Message,
    /// The host left or was dropped: forget its session.
    Closed,
}

/// Why setup mode drops its BLE host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleDrop {
    /// The session ended; its last reply has had time to go out.
    Done,
    /// The host broke the protocol.
    Protocol,
}

/// One step of a driver the platform owns: the delay it asks for, and its
/// own record, if it made one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverStep<R> {
    pub delay_ms: u32,
    pub record: Option<R>,
}

/// One step of setup mode's BLE link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BleStep<R> {
    pub delay_ms: u32,
    pub record: Option<R>,
    pub link: Option<BleLink>,
}

/// A record a platform's own driver makes (the BLE link's, the speaker's),
/// logged in order with the pass's under its own tag; its `Display` is the
/// line's body.
pub trait DriverRecord: Copy + fmt::Debug + fmt::Display + PartialEq {
    /// The record line's tag, `PL…1`.
    fn tag(&self) -> &'static str;
}

/// A platform with no driver records of its own.
impl DriverRecord for core::convert::Infallible {
    fn tag(&self) -> &'static str {
        match *self {}
    }
}

/// The rest of the feeder, as one loop pass reaches it.
pub trait Platform {
    /// A connected socket, the listener or the mDNS socket.
    type Socket: Copy + PartialEq + fmt::Debug;
    /// What the platform's own drivers record.
    type Record: DriverRecord;

    /// The lock indicator's pad, for its record.
    const LOCK_INDICATOR: &'static str;
    /// How long a requested indicator level settles before it reads back.
    const INDICATOR_SETTLE_MS: u32;
    /// How often the supply is sampled, and so how often a subscriber gets
    /// a power sample.
    const POWER_SAMPLE_MS: u32;

    // ---- The build and the boot.

    /// The build identity, as hex.
    fn build(&self) -> &'static str;
    fn boot(&self) -> Boot;
    /// What this firmware serves beyond the protocol's first ops.
    fn capabilities(&self) -> Capabilities;
    /// Restart the feeder (a watchdog reset on hardware).
    fn reset(&mut self);
    /// Fill `out` from the vetted generator; false when it has no output.
    fn random(&mut self, out: &mut [u8; 32]) -> bool;
    /// The identity secret and its generation, if published since `seen`
    /// (0 before the first).
    fn identity_since(&mut self, seen: u32) -> Option<(u32, [u8; 32])>;

    // ---- The panel.

    /// The button service's next queued gesture.
    fn gesture(&mut self) -> Option<Gesture>;
    /// Drive the lock indicator; false when the request is refused.
    fn indicator(&mut self, on: bool) -> bool;
    /// How the lock indicator's settled level reads back against `on`.
    fn indicator_check(&mut self, on: bool) -> Check;
    /// Drive every indicator off, for good.
    fn indicators_off(&mut self);
    /// The supply, once sampled.
    fn power(&mut self, now_ms: u32) -> Option<Power>;
    fn link(&mut self) -> Link;
    /// This boot's address, once DHCP gave one.
    fn ipv4(&mut self) -> Option<u32>;

    // ---- The settings writer's mailboxes.

    /// The settings as the writer last published them.
    fn settings(&mut self) -> Option<SettingsView>;
    /// The settings, or one change queued for the writer.
    fn settings_request(&mut self, change: SettingsFields) -> SettingsResult;
    /// The Wi-Fi store as the writer last published it.
    fn wifi(&mut self) -> WifiResult;
    /// Queue a Wi-Fi save (`None` clears) for the writer.
    fn wifi_change(&mut self, credentials: Option<Credentials>) -> WifiResult;
    /// Whether a Wi-Fi change waits for the writer.
    fn wifi_saving(&self) -> bool;
    /// Which credentials this boot's joins use.
    fn wifi_source(&self) -> WifiSource;
    /// Queue a client store change for the writer.
    fn keys_request(&mut self, change: ClientChange) -> Requested;
    /// The ticket of the change last queued.
    fn keys_ticket(&self) -> u32;
    /// The result of the change with `ticket`, once written.
    fn keys_outcome(&self, ticket: u32) -> Option<KeyResult>;
    /// Copy the enrolled clients into `table` if published since `seen` (0
    /// before the first); their generation.
    fn clients_since(&mut self, seen: u32, table: &mut [Option<Client>; CLIENTS]) -> Option<u32>;

    // ---- The event history.

    /// Queue one record for the event ring; false (and counted lost) when
    /// the queue has no `slot` left.
    fn queue_event(&mut self, at: Option<u32>, body: [u8; BODY_BYTES], slot: EventSlot) -> bool;
    /// Records the queue dropped since the last call.
    fn take_lost(&mut self) -> u32;
    /// The ring's answer for `after`, `None` while the writer reads it.
    fn event_after(&mut self, after: Option<u64>) -> Option<Read>;
    /// The ring's epoch, once the writer has opened it.
    fn epoch(&self) -> Option<u32>;
    /// The ring's newest record.
    fn last_event(&self) -> Option<u64>;

    // ---- The wall clock.

    /// Unix milliseconds now, while the clock is valid.
    fn wall_ms(&mut self, now_ms: u32) -> Option<u64>;
    /// Check and adopt a sync from a client.
    fn time_sync(&mut self, now_ms: u32, sync: Sync) -> Result<Jump, SyncError>;

    // ---- The outputs' turn (feeds, joins and store writes).

    /// A feed holds the outputs: store writes wait for it.
    fn outputs_claimed(&self) -> bool;
    /// A feed holds the outputs and has not run past its bound.
    fn claim_in_bound(&self, now_ms: u32) -> bool;

    // ---- The update stage's mailbox.

    fn update_offer(
        &mut self,
        manifest: &[u8],
        downgrade: Option<&[u8]>,
        device: [u8; 32],
    ) -> Result<u32, Refused>;
    fn update_statement(&mut self, statement: &[u8]) -> Result<u32, Refused>;
    fn update_cancel(&mut self) -> Result<u32, Refused>;
    /// The answer to the request with `ticket`, once the stage has one.
    fn update_outcome(&mut self, ticket: u32) -> Option<Result<Answer, Refused>>;
    fn update_chunk(&mut self, offset: u32, data: &[u8]) -> Result<(), Refused>;
    /// The stage's state, once it has published one.
    fn update_progress(&mut self) -> Option<Progress>;
    /// An install holds the flash (or is about to restart).
    fn installing(&self) -> bool;

    // ---- The device API's sockets.

    fn listen(&mut self, port: u16) -> Option<Self::Socket>;
    fn accept(&mut self, listener: Self::Socket) -> Accepted<Self::Socket>;
    fn receive(&mut self, socket: Self::Socket, into: &mut [u8]) -> Received;
    /// Bytes sent, `None` once the socket failed.
    fn send(&mut self, socket: Self::Socket, bytes: &[u8]) -> Option<usize>;
    fn close(&mut self, socket: Self::Socket);
    fn mdns_open(&mut self) -> Option<Self::Socket>;
    fn mdns_send(&mut self, socket: Self::Socket, bytes: &[u8], ip: u32, port: u16) -> bool;
    fn mdns_receive(&mut self, socket: Self::Socket, into: &mut [u8]) -> Datagram;
    /// The heap free now, and its low point.
    fn heap(&self) -> (u32, u32);

    // ---- Setup mode's BLE link and speaker, which run only in setup mode.

    /// A fresh link, before setup mode's first step.
    fn ble_restart(&mut self);
    /// Step the link; advertise only while `advertise`.
    fn ble_step(&mut self, advertise: bool) -> BleStep<Self::Record>;
    /// The held message, after [`BleLink::Message`].
    fn ble_message(&self) -> Option<&[u8]>;
    /// Done with the held message.
    fn ble_consume(&mut self);
    /// Notify `body` as one message; false when it did not go.
    fn ble_send(&mut self, body: &[u8]) -> bool;
    /// Drop the host; its disconnect arrives as [`BleLink::Closed`]. The
    /// link's record of the drop, if it keeps one.
    fn ble_close(&mut self, reason: BleDrop) -> Option<Self::Record>;
    /// A quiet speaker, before setup mode's first step.
    fn speaker_restart(&mut self);
    /// Nothing queued or playing.
    fn speaker_idle(&self) -> bool;
    /// Queue a tune for the speaker.
    fn play(&mut self, tune: Tune) -> PlayResult;
    fn speaker_step(&mut self) -> DriverStep<Self::Record>;
}
