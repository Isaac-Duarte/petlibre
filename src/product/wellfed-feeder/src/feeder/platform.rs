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
use wellfed_core::clock::wall::{Fix, Jump, Origin, State as ClockState, Sync, SyncError};
use wellfed_core::clock::zone::Rule;
use wellfed_core::indicator::Light;
use wellfed_core::log_tail::Batch as LogBatch;
use wellfed_core::ota::install::{Answer, Progress, Refused};
use wellfed_core::supply::Reading;
use wellfed_core::tone::Tune;
use wellfed_proto::api::{
    Capabilities, Counts, Erases, Power, Refusal, STOCK_PLAN_ENTRIES_LEN, TASKS_MAX, TraceSummary,
};
use wellfed_proto::trace::PAGE_MAX;

use super::supply::Radio;
use crate::stats::Totals;
use crate::store::events::{BODY_BYTES, Read};
use wellfed_device_api::Reader;
use wellfed_device_api::clients::{CLIENTS, Client};
use wellfed_device_api::mailbox::{
    Calibration, CalibrationResult, CallChange, CallResult, ClientChange, KeyResult, PlansChange,
    PlansResult, PlayResult, Requested, SettingsFields, SettingsResult, SettingsView, TimeSettings,
    TimeSettingsResult, WifiResult, WifiSource,
};
use wellfed_device_api::wifi::Credentials;

/// This boot's facts, for the `Boot` event and every status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Boot {
    pub bank: u8,
    pub reset_flags: u32,
    pub boot_count: u32,
}

/// Whether the image was built with Wi-Fi credentials of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildWifi {
    /// A bench image: the C entry prefills the bench hotspot.
    Present,
    /// A consumer image: the only network is the one the owner stores, so
    /// a boot with none stored starts BLE setup mode on its own.
    Absent,
}

/// The Wi-Fi link as status and the statistics report it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    pub rssi: i32,
    pub link_losses: u32,
    /// Losses the link's checks found (its gateway stopped answering, or its
    /// lease went).
    pub check_losses: u32,
    /// Leases renewed (or rebound) without a rejoin.
    pub dhcp_renewals: u32,
    /// Gateway probes answered this boot, and the latest one's round trip.
    pub gateway_answers: u32,
    pub gateway_ms: u32,
    /// The joined access point's channel, `None` while not joined.
    pub channel: Option<u8>,
    /// Joins this boot that landed on another access point than the join
    /// before.
    pub ap_changes: u32,
}

/// One task's stack, as the RTOS reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskStack {
    name: [u8; Self::NAME],
    len: u8,
    /// The fewest free bytes since the task started.
    pub free_min: u32,
}

impl TaskStack {
    /// A name's longest, as the device API carries it.
    pub const NAME: usize = 16;
    pub const EMPTY: Self = Self {
        name: [0; Self::NAME],
        len: 0,
        free_min: 0,
    };

    /// `name` cut to [`Self::NAME`] bytes, at a character boundary.
    pub fn new(name: &str, free_min: u32) -> Self {
        let mut end = name.len().min(Self::NAME);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut stack = Self {
            free_min,
            ..Self::default()
        };
        stack.name[..end].copy_from_slice(&name.as_bytes()[..end]);
        stack.len = u8::try_from(end).unwrap_or(0);
        stack
    }

    pub fn name(&self) -> &str {
        let len = usize::from(self.len).min(Self::NAME);
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

/// What [`Platform::task_stacks`] filled, and how many tasks there are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskCount {
    pub listed: usize,
    pub total: u32,
}

/// The event ring's fill, for the statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventsFill {
    pub kept: u32,
    pub capacity: u32,
}

/// The trace ring's fill, and the traces that never reached it, for the
/// statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TracesFill {
    pub kept: u32,
    pub capacity: u32,
    pub unsaved: u32,
}

