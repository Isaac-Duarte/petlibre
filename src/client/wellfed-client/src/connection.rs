//! A whole device API session, sans-IO: what every client runs over its
//! own socket, so the CLI, the server and the app keep one set of session
//! rules.
//!
//! The caller owns the connection and the clock, in the style of
//! quinn-proto: it feeds received bytes to [`Connection::handle_input`],
//! writes what [`Connection::poll_transmit`] returns, wakes at
//! [`Connection::poll_timeout`] to call [`Connection::handle_timeout`],
//! and takes results from [`Connection::poll_event`]. Time comes in as an
//! argument and is never read here, so tests run hours of a session in
//! microseconds.
//!
//! The connection owns:
//! - **Keepalive.** The feeder drops a session that sends nothing for its
//!   idle limit (90 s), so a `Status` goes out under [`KEEPALIVE_ID`] when
//!   this side has been quiet for [`Timing::keepalive`]; its answer is
//!   swallowed.
//! - **Silence.** A live feeder answers every keepalive, so hearing nothing
//!   for [`Timing::silence`] ends the session with [`Error::Silent`]: a
//!   feeder that lost power sends no reset, and the kernel would keep the
//!   socket open for many minutes.
//! - **Request–answer matching.** Each [`Connection::request`] ends in
//!   exactly one [`Event::Answer`] or, after [`Timing::answer`] or when the
//!   session ends first, [`Event::Unanswered`]. An unanswered request may have run: a write is
//!   retried only under the same id, which the feeder answers from memory
//!   without running it twice, and a feed is never retried automatically.
//! - **The event cursor.** [`Connection::subscribe`] takes the newest
//!   event a mirror holds as a [`Cursor`], and resumes after it only when
//!   it counts in the feeder's current epoch; a cursor from a re-created
//!   ring starts over from the oldest event kept ([`Resume::Restarted`]).
//!   [`Connection::cursor`] follows every event and gap pushed since.
//!
//! LAN recovery, the button-hold window's setup exchange over the device
//! API's port, is [`Recovery`].

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use wellfed_model::setup::Reply;
use wellfed_model::{Answer, Hello, Op, OpenRefusal, Pushed};
use wellfed_proto::api::{Open, RequestId};
use wellfed_session::noise::Keypair;

pub use crate::api::Opened;
use crate::api::{Error, Incoming, Session, open_frame};
use crate::setup;

/// The id of the keepalive's `Status`. A read is never remembered by the
/// feeder, so reusing one id is safe; its answer is dropped here, so a
/// caller may not use it.
pub const KEEPALIVE_ID: RequestId = *b"wellfed-alive\0\0\0";

/// The session's clocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// A request is answered at once, or after its store write; the
    /// welcome comes within this too.
    pub answer: Duration,
    /// Well inside the feeder's 90 s idle limit, with room for a lost
    /// segment.
    pub keepalive: Duration,
    /// Room for a keepalive and its answer, with margin.
    pub silence: Duration,
}

impl Timing {
    pub const DEFAULT: Self = Self {
        answer: Duration::from_secs(10),
        keepalive: Duration::from_secs(45),
        silence: Duration::from_secs(90),
    };
}

impl Default for Timing {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Where a mirror of the feeder's history stands: the newest event (or
/// gap end) it holds, in the epoch it counts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub epoch: u32,
    pub seq: u64,
}

/// What [`Connection::subscribe`] did with the mirror's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resume {
    /// The history continues after the cursor (or from the oldest event,
    /// with none).
    Continued,
    /// The cursor counts in an epoch the feeder no longer has: its ring was
    /// re-created, and the history starts again from its oldest event.
    Restarted,
}

/// What [`Connection::poll_event`] yields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The feeder's answer to the request under `id`.
    Answer { id: RequestId, answer: Answer },
    /// No answer within [`Timing::answer`], or the session ended first:
    /// the request may have run. Its late answer, if any, is dropped.
    Unanswered { id: RequestId },
    /// An event, gap or sample the feeder pushed.
    Pushed(Pushed),
}

/// An enrolled client's session over one connection.
#[derive(Debug)]
pub struct Connection {
    session: Session,
    timing: Timing,
    opened: Option<Opened>,
    /// Until the welcome: when it is overdue.
    welcome_by: Instant,
    transmit: VecDeque<Vec<u8>>,
    events: VecDeque<Event>,
    /// Requests sent and not yet answered, with when they are overdue.
    pending: Vec<(RequestId, Instant)>,
    cursor: Option<Cursor>,
    /// When this side last queued bytes, for the keepalive.
    sent_at: Instant,
    /// When the feeder was last heard, for the silence limit.
    heard_at: Instant,
    /// The error that ended the session; every later call returns it.
    ended: Option<Ended>,
}

/// Why the session ended, kept so later calls repeat it.
#[derive(Clone, Copy, Debug)]
enum Ended {
    Refused(OpenRefusal),
    Timeout,
    Silent,
    Failed,
}

