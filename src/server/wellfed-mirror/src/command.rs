//! The command surface: [`Mirror::ask`] for a short command, and an
//! [`Operation`] for a long one that sends its steps one at a time.
//!
//! Each reaches the feeder task on its own lane ([`crate::lanes`]): a
//! short command goes out even while a step is in flight, and the next step
//! waits for the short commands, so a status read or a write waits at most
//! one step. Only one long operation runs per feeder.

use std::sync::Arc;

use wellfed_api::{Connection as Link, FeederId};
use wellfed_model::{Answer, Op};
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot, watch};
use tokio::time::{Duration, Instant, timeout_at};

use crate::task::LongLane;
use crate::{AskError, Mirror, Random, RequestId};

/// One request for the feeder, and where its answer goes.
pub(crate) struct Command {
    pub(crate) id: RequestId,
    pub(crate) op: Op,
    reply: oneshot::Sender<Result<Answer, AskError>>,
}

/// A long operation's turn on a feeder: its steps go out one at a time on
/// the long lane, and short commands go first. Dropping it ends the
/// operation.
pub struct Operation {
    steps: mpsc::Sender<Command>,
    pub(crate) link: watch::Receiver<Link>,
    answer: Duration,
    /// Request ids for the steps an install sends.
    pub(crate) random: Arc<dyn Random>,
    _turn: OwnedSemaphorePermit,
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation").finish_non_exhaustive()
    }
}

impl Operation {
    /// Send one step and wait for its answer, as [`Mirror::ask`] does.
    pub async fn step(&mut self, request: RequestId, op: Op) -> Result<Answer, AskError> {
        let link = *self.link.borrow();
        send(&self.steps, link, self.answer, request, op).await
    }
}

impl Mirror {
    /// Hand `op` to the feeder's task under `request` and wait for its
    /// answer. Turned away at once while the feeder is offline;
    /// [`AskError::Unknown`] when the answer does not come in time.
    pub async fn ask(&self, id: FeederId, request: RequestId, op: Op) -> Result<Answer, AskError> {
        let (commands, link) = {
            let handles = self.handles();
            let handle = handles.get(&id).ok_or(AskError::NotRunning)?;
            (handle.commands.clone(), handle.link())
        };
        send(
            &commands,
            link,
            self.shared.config.timing.answer,
            request,
            op,
        )
        .await
    }

    /// Wait for the feeder's long-operation turn. The turn lasts until the
    /// [`Operation`] is dropped.
    pub async fn operation(&self, id: FeederId) -> Result<Operation, AskError> {
        let long = {
            let handles = self.handles();
            let handle = handles.get(&id).ok_or(AskError::NotRunning)?;
            handle.long.clone()
        };
        long.operation(
            self.shared.config.timing.answer,
            Arc::clone(&self.shared.random),
        )
        .await
    }
}

impl LongLane {
    /// Wait for the turn, and hold it in an [`Operation`] whose steps wait
    /// `answer` each.
    pub(crate) async fn operation(
        self,
        answer: Duration,
        random: Arc<dyn Random>,
    ) -> Result<Operation, AskError> {
        let turn = self
            .turn
            .acquire_owned()
            .await
            .map_err(|_| AskError::NotRunning)?;
        Ok(Operation {
            steps: self.steps,
            link: self.link,
            answer,
            random,
            _turn: turn,
        })
    }
}

/// Queue a command and wait for its answer until `answer` has passed.
async fn send(
    commands: &mpsc::Sender<Command>,
    link: Link,
    answer: Duration,
    request: RequestId,
    op: Op,
) -> Result<Answer, AskError> {
    if link != Link::Online {
        return Err(AskError::Offline);
    }

    let deadline = Instant::now() + answer;
    let (reply, answered) = oneshot::channel();
    let command = Command {
        id: request,
        op,
        reply,
    };
    match timeout_at(deadline, commands.send(command)).await {
        Ok(Ok(())) => {}
        // Never handed over, so never sent.
        Ok(Err(_)) | Err(_) => return Err(AskError::Offline),
    }
    match timeout_at(deadline, answered).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => Err(AskError::Unknown),
    }
}

impl Command {
    /// Whether the caller stopped waiting: the command is then dropped
    /// unsent, so the caller's `Unknown` never turns into a late write.
    pub(crate) fn left(&self) -> bool {
        self.reply.is_closed()
    }

    /// Hand the caller its result; a caller that left hears nothing.
    pub(crate) fn reply(self, result: Result<Answer, AskError>) {
        let _ = self.reply.send(result);
    }

    /// Turn the command away unsent: the task has no session.
    pub(crate) fn offline(self) {
        self.reply(Err(AskError::Offline));
    }
}
