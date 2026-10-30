//! One feeder's device API session for the app: open it, keep what the Home
//! card and the Today tab show current, read the feeding history, and route
//! Dart's requests. The connection, keepalive and event cursor are
//! `wellfed_connect`'s; this loop only routes commands in and updates out,
//! and [`crate::requests`] builds each request's ops.
//!
//! A feed goes out once, under a fresh request id, and is never sent
//! again: an answer that never came means the feed may have run, and its
//! `Fed` event (or the history) settles it. Every other write goes the same
//! way, and its absent answer comes back as such.
//!
//! After pairing, or whenever the feeder's clock is not set, the session
//! gives the feeder this phone's clock and zone, as `wellfed time` does.
//! That sync carries the zone's next change too, so a feeder with no zone
//! setting keeps local time across it.
//!
//! A session reads the history from the oldest event the feeder keeps, which
//! is what "last fed" and today's meals need. It holds nothing across
//! sessions, so a new session reads it again: up to the ring's 1,408 records,
//! about a frame a loop pass.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokio::sync::{mpsc, oneshot};
use wellfed_client::history::{Backlog, Entry, Feed};
use wellfed_connect::{Connection, Received};
use wellfed_model::{Answer, EventBody, Op, Pushed};
use wellfed_session::noise::Keypair;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::keys::random;
use crate::api::session::{FeedRefusal, HistoryEntry, SessionUpdate};
use crate::today::{Facts, event_entry, gap_entry, result};

/// How long one wait on the connection lasts before the loop looks at its
/// commands again and reads the status anew; the session keeps itself alive
/// meanwhile. So the card's meals and signal are at most this old.
const TURN: Duration = Duration::from_secs(30);
/// While the history comes in: a backlog whose newest event this session
/// skips never reaches its end, so a quiet moment ends it instead.
const QUIET: Duration = Duration::from_secs(2);

/// The device API's request id.
type RequestId = [u8; 16];

#[derive(Debug)]
pub enum Command {
    Feed {
        portions: u8,
    },
    CancelFeed {
        feed: u32,
    },
    Refresh,
    /// One op; `reply` hears its answer, `None` if none came, and is dropped
    /// unused if it never went out.
    Ask {
        op: Op,
        reply: oneshot::Sender<Option<Answer>>,
    },
    Close,
}

/// What an outstanding request was for.
enum Asked {
    Status,
    Settings,
    Plans,
    Time,
    Feed { portions: u8 },
    Cancel,
    Clock,
    Ask(oneshot::Sender<Option<Answer>>),
}

/// Run the session until Dart closes it or the feeder goes away. `update`
/// returns `false` once nobody is listening, which ends the session too.
pub async fn run(
    keys: Keypair,
    feeder: [u8; 32],
    address: SocketAddr,
    sync_clock: bool,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut update: impl FnMut(SessionUpdate) -> bool,
) {
    let mut asked = HashMap::new();
    let error = serve(
        &keys,
        feeder,
        address,
        sync_clock,
        &mut commands,
        &mut update,
        &mut asked,
    )
    .await
    .err();

    // A session that ends answers nothing still outstanding, and each of
    // those writes may have run: say so before the end.
    for what in asked.into_values() {
        match what {
            Asked::Feed { .. } => {
                update(SessionUpdate::FeedUnanswered);
            }
            Asked::Ask(reply) => {
                // Nobody waiting means nobody to tell.
                let _ = reply.send(None);
            }
            _ => {}
        }
    }
    update(SessionUpdate::Closed { error });
}

/// The session's own requests and the history as it comes in.
struct Serving<'a, U> {
    connection: Connection,
    update: &'a mut U,
    asked: &'a mut HashMap<RequestId, Asked>,
    facts: Facts,
    /// Until it ends, events only fill `batch`; after it they are news.
    backlog: Option<Backlog>,
    batch: Vec<HistoryEntry>,
}