/// What [`Platform::trace_page`] copied: the trace's summary, whether it is
/// in storage, its body's whole length and the bytes copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceRead {
    pub summary: TraceSummary,
    pub stored: bool,
    pub total: u16,
    pub len: usize,
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
pub use wellfed_core::indicator::Check;

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
    /// Whether the image has build Wi-Fi to join with nothing stored.
    fn build_wifi(&self) -> BuildWifi;
    /// A reset storm holds this boot: the feeding owner stays closed until
    /// the hold's time has run. Held boots step the loop all the same.
    fn held(&self) -> bool;
    /// Restart the feeder (a watchdog reset on hardware).
    fn reset(&mut self);
    /// A restart was asked for ([`Self::reset`]) and has not come yet: no
    /// feed may start, since the restart would leave it uncertain.
    fn restarting(&self) -> bool;
    /// The settings writer could not finish a factory reset this boot and
    /// asks for a restart to try again.
    fn restart_wanted(&self) -> bool;
    /// Fill `out` from the vetted generator; false when it has no output.
    fn random(&mut self, out: &mut [u8; 32]) -> bool;
    /// The identity secret and its generation, if published since `seen`
    /// (0 before the first).
    fn identity_since(&mut self, seen: u32) -> Option<(u32, [u8; 32])>;

    // ---- The panel.

    /// The button service's next queued gesture.
    fn gesture(&mut self) -> Option<Gesture>;
    /// Drive `light`, whatever the motor and emitters are doing; false when
    /// the request is refused.
    fn indicator(&mut self, light: Light, on: bool) -> bool;
    /// How `light`'s settled level reads back against `on`.
    fn indicator_check(&mut self, light: Light, on: bool) -> Check;
    /// Drive every indicator off, for good.
    fn indicators_off(&mut self);
    /// The supply, once sampled.
    fn power(&mut self, now_ms: u32) -> Option<Power>;
    /// The supply judged, unknowns named, with the pack's charge on the
    /// board's curve.
    fn supply(&mut self, now_ms: u32) -> Reading;
    /// Bring the Wi-Fi radio up or take it down (battery mode), asked every
    /// pass. Off leaves the network and turns the radio off; on turns it
    /// back on and rejoins. The call only records the wish: the platform's
    /// network task acts on it.
    fn radio(&mut self, radio: Radio);
    fn link(&mut self) -> Link;
    /// This boot's address, once DHCP gave one.
    fn ipv4(&mut self) -> Option<u32>;
    /// The lease's gateway, once DHCP gave one.
    fn gateway(&mut self) -> Option<u32>;

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
    /// The time settings as the writer last published them.
    fn time_settings(&mut self) -> TimeSettingsResult;
    /// Queue the whole new time settings for the writer.
    fn time_settings_change(&mut self, settings: TimeSettings) -> TimeSettingsResult;
    /// The portion calibration as the writer last published it.
    fn calibration(&mut self) -> CalibrationResult;
    /// Queue the whole new calibration for the writer.
    fn calibration_change(&mut self, calibration: Calibration) -> CalibrationResult;
    /// The feeding plans as the writer last published them.
    fn plans(&mut self) -> PlansResult;
    /// Queue one change to the plans for the writer.
    fn plans_change(&mut self, change: PlansChange) -> PlansResult;
    /// The meal call as the writer last published it.
    fn call(&mut self) -> CallResult;
    /// Queue an upload's start or a delete for the writer.
    fn call_change(&mut self, change: CallChange) -> CallResult;
    /// Queue one chunk of the upload under way for the writer.
    fn call_chunk(&mut self, offset: u32, data: &[u8]) -> Result<(), Refusal>;

    // ---- The event history.

    /// Queue one record for the event ring; false (and counted lost) when
    /// the queue has no `slot` left.
    fn queue_event(&mut self, at: Option<u32>, body: [u8; BODY_BYTES], slot: EventSlot) -> bool;
    /// Records the queue dropped since the last call.
    fn take_lost(&mut self) -> u32;
    /// The ring's answer for `after`, for the subscriber in slot `reader`;
    /// `None` while the writer reads it, or while it waits its turn.
    fn event_after(&mut self, reader: Reader, after: Option<u64>) -> Option<Read>;
    /// The subscriber in slot `reader` ended.
    fn reader_left(&mut self, _reader: Reader) {}
    /// The ring's epoch, once the writer has opened it.
    fn epoch(&self) -> Option<u32>;
    /// The ring's newest record.
    fn last_event(&self) -> Option<u64>;

    // ---- The log tail.

    /// The record lines logged after line `after` (`None`: from the oldest
    /// the RAM tail keeps) that fit `text`; `None` when there are none, or
    /// the platform keeps no tail (it then claims no `Capabilities::LOG`).
    fn log_after(&self, after: Option<u32>, text: &mut [u8]) -> Option<LogBatch>;

    // ---- The wall clock.

    /// The clock's own millisecond count, at the loop's `now_ms`: one that
    /// keeps counting while interrupts are masked, where the loop's may
    /// not. The NTP exchange runs on it, so a flash erase during an
    /// exchange lengthens the round trip, and the error, it measures.
    fn clock_ms(&mut self, now_ms: u32) -> u32;
    /// Unix milliseconds now, while the clock is valid.
    fn wall_ms(&mut self, now_ms: u32) -> Option<u64>;
    /// Check and adopt a sync from a client; `origin` says whether it may
    /// step the clock.
    fn time_sync(&mut self, now_ms: u32, sync: Sync, origin: Origin) -> Result<Jump, SyncError>;
    /// Adopt an NTP fix measured at `now_ms` on the clock's count
    /// ([`Self::clock_ms`]).
    fn time_fix(&mut self, now_ms: u32, fix: Fix) -> Result<Jump, SyncError>;
    /// The clock's state now.
    fn clock_state(&mut self, now_ms: u32) -> ClockState;
    /// The zone's offset east of UTC at `unix_ms`, from its rule or a
    /// client sync's offsets; `None` with neither.
    fn offset_at(&self, unix_ms: u64) -> Option<i32>;
    /// Set or clear the zone's rule.
    fn set_zone_rule(&mut self, rule: Option<Rule>);

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
    /// Copy bank 1's bytes from `offset` into `out`, through the mapped
    /// flash (a read takes no lease). False when this platform cannot read
    /// bank 1: it runs from bank 1, or keeps no banks.
    fn bank1_read(&mut self, _offset: u32, _out: &mut [u8]) -> bool {
        false
    }
    /// Stock's plan count byte, with its first entries copied into
    /// `entries`, through the mapped flash. `None` when this platform keeps
    /// no stock flash. Read only: the replacement never writes stock's
    /// plan sectors, so a rollback finds its plans as it left them.
    fn stock_plans_read(&mut self, _entries: &mut [u8; STOCK_PLAN_ENTRIES_LEN]) -> Option<u8> {
        None
    }

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
    /// The NTP exchange's socket: UDP on an ephemeral port.
    fn ntp_open(&mut self) -> Option<Self::Socket>;
    fn ntp_send(&mut self, socket: Self::Socket, bytes: &[u8], ip: u32, port: u16) -> bool;
    fn ntp_receive(&mut self, socket: Self::Socket, into: &mut [u8]) -> Datagram;
    /// The heap free now, and its low point.
    fn heap(&self) -> (u32, u32);

    // ---- The statistics.

    /// Each task's stack into `out`, and how many tasks there are. The
    /// RTOS may pause the scheduler to walk the stacks.
    fn task_stacks(&mut self, out: &mut [TaskStack; TASKS_MAX]) -> TaskCount;
    /// Each storage sector's erases this boot, and bank 1's.
    fn erases(&self) -> Erases;
    /// The event ring's fill, as the writer last published it.
    fn events_fill(&self) -> EventsFill;
    /// The lifetime totals the stats store holds, this boot's reset
    /// counted, as the writer published them: `None` before it opened the
    /// store, or when the store keeps none it can write.
    fn totals(&self) -> Option<Totals>;
    /// This boot's counts for the writer's next save of the totals; the
    /// loop hands them over every sample window, never writing itself.
    fn hand_counts(&mut self, counts: Counts);

    // ---- The per-feed traces.

    /// The trace ring's fill, as the writer last published it.
    fn traces_fill(&self) -> TracesFill;
    /// Copy feed `feed`'s trace body from `offset` into `out`: from the
    /// latest feed's trace in RAM, or from the storage ring. `NotFound` when
    /// neither holds it, `Busy` while that feed is still recorded,
    /// `Invalid` for an offset past the body.
    fn trace_page(
        &mut self,
        feed: u32,
        offset: u16,
        out: &mut [u8; PAGE_MAX],
    ) -> Result<TraceRead, Refusal>;

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
    /// Queue the meal call for the speaker, `plays` times. The loop steps
    /// the speaker while it plays, outside setup mode as well.
    fn play_call(&mut self, plays: u8) -> PlayResult;
    fn speaker_step(&mut self) -> DriverStep<Self::Record>;

    // ---- The buzzer.

    /// Make the buzzer's line an output, LOW. Called once, with no feed
    /// queued or running, since it rewrites a port register the motor
    /// shares.
    fn buzzer_enable(&mut self);
    /// Drive the panel's buzzer (PA25 on the PLAF103, an active buzzer:
    /// on beeps at its own pitch), once enabled.
    fn buzzer(&mut self, on: bool);
}
