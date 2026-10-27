//! The command surface: [`Mirror::ask`] for a short command, and an
//! [`Operation`] for a long one that sends its steps one at a time.
//!
//! Both go through the feeder task's one queue, which runs a command at a
//! time, so a long operation's steps take turns with everything else: a
//! status read or a write waits at most one step. Only one long operation
//! runs per feeder.

use std::sync::Arc;

use wellfed_api::{Connection as Link, FeederId};
use wellfed_connect::Connection;
use wellfed_model::{Answer, Op};
use tokio::sync::{OwnedSemaphorePermit, mpsc, oneshot, watch};
use tokio::time::{Duration, Instant, timeout_at};

use crate::session::End;
use crate::{AskError, Mirror, RequestId};

/// One request for the feeder, and where its answer goes.
pub(crate) struct Command {
    id: RequestId,
    op: Op,
    reply: oneshot::Sender<Result<Answer, AskError>>,
}

/// A long operation's turn on a feeder: its steps go out one at a time,
/// and other commands run between them. Dropping it ends the operation.
#[derive(Debug)]
pub struct Operation {
    commands: mpsc::Sender<Command>,
    link: watch::Receiver<Link>,
    answer: Duration,
    _turn: OwnedSemaphorePermit,
}

impl Operation {
    /// Send one step and wait for its answer, as [`Mirror::ask`] does.
    pub async fn step(&mut self, request: RequestId, op: Op) -> Result<Answer, AskError> {
        let link = *self.link.borrow();
        send(&self.commands, link, self.answer, request, op).await
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
        let (commands, link, turn) = {
            let handles = self.handles();
            let handle = handles.get(&id).ok_or(AskError::NotRunning)?;
            (
                handle.commands.clone(),
                handle.link.clone(),
                Arc::clone(&handle.long),
            )
        };
        let turn = turn
            .acquire_owned()
            .await
            .map_err(|_| AskError::NotRunning)?;
        Ok(Operation {
            commands,
            link,
            answer: self.shared.config.timing.answer,
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
    /// Turn the command away unsent: the task has no session.
    pub(crate) fn offline(self) {
        let _ = self.reply.send(Err(AskError::Offline));
    }

    /// Run the command on the session. A command whose answer does not
    /// arrive is unknown, not failed: the feeder may have run it. One whose
    /// caller stopped waiting is dropped unsent, so the caller's `Unknown`
    /// never turns into a late write.
    pub(crate) async fn run(self, connection: &mut Connection) -> Result<(), End> {
        if self.reply.is_closed() {
            return Ok(());
        }
        match connection.ask(self.id, &self.op).await {
            Ok(answer) => {
                let _ = self.reply.send(Ok(answer));
                Ok(())
            }
            Err(wellfed_connect::Error::Timeout) => {
                let _ = self.reply.send(Err(AskError::Unknown));
                Ok(())
            }
            // Refused before anything was sent: the session is untouched.
            Err(wellfed_connect::Error::ReservedId | wellfed_connect::Error::Encode) => {
                let _ = self.reply.send(Err(AskError::Unsendable));
                Ok(())
            }
            Err(error) => {
                let _ = self.reply.send(Err(AskError::Unknown));
                Err(End::Lost(error))
            }
        }
    }
}