async fn serve(
    keys: &Keypair,
    feeder: [u8; 32],
    address: SocketAddr,
    sync_clock: bool,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    update: &mut impl FnMut(SessionUpdate) -> bool,
    asked: &mut HashMap<RequestId, Asked>,
) -> Result<(), BridgeError> {
    let mut connection = Connection::open(keys, feeder, address, random()?).await?;
    if !update(SessionUpdate::Opened {
        build: connection.build().to_owned(),
        role: connection.role(),
    }) {
        return Ok(());
    }

    let Answer::Status(status) = connection.ask(random()?, &Op::Status).await? else {
        return Err(BridgeError::new(
            ErrorKind::Protocol,
            "the feeder did not answer with its status",
        ));
    };
    let backlog = Backlog {
        newest: status.last_event,
    };
    let mut serving = Serving {
        connection,
        update,
        asked,
        facts: Facts::default(),
        backlog: Some(backlog),
        batch: Vec::new(),
    };
    if sync_clock || status.clock.is_none() {
        let sync = wellfed_client::time::sync(Timestamp::now(), &TimeZone::system());
        serving.request(&Op::TimeSync(sync), Asked::Clock)?;
    }
    serving.request(&Op::GetSettings, Asked::Settings)?;
    serving.request(&Op::GetPlans, Asked::Plans)?;
    serving.request(&Op::GetTime, Asked::Time)?;
    serving.connection.subscribe(None, false).await?;
    serving.facts.status = Some(status);
    if !serving.card() {
        return Ok(());
    }
    if backlog.read(None) && !serving.end_backlog() {
        return Ok(());
    }

    loop {
        let wait = if serving.backlog.is_some() {
            QUIET
        } else {
            TURN
        };
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                let (op, what) = match command {
                    Command::Close => return Ok(()),
                    Command::Refresh => (Op::Status, Asked::Status),
                    Command::Feed { portions } => (Op::Feed { portions }, Asked::Feed { portions }),
                    Command::CancelFeed { feed } => (Op::CancelFeed { feed }, Asked::Cancel),
                    Command::Ask { op, reply } => (op, Asked::Ask(reply)),
                };
                serving.request(&op, what)?;
            }
            received = serving.connection.receive(wait) => {
                let more = match received {
                    Err(wellfed_connect::Error::Timeout) => serving.turn()?,
                    other => match other? {
                        Received::Answer { id, answer } => serving.answered(id, answer)?,
                        Received::Pushed(pushed) => serving.pushed(&pushed)?,
                    },
                };
                if !more {
                    return Ok(());
                }
            }
        }
    }
}