impl Ended {
    fn error(self) -> Error {
        match self {
            Self::Refused(refusal) => Error::Refused(refusal),
            Self::Timeout => Error::Timeout,
            Self::Silent => Error::Silent,
            Self::Failed => Error::Unexpected,
        }
    }
}

impl Connection {
    /// Start a session with the feeder whose identity is `feeder`, claiming
    /// every capability this crate knows; the first bytes are ready in
    /// [`Self::poll_transmit`]. `ephemeral` comes from the OS generator,
    /// fresh.
    pub fn new(
        keys: &Keypair,
        feeder: [u8; 32],
        ephemeral: [u8; 32],
        now: Instant,
    ) -> Result<Self, Error> {
        Self::with(
            keys,
            feeder,
            ephemeral,
            Hello::CURRENT,
            Timing::DEFAULT,
            now,
        )
    }

    /// As [`Self::new`], with another hello or other clocks.
    pub fn with(
        keys: &Keypair,
        feeder: [u8; 32],
        ephemeral: [u8; 32],
        hello: Hello,
        timing: Timing,
        now: Instant,
    ) -> Result<Self, Error> {
        let (session, bytes) = Session::connect_with(keys, feeder, ephemeral, hello)?;
        Ok(Self {
            session,
            timing,
            opened: None,
            welcome_by: now + timing.answer,
            transmit: VecDeque::from([bytes]),
            events: VecDeque::new(),
            pending: Vec::new(),
            cursor: None,
            sent_at: now,
            heard_at: now,
            ended: None,
        })
    }

    /// The feeder's welcome, once it has arrived.
    pub fn opened(&self) -> Option<&Opened> {
        self.opened.as_ref()
    }

    /// The newest event or gap end pushed since [`Self::subscribe`], or the
    /// cursor it resumed after.
    pub fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }

    fn live(&self) -> Result<(), Error> {
        self.ended.map_or(Ok(()), |ended| Err(ended.error()))
    }

    /// End the session. Every request still waiting is reported
    /// unanswered, so each request still ends in exactly one event.
    fn end(&mut self, ended: Ended) -> Error {
        self.ended = Some(ended);
        self.transmit.clear();
        for (id, _) in self.pending.drain(..) {
            self.events.push_back(Event::Unanswered { id });
        }
        ended.error()
    }

    /// Bytes from the connection. An error ends the session.
    pub fn handle_input(&mut self, now: Instant, bytes: &[u8]) -> Result<(), Error> {
        self.live()?;
        self.heard_at = now;
        if let Err(error) = self.session.receive(bytes) {
            self.end(Ended::Failed);
            return Err(error);
        }
        loop {
            match self.take() {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(Taken::Refused(refusal)) => return Err(self.end(Ended::Refused(refusal))),
                Err(Taken::Failed(error)) => {
                    self.end(Ended::Failed);
                    return Err(error);
                }
            }
        }
    }

    /// One opened message into the session's state; false once none is
    /// left.
    fn take(&mut self) -> Result<bool, Taken> {
        let Some(incoming) = self.session.next_incoming().map_err(Taken::Failed)? else {
            return Ok(false);
        };
        match incoming {
            Incoming::Opened(opened) => self.opened = Some(opened),
            Incoming::Refused(refusal) => return Err(Taken::Refused(refusal)),
            Incoming::Answer { id, .. } if id == KEEPALIVE_ID => {}
            Incoming::Answer { id, answer } => {
                // A request already reported unanswered keeps that outcome.
                if let Some(index) = self.pending.iter().position(|(sent, _)| *sent == id) {
                    self.pending.swap_remove(index);
                    self.events.push_back(Event::Answer { id, answer });
                }
            }
            Incoming::Pushed(pushed) => {
                match &pushed {
                    Pushed::Event(event) => {
                        self.cursor = Some(Cursor {
                            epoch: event.epoch,
                            seq: event.seq,
                        });
                    }
                    Pushed::Gap(gap) => {
                        self.cursor = Some(Cursor {
                            epoch: gap.epoch,
                            seq: gap.to,
                        });
                    }
                    // Log lines count per boot, outside the history: the
                    // caller keeps their cursor.
                    Pushed::Sample(_) | Pushed::Log(_) => {}
                }
                self.events.push_back(Event::Pushed(pushed));
            }
        }
        Ok(true)
    }

    /// The next bytes to write, in order.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        self.transmit.pop_front()
    }

    /// The next result, in arrival order.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// When [`Self::handle_timeout`] is next due; `None` once the session
    /// has ended.
    pub fn poll_timeout(&self) -> Option<Instant> {
        if self.ended.is_some() {
            return None;
        }
        let silence = self.heard_at + self.timing.silence;
        if self.opened.is_none() {
            return Some(self.welcome_by.min(silence));
        }
        let keepalive = self.sent_at + self.timing.keepalive;
        let overdue = self.pending.iter().map(|&(_, by)| by).min();
        Some(overdue.map_or(silence.min(keepalive), |by| by.min(silence).min(keepalive)))
    }

    /// Act on the clock: report overdue answers, send the keepalive, or end
    /// a silent session or one never welcomed.
    pub fn handle_timeout(&mut self, now: Instant) -> Result<(), Error> {
        self.live()?;
        if self.opened.is_none() && self.welcome_by <= now {
            return Err(self.end(Ended::Timeout));
        }
        if self.heard_at + self.timing.silence <= now {
            return Err(self.end(Ended::Silent));
        }
        if self.opened.is_none() {
            return Ok(());
        }

        let mut index = 0;
        while let Some(&(id, by)) = self.pending.get(index) {
            if by <= now {
                self.pending.swap_remove(index);
                self.events.push_back(Event::Unanswered { id });
            } else {
                index += 1;
            }
        }
        if self.sent_at + self.timing.keepalive <= now {
            self.send_request(now, KEEPALIVE_ID, &Op::Status)?;
        }
        Ok(())
    }

    /// Send one request under `id`; its answer, or its absence, comes from
    /// [`Self::poll_event`]. `id` may not be [`KEEPALIVE_ID`].
    pub fn request(&mut self, now: Instant, id: RequestId, op: &Op) -> Result<(), Error> {
        self.request_within(now, id, op, self.timing.answer)
    }

    /// As [`Self::request`], for a request whose answer waits on someone
    /// at the feeder (an update key's Dispense press): unanswered only
    /// after `answer`.
    pub fn request_within(
        &mut self,
        now: Instant,
        id: RequestId,
        op: &Op,
        answer: Duration,
    ) -> Result<(), Error> {
        if id == KEEPALIVE_ID {
            return Err(Error::ReservedId);
        }
        self.send_request(now, id, op)?;
        if !self.pending.iter().any(|(sent, _)| *sent == id) {
            self.pending.push((id, now + answer));
        }
        Ok(())
    }

    fn send_request(&mut self, now: Instant, id: RequestId, op: &Op) -> Result<(), Error> {
        self.live()?;
        if self.opened.is_none() {
            return Err(Error::NotOpen);
        }
        let bytes = self.session.request(id, op)?;
        self.queue(now, bytes);
        Ok(())
    }

    /// Ask for the history after `from`, the newest event a mirror holds,
    /// then each new event, and the live samples if `samples`. A cursor
    /// from another epoch, or none, reads from the oldest event kept.
    pub fn subscribe(
        &mut self,
        now: Instant,
        from: Option<Cursor>,
        samples: bool,
    ) -> Result<Resume, Error> {
        self.live()?;
        let Some(opened) = &self.opened else {
            return Err(Error::NotOpen);
        };
        let (resume, cursor) = match from {
            Some(cursor) if cursor.epoch != opened.epoch => (Resume::Restarted, None),
            cursor => (Resume::Continued, cursor),
        };

        let bytes = self
            .session
            .subscribe(cursor.map(|cursor| cursor.seq), samples)?;
        self.cursor = cursor;
        self.queue(now, bytes);
        Ok(resume)
    }

    fn queue(&mut self, now: Instant, bytes: Vec<u8>) {
        self.transmit.push_back(bytes);
        self.sent_at = now;
    }
}

