//! The session's two command lanes: short commands (status, settings, the
//! lock, a feed) and a long operation's steps (install chunks, trace and
//! stats pages), each with at most one request in flight.
//!
//! The feeder reads one request per client per pass, only once the last
//! answer is out and any write it started has finished, and answers in the
//! order they arrived. So the task pipelines: a short command goes out
//! while a step is still in flight and is answered right after it, and the
//! operation's next step goes out only once no short command is in flight
//! or waiting. A short command therefore waits at most one step, and a
//! long operation yields between its steps.
//!
//! One request in flight per lane keeps a cut session's uncertain writes to
//! two, and keeps a queued command unsent until its turn, so one whose
//! caller left is dropped rather than sent late. Answers are matched to
//! their request by id; a command whose id is already in flight on the
//! other lane is held until that one is answered, so one answer never
//! settles two requests. When the session ends, every request in flight is
//! unknown and every held one was never sent.

use wellfed_connect::Connection;
use wellfed_model::Answer;

use crate::command::Command;
use crate::session::End;
use crate::{AskError, RequestId};

/// Which lane a command came in on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Short,
    Long,
}

/// The session's requests in flight, per lane.
#[derive(Default)]
pub(crate) struct Lanes {
    short: Slot,
    long: Slot,
}

#[derive(Default)]
struct Slot {
    flight: Option<Command>,
    /// Taken from the lane but not yet sent: its id is in flight on the
    /// other lane.
    held: Option<Command>,
}

impl Slot {
    fn free(&self) -> bool {
        self.flight.is_none() && self.held.is_none()
    }

    fn in_flight(&self, id: RequestId) -> bool {
        self.flight.as_ref().is_some_and(|command| command.id == id)
    }
}

impl Lanes {
    /// Whether the short lane takes its next command.
    pub(crate) fn short_free(&self) -> bool {
        self.short.free()
    }

    /// Whether the long lane takes its next step: only with the short lane
    /// idle and no short command `waiting`, so a step never goes out ahead
    /// of one.
    pub(crate) fn long_free(&self, waiting: bool) -> bool {
        self.long.free() && self.short.free() && !waiting
    }

    /// Send `command` on `lane`, or hold it while its id is in flight on
    /// the other lane. One whose caller left is dropped unsent.
    pub(crate) fn dispatch(
        &mut self,
        lane: Lane,
        command: Command,
        connection: &mut Connection,
    ) -> Result<(), End> {
        let (slot, other) = self.pick(lane);
        if command.left() {
            return Ok(());
        }
        if other.in_flight(command.id) {
            slot.held = Some(command);
            return Ok(());
        }
        send(slot, command, connection)
    }

    /// The answer to `id`, or its absence, for the command in flight under
    /// it; then a held command may go. An answer to no command here (one
    /// of the task's own, given up on) is dropped.
    pub(crate) fn answered(
        &mut self,
        id: RequestId,
        answer: Option<Answer>,
        connection: &mut Connection,
    ) -> Result<(), End> {
        let slot = if self.short.in_flight(id) {
            &mut self.short
        } else if self.long.in_flight(id) {
            &mut self.long
        } else {
            return Ok(());
        };
        if let Some(command) = slot.flight.take() {
            command.reply(answer.ok_or(AskError::Unknown));
        }

        for lane in [Lane::Short, Lane::Long] {
            let (slot, other) = self.pick(lane);
            let Some(held) = slot.held.take() else {
                continue;
            };
            if held.left() {
                continue;
            }
            if other.in_flight(held.id) {
                slot.held = Some(held);
                continue;
            }
            send(slot, held, connection)?;
        }
        Ok(())
    }

    /// The session ended: each command in flight may have run, and each
    /// held one never went out.
    pub(crate) fn lost(&mut self) {
        for slot in [&mut self.short, &mut self.long] {
            if let Some(command) = slot.flight.take() {
                command.reply(Err(AskError::Unknown));
            }
            if let Some(command) = slot.held.take() {
                command.offline();
            }
        }
    }

    fn pick(&mut self, lane: Lane) -> (&mut Slot, &Slot) {
        match lane {
            Lane::Short => (&mut self.short, &self.long),
            Lane::Long => (&mut self.long, &self.short),
        }
    }
}

/// Queue `command` on the session and keep it in flight on `slot`. A
/// request refused before anything was queued leaves the session as it
/// was; any other failure ends it, and the command is then unknown.
fn send(slot: &mut Slot, command: Command, connection: &mut Connection) -> Result<(), End> {
    match connection.request(command.id, &command.op) {
        Ok(()) => {
            slot.flight = Some(command);
            Ok(())
        }
        Err(wellfed_connect::Error::ReservedId | wellfed_connect::Error::Encode) => {
            command.reply(Err(AskError::Unsendable));
            Ok(())
        }
        Err(error) => {
            command.reply(Err(AskError::Unknown));
            Err(End::Lost(error))
        }
    }
}
