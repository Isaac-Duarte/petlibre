//! What the feed tests share: a quick answer deadline, and reading what
//! the server and the simulator hold of feeds.

use std::rc::Rc;
use std::time::Duration;

use wellfed_api::{
    Command, CommandOutcome, CommandState, EventBody, EventKind, EventQuery, FeedOutcome,
    FeedReason, FeederId,
};
use wellfed_server_testkit::feeder::{self, Flash};
use wellfed_server_testkit::{ada, config, until};
use wellfed_service::{CommandError, Config, Error, IdempotencyKey, Service};

/// A test service whose callers stop waiting for an answer after 2 s, so a
/// lost answer is unknown without waiting out the default.
pub fn quick() -> Config {
    let mut config = config();
    config.timing.answer = Duration::from_secs(2);
    config
}

/// The `Fed` events the server holds.
pub async fn fed_events(service: &Service, id: FeederId) -> Vec<EventBody> {
    let query = EventQuery {
        kind: Some(EventKind::Fed),
        ..EventQuery::default()
    };
    let page = service.events(id, query).await.unwrap();
    page.events.into_iter().map(|event| event.body).collect()
}

/// Wait until `key`'s entry is resolved, and return it.
pub async fn resolved(service: &Service, key: IdempotencyKey) -> Command {
    until("the feed is settled from the history", || async {
        service
            .command(&ada(), key)
            .await
            .is_ok_and(|entry| entry.state == CommandState::Resolved)
    })
    .await;
    service.command(&ada(), key).await.unwrap()
}

/// The feeder fed once, and the server holds that one feed's `Fed` event.
pub async fn one_feed(service: &Service, id: FeederId, flash: &Rc<Flash>) {
    assert_eq!(
        feeder::feeds(flash),
        1,
        "the feeder's journal holds one feed"
    );
    let fed = fed_events(service, id).await;
    assert_eq!(fed.len(), 1, "{fed:?}");
}

pub fn done(feed: u32, portions: u8) -> CommandOutcome {
    CommandOutcome::Fed {
        feed,
        requested: portions,
        completed: portions,
        outcome: FeedOutcome::Done,
        reason: FeedReason::Done,
    }
}

pub fn unknown<T: std::fmt::Debug>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(Error::Command(CommandError::Unknown)))
}