enum Taken {
    Refused(OpenRefusal),
    Failed(Error),
}

/// The recovery window's session over the device API: the setup exchange
/// ([`setup::Session`]) after a clear [`Open::Recovery`] frame, each
/// message in a setup frame. The code proof keeps a LAN neighbor out
/// while the window is open, as it keeps a radio neighbor out of BLE
/// setup.
#[derive(Debug)]
pub struct Recovery {
    session: setup::Session,
    deframer: setup::Deframer,
    transmit: VecDeque<Vec<u8>>,
    replies: VecDeque<Reply>,
}

impl Recovery {
    /// Start recovering with `session`; the first bytes are ready in
    /// [`Self::poll_transmit`].
    pub fn new(mut session: setup::Session) -> Result<Self, Error> {
        let mut first = open_frame(Open::Recovery)?;
        first.extend(setup::frame(&session.start()?)?);
        Ok(Self {
            session,
            deframer: setup::Deframer::default(),
            transmit: VecDeque::from([first]),
            replies: VecDeque::new(),
        })
    }

    /// Bytes from the connection.
    pub fn handle_input(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.deframer.push(bytes);
        while let Some(message) = self.deframer.next_message()? {
            match self.session.receive(&message)? {
                setup::Step::Send(bytes) => self.transmit.push_back(setup::frame(&bytes)?),
                setup::Step::Reply(reply) => self.replies.push_back(reply),
            }
        }
        Ok(())
    }

    /// The next bytes to write, in order.
    pub fn poll_transmit(&mut self) -> Option<Vec<u8>> {
        self.transmit.pop_front()
    }

    /// The feeder's next reply: `EnterCode`, then `Done` or `Refused`.
    pub fn poll_reply(&mut self) -> Option<Reply> {
        self.replies.pop_front()
    }

    /// The setup session: its code, and the feeder's key once proven.
    pub fn session(&self) -> &setup::Session {
        &self.session
    }
}
