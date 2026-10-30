//! One feeder's device API session for the app: open it, keep the Home
//! card's status current, and run feeds. The connection, keepalive and
//! event cursor are `wellfed_connect`'s; this loop only routes Dart's
//! commands in and the screen's updates out.
//!
//! A feed goes out once, under a fresh request id, and is never sent
//! again: an answer that never came means the feed may have run, and its
//! `Fed` event (or the history) settles it.
//!
//! After pairing, or whenever the feeder's clock is not set, the session
//! gives the feeder this phone's clock and zone, as `wellfed time` does.
//! That sync carries the zone's next change too, so a feeder with no zone
//! setting keeps local time across it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokio::sync::mpsc;
use wellfed_client::history::{Delivered, Feed};
use wellfed_connect::{Connection, Cursor, Received};
use wellfed_model::{Answer, Battery, FeedOutcome, Op, Pushed, Readiness, Status};
use wellfed_session::noise::Keypair;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::keys::random;
use crate::api::session::{Card, FeedRefusal, FeedResult, SessionUpdate};

/// How long one wait on the connection lasts before the loop looks at its
/// commands again; the session keeps itself alive meanwhile.
const TURN: Duration = Duration::from_secs(30);

/// The device API's request id.
type RequestId = [u8; 16];

#[derive(Debug)]
pub enum Command {
    Feed { portions: u8 },
    CancelFeed { feed: u32 },
    Refresh,
    Close,
}

/// What an outstanding request was for.
enum Asked {
    Status,
    Feed { portions: u8 },
    Cancel,
    Clock,
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
    // those feeds may have run: say so before the end.
    for what in asked.into_values() {
        if matches!(what, Asked::Feed { .. }) {
            update(SessionUpdate::FeedUnanswered);
        }
    }
    update(SessionUpdate::Closed { error });
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

    // Only events after the status just read matter to the card: the
    // history screen reads the rest.
    let Answer::Status(status) = connection.ask(random()?, &Op::Status).await? else {
        return Err(BridgeError::new(
            ErrorKind::Protocol,
            "the feeder did not answer with its status",
        ));
    };
    if sync_clock || status.clock.is_none() {
        let sync = wellfed_client::time::sync(Timestamp::now(), &TimeZone::system());
        let id = random()?;
        connection.request(id, &Op::TimeSync(sync))?;
        asked.insert(id, Asked::Clock);
    }
    let from = status.last_event.map(|seq| Cursor {
        epoch: connection.epoch(),
        seq,
    });
    connection.subscribe(from, false).await?;
    if !update(SessionUpdate::Status {
        card: card(&status),
    }) {
        return Ok(());
    }

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                let (op, what) = match command {
                    Command::Close => return Ok(()),
                    Command::Refresh => (Op::Status, Asked::Status),
                    Command::Feed { portions } => (Op::Feed { portions }, Asked::Feed { portions }),
                    Command::CancelFeed { feed } => (Op::CancelFeed { feed }, Asked::Cancel),
                };
                let id = random()?;
                connection.request(id, &op)?;
                asked.insert(id, what);
            }
            received = connection.receive(TURN) => {
                let received = match received {
                    Err(wellfed_connect::Error::Timeout) => continue,
                    other => other?,
                };
                let message = match received {
                    Received::Answer { id, answer } => {
                        let Some(what) = asked.remove(&id) else { continue };
                        answered(what, answer)
                    }
                    Received::Pushed(Pushed::Event(event)) => {
                        // Any event may move the card (a feed, the lock,
                        // the hopper), so it is read again.
                        if !asked.values().any(|what| matches!(what, Asked::Status)) {
                            let id = random()?;
                            connection.request(id, &Op::Status)?;
                            asked.insert(id, Asked::Status);
                        }
                        Feed::from_event(&event).map(|fed| SessionUpdate::FeedEnded { result: result(&fed) })
                    }
                    Received::Pushed(_) => None,
                };
                if let Some(message) = message
                    && !update(message)
                {
                    return Ok(());
                }
            }
        }
    }
}

/// The screen's update for an answer, or its absence.
fn answered(what: Asked, answer: Option<Answer>) -> Option<SessionUpdate> {
    match (what, answer) {
        (Asked::Status, Some(Answer::Status(status))) => Some(SessionUpdate::Status {
            card: card(&status),
        }),
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
        (Asked::Clock, Some(Answer::Done { .. })) => Some(SessionUpdate::ClockSynced),
        (Asked::Clock, _) => Some(SessionUpdate::ClockNotSynced),
        _ => None,
    }
}

pub(crate) fn card(status: &Status) -> Card {
    let adapter = status
        .supply
        .and_then(|supply| supply.adapter)
        .or(status.power.map(|power| power.adapter));
    let battery_percent = status.supply.and_then(|supply| match supply.battery {
        Battery::Reading { percent, .. } => Some(percent),
        _ => None,
    });
    let blocked = match status.readiness {
        Some(Readiness::Blocked(reason)) => Some(reason),
        Some(Readiness::Ready) | None => None,
    };
    Card {
        build: status.build.clone(),
        food: status.food,
        locked: status.locked,
        ready: status.readiness == Some(Readiness::Ready),
        blocked,
        adapter,
        battery_percent,
        clock_set: status.clock.is_some(),
        rssi: status.rssi,
        fault: status.feed_fault.map(|fault| fault.outcome),
    }
}

pub(crate) fn result(fed: &Feed) -> FeedResult {
    let (confirmed, at_most) = match fed.delivered {
        Delivered::Counted(count) => (count, count),
        Delivered::Uncertain {
            confirmed,
            requested,
        } => (confirmed, requested),
    };
    FeedResult {
        feed: fed.feed,
        requested: fed.requested,
        confirmed,
        at_most,
        outcome: fed.outcome,
        reason: fed.reason,
        uncertain: fed.outcome == FeedOutcome::Uncertain,
        words: fed.delivered.describe(fed.requested, fed.outcome),
    }
}