impl<U: FnMut(SessionUpdate) -> bool> Serving<'_, U> {
    fn request(&mut self, op: &Op, what: Asked) -> Result<(), BridgeError> {
        let id = random()?;
        self.connection.request(id, op)?;
        self.asked.insert(id, what);
        Ok(())
    }

    /// Say the card again; `false` once nobody listens.
    fn card(&mut self) -> bool {
        match self.facts.card() {
            Some(card) => (self.update)(SessionUpdate::Status { card }),
            None => true,
        }
    }

    /// Read the status again, unless a read is already on its way.
    fn refresh(&mut self) -> Result<(), BridgeError> {
        if !self
            .asked
            .values()
            .any(|what| matches!(what, Asked::Status))
        {
            self.request(&Op::Status, Asked::Status)?;
        }
        Ok(())
    }

    /// Hand over the history so far.
    fn end_backlog(&mut self) -> bool {
        self.backlog = None;
        let entries = std::mem::take(&mut self.batch);
        (self.update)(SessionUpdate::History {
            entries,
            caught_up: true,
        }) && self.card()
    }

    /// Nothing came for a whole wait.
    fn turn(&mut self) -> Result<bool, BridgeError> {
        if self.backlog.is_some() && !self.end_backlog() {
            return Ok(false);
        }
        self.refresh()?;
        Ok(true)
    }

    fn answered(&mut self, id: RequestId, answer: Option<Answer>) -> Result<bool, BridgeError> {
        let Some(what) = self.asked.remove(&id) else {
            return Ok(true);
        };
        match (what, answer) {
            (Asked::Status, Some(Answer::Status(status))) => {
                self.facts.status = Some(status);
                Ok(self.card())
            }
            (Asked::Settings, Some(Answer::Settings(settings))) => {
                self.facts.settings = Some(settings);
                Ok(self.card())
            }
            (Asked::Plans, Some(Answer::Plans(plans))) => {
                self.facts.plans = Some(plans);
                Ok(self.card())
            }
            (Asked::Time, Some(Answer::Time(time))) => {
                self.facts.time = Some(time);
                Ok(self.card())
            }
            (Asked::Ask(reply), answer) => {
                // Dart stopped waiting: nothing to tell it.
                let _ = reply.send(answer);
                Ok(true)
            }
            (Asked::Clock, Some(Answer::Done { .. })) => {
                self.request(&Op::GetTime, Asked::Time)?;
                Ok((self.update)(SessionUpdate::ClockSynced))
            }
            (Asked::Clock, _) => Ok((self.update)(SessionUpdate::ClockNotSynced)),
            (what, answer) => {
                Ok(feed_answered(what, answer).is_none_or(|message| (self.update)(message)))
            }
        }
    }

    fn pushed(&mut self, pushed: &Pushed) -> Result<bool, BridgeError> {
        if let Pushed::Log(log) = pushed {
            return Ok((self.update)(SessionUpdate::Log {
                seq: log.seq,
                dropped: log.dropped,
                lines: log.lines.clone(),
            }));
        }

        let fed = match Entry::from_pushed(pushed) {
            Some(Entry::Feed(feed)) => Some(feed),
            _ => None,
        };
        if let Some(feed) = fed {
            self.facts.feeds.push(feed);
        }
        let entry = match pushed {
            Pushed::Event(event) => event_entry(event),
            Pushed::Gap(gap) => gap_entry(gap),
            Pushed::Sample(_) | Pushed::Log(_) => return Ok(true),
        };

        if let Some(backlog) = self.backlog {
            self.batch.push(entry);
            return Ok(!backlog.ends_at(pushed) || self.end_backlog());
        }
        if !(self.update)(SessionUpdate::History {
            entries: vec![entry],
            caught_up: true,
        }) {
            return Ok(false);
        }
        self.news(pushed, fed.as_ref())
    }

    /// An event after the backlog: any may move the card (a feed, the lock,
    /// the hopper), so it is read again.
    fn news(&mut self, pushed: &Pushed, fed: Option<&Feed>) -> Result<bool, BridgeError> {
        if let Pushed::Event(event) = pushed {
            match event.body {
                EventBody::Plans { .. } => self.request(&Op::GetPlans, Asked::Plans)?,
                EventBody::Settings { .. } => self.request(&Op::GetSettings, Asked::Settings)?,
                EventBody::ClockSync { .. } => self.request(&Op::GetTime, Asked::Time)?,
                _ => {}
            }
        }
        self.refresh()?;
        match fed {
            Some(fed) => Ok((self.update)(SessionUpdate::FeedEnded {
                result: result(fed),
            })),
            None => Ok(true),
        }
    }
}

/// The screen's update for a feed's or a cancel's answer, or its absence.
fn feed_answered(what: Asked, answer: Option<Answer>) -> Option<SessionUpdate> {
    match (what, answer) {
        (Asked::Feed { portions }, Some(Answer::Feeding { feed })) => {
            Some(SessionUpdate::FeedStarted { feed, portions })
        }
        (Asked::Feed { .. }, Some(Answer::Refused(refusal))) => Some(SessionUpdate::FeedRefused {
            refusal: FeedRefusal::from(refusal),
        }),
        // The feed may have run; it is never sent again.
        (Asked::Feed { .. }, None) => Some(SessionUpdate::FeedUnanswered),
        (Asked::Cancel, Some(Answer::Refused(refusal))) => Some(SessionUpdate::CancelUnconfirmed {
            refusal: Some(FeedRefusal::from(refusal)),
        }),
        (Asked::Cancel, None) => Some(SessionUpdate::CancelUnconfirmed { refusal: None }),
        _ => None,
    }
}
