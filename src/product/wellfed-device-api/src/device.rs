//! What the server needs from the rest of the feeder, and the values it
//! passes across: an update request for the installer, a client store
//! change, an event read from the ring.

use crate::clients::{Label, Role};
use crate::mailbox::{
    Calibration, CalibrationResult, LockResult, Origin, PlayResult, SettingsFields, SettingsResult,
    TimeFields, TimeResult, TimeSettings, TimeSettingsResult, TimeState, WifiResult,
};
use crate::wifi::Credentials;
use wellfed_core::log_tail::Batch as LogBatch;
use wellfed_core::named_enum;
use wellfed_core::ota::offer::KeyId;
use wellfed_core::tone::Tune as CoreTune;
use wellfed_proto::api::{
    self as proto, Bank1Page, Blocked, Bowl, Capabilities, EventBody, Phase, Readiness, Refusal,
    Source, Stats, StatsSection, Status, TracePage, UpdateView,
};

/// The rest of the feeder, as the server needs it. Every call returns at
/// once: a write is queued for its owner, and the server polls for the
/// result on later passes.
pub trait Device {
    /// Fill `out` from the vetted generator; false when it has no output.
    fn random(&mut self, out: &mut [u8; 32]) -> bool;
    /// The build identity, for the welcome and status.
    fn build(&self) -> &'static str;
    /// Everything [`Status`] reports but the session count.
    fn status(&mut self, now_ms: u32) -> Status<'static>;
    /// The settings, or one change queued for the writer.
    fn settings(&mut self, change: SettingsFields) -> SettingsResult;
    /// The Wi-Fi store; with `Some`, one change (`Some(None)` clears) queued.
    fn wifi(&mut self, change: Option<Option<Credentials>>) -> WifiResult;
    fn lock(&mut self, locked: bool) -> LockResult;
    /// `None` when this firmware does not play tunes outside setup mode.
    fn play(&mut self, tune: CoreTune) -> Option<PlayResult>;
    /// A client's sync; `origin` says whether it may step the clock.
    fn time_sync(&mut self, fields: TimeFields, origin: Origin) -> TimeResult;
    /// Queue a client store change; its ticket, or `None` while the mailbox
    /// is busy.
    fn keys(&mut self, change: KeyChange) -> Option<u32>;
    /// The result of the change with `ticket`, once written.
    fn keys_outcome(&mut self, ticket: u32) -> Option<KeyOutcome>;
    /// What this firmware serves beyond major version 1's first ops.
    fn capabilities(&self) -> Capabilities;
    /// Whether a feed can start now, and why not: the one decision both
    /// the status reports and [`Self::feed`] is asked under. A device with
    /// no feeding owner is [`Blocked::Unserved`].
    fn readiness(&mut self) -> Readiness {
        Readiness::Blocked(Blocked::Unserved)
    }
    /// Queue one feed for the feeding owner on `by`'s behalf: the number
    /// its journal will hold it under. Asked only while
    /// [`Self::readiness`] is ready; the owner still refuses one that lost
    /// a race (a join that began since).
    fn feed(&mut self, portions: u8, by: Source) -> Result<u32, Refusal>;
    /// Whether the feed `feed` queued is durably reserved yet: `None` while
    /// it waits, or why it ended before it was.
    fn feed_reserved(&mut self, feed: u32) -> Option<Result<(), Refusal>>;
    /// Stop the feed `feed` if it is still running; false once it has ended.
    fn cancel_feed(&mut self, feed: u32) -> bool;
    /// Record one durable event.
    fn record(&mut self, now_ms: u32, body: EventBody);
    /// The first durable event after `after` (`None`: from the oldest).
    fn event_after(&mut self, after: Option<u64>) -> EventRead;
    /// The event ring's epoch, `None` until the ring is open. A session is
    /// welcomed only once it is known.
    fn epoch(&self) -> Option<u32>;
    /// Restart the feeder.
    fn reboot(&mut self);
    /// One section of the statistics, read when its answer is sealed. Asked
    /// only of a device whose capabilities carry the section's bit.
    fn stats(&mut self, section: StatsSection, now_ms: u32) -> Stats<'_>;
    /// One page of feed `feed`'s trace from byte `offset` of its body, read
    /// when its answer is sealed: `NotFound` when no trace of it is kept,
    /// `Busy` while it is still recorded, `Invalid` for an offset past the
    /// body. Asked only of a device whose capabilities carry the traces.
    fn feed_trace(&mut self, feed: u32, offset: u16) -> Result<TracePage<'_>, Refusal>;
    /// One page of bank 1 from `offset`, read when its answer is sealed:
    /// served only by the bank-0 recovery role (`Unavailable` elsewhere),
    /// `Busy` while an install holds the flash, `Invalid` at or past the
    /// bank's end. Asked only of a device whose capabilities carry
    /// [`Capabilities::BACKUP`], and only for an owner.
    fn bank1_page(&mut self, _offset: u32) -> Result<Bank1Page<'_>, Refusal> {
        Err(Refusal::Unavailable)
    }

    /// Queue an update request for the installer: its ticket, or why it
    /// cannot be queued now. A device without [`Capabilities::UPDATE`]
    /// serves none.
    fn update(&mut self, _request: UpdateRequest<'_>) -> Result<u32, Refusal> {
        Err(Refusal::Unavailable)
    }
    /// The result of the update request with `ticket`, once done.
    fn update_outcome(&mut self, _ticket: u32) -> Option<Result<UpdateAnswer, Refusal>> {
        None
    }
    /// Hand one chunk to the install: the state once it is taken (or was
    /// not wanted), `Busy` while the last ones are still being written.
    fn update_chunk(&mut self, _offset: u32, _data: &[u8]) -> Result<UpdateView, Refusal> {
        Err(Refusal::Unavailable)
    }
    /// The update's state now.
    fn update_view(&mut self) -> Option<UpdateView> {
        None
    }
    /// An install holds the flash (or is about to restart): no feed starts.
    fn installing(&self) -> bool {
        false
    }
    /// A feed is queued or runs: no install starts.
    fn feed_active(&self) -> bool {
        false
    }

    /// The time settings; with `Some`, the whole new settings queued for
    /// the writer. A device without [`Capabilities::TIME`] keeps none.
    fn time_settings(&mut self, _change: Option<TimeSettings>) -> TimeSettingsResult {
        TimeSettingsResult::Unserved
    }
    /// The clock and the NTP exchange now.
    fn time_state(&mut self, _now_ms: u32) -> Option<TimeState> {
        None
    }
    /// The portion calibration; with `Some`, the whole new calibration
    /// queued for the writer. A device without
    /// [`Capabilities::CALIBRATION`] keeps none.
    fn calibration(&mut self, _change: Option<Calibration>) -> CalibrationResult {
        CalibrationResult::Unserved
    }
    /// The bowl mode this build runs: the only one a calibration may name,
    /// since a build runs only the mode it was verified for.
    fn bowl(&self) -> Bowl {
        Bowl::Single
    }
    /// Where the rotor rests, once the feeding owner knows; `None` from a
    /// device with no owner.
    fn phase(&self) -> Option<Phase> {
        None
    }
    /// The logged lines after line `after` (`None`: from the oldest kept)
    /// that fit `text` whole, each ending in `\n` and printable ASCII.
    /// `None` when there is nothing new. A device without
    /// [`Capabilities::LOG`] keeps none.
    fn log_after(&mut self, _after: Option<u32>, _text: &mut [u8]) -> Option<LogBatch> {
        None
    }
}

/// An update op, as the device's installer takes it. The bytes are copied
/// into its mailbox before `Device::update` returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateRequest<'a> {
    Offer {
        manifest: &'a [u8],
        downgrade: Option<&'a [u8]>,
        /// The feeder's identity key, which a downgrade statement names.
        device: [u8; 32],
    },
    Statement {
        statement: &'a [u8],
    },
    Cancel,
}

named_enum! {
    /// What became of an update key.
    pub enum UpdateKeyChange {
        /// An owner enrolled it, confirmed by a Dispense press.
        Enrolled = "enrolled",
        /// A key statement filled the release slot with it.
        Added = "added",
        /// A key statement retired it for good.
        Retired = "retired",
    }
}

/// What an update request came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAnswer {
    View(UpdateView),
    /// A key statement took effect on the key with this id.
    Key {
        change: UpdateKeyChange,
        id: KeyId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyChange {
    Enroll {
        key: [u8; 32],
        label: Label,
        role: Role,
    },
    Revoke {
        key: [u8; 32],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    Done,
    Full,
    Duplicate,
    NotFound,
    NotWritable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventRead {
    Event(proto::Event),
    /// Events `from..=to` are gone from the ring.
    Gap {
        from: u64,
        to: u64,
    },
    /// Nothing newer yet.
    None,
    /// Ask again on a later pass (the writer is reading flash).
    Wait,
}
