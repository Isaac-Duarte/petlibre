//! Uploading a meal call over any open session: the CLI's, or the app's,
//! whose requests go through its session loop. The clip and every rule about
//! it are `wellfed_client::call`'s; this is the waiting between its chunks.
//!
//! A chunk is idempotent by its offset and a `Busy` one is sent again, but a
//! request that was never answered ends the upload through the caller's
//! `ask` rather than being sent again here: starting over is the owner's call.

use std::time::Duration;

use tokio::time::{Instant, sleep};
use wellfed_client::call::{Clip, Sent, UploadError};
use wellfed_model::{Answer, CallView, Op, Refusal};

/// The pause before a chunk the feeder was still writing for is sent again.
const RETRY: Duration = Duration::from_millis(50);
/// How long an upload waits on chunks the feeder keeps refusing `Busy`: past
/// any feed, during which no store is written.
const BUSY_WAIT: Duration = Duration::from_secs(180);

/// Why an upload stopped. The clip the feeder held is gone once it started.
#[derive(Debug, thiserror::Error)]
pub enum Failed {
    #[error(transparent)]
    Upload(#[from] UploadError),
    /// The feeder would not start the upload, or show its call.
    #[error("the feeder refused: {0:?}")]
    Refused(Refusal),
    #[error("the feeder answered something else: {0:?}")]
    Unexpected(Box<Answer>),
    #[error("the feeder stayed busy for {} s", BUSY_WAIT.as_secs())]
    Busy,
    #[error("the feeder did not store the clip in time")]
    NotStored,
}

/// Upload `clip` whole, asking through `ask`: the feeder's view once it has
/// stored it, or why it stopped. An `ask` that fails ends it, as the outer
/// error.
pub async fn upload<E>(
    clip: &Clip,
    mut ask: impl AsyncFnMut(&Op) -> Result<Answer, E>,
) -> Result<Result<CallView, Failed>, E> {
    match ask(&clip.start()).await? {
        Answer::Done { .. } => {}
        Answer::Refused(refusal) => return Ok(Err(Failed::Refused(refusal))),
        other => return Ok(Err(Failed::Unexpected(Box::new(other)))),
    }

    let mut offset = 0;
    let mut busy_since = None;
    while let Some(op) = clip.chunk(offset) {
        let answer = ask(&op).await?;
        let sent = match clip.after_chunk(offset, &answer) {
            Ok(sent) => sent,
            Err(error) => return Ok(Err(error.into())),
        };
        match sent {
            Sent::Next(next) => {
                offset = next;
                busy_since = None;
            }
            Sent::Retry => {
                let since = *busy_since.get_or_insert_with(Instant::now);
                if since.elapsed() > BUSY_WAIT {
                    return Ok(Err(Failed::Busy));
                }
                sleep(RETRY).await;
            }
            Sent::Ask => match view(&mut ask)
                .await?
                .and_then(|view| Ok(clip.resume(&view)?))
            {
                Ok(next) => offset = next,
                Err(failed) => return Ok(Err(failed)),
            },
        }
    }

    // The last chunk is in: the feeder reads the stream back, then stores
    // it, on its writer's next turns.
    let deadline = Instant::now() + BUSY_WAIT;
    loop {
        let view = match view(&mut ask).await? {
            Ok(view) => view,
            Err(failed) => return Ok(Err(failed)),
        };
        match clip.settled(&view) {
            Ok(true) => return Ok(Ok(view)),
            Ok(false) => {}
            Err(error) => return Ok(Err(error.into())),
        }
        if Instant::now() > deadline {
            return Ok(Err(Failed::NotStored));
        }
        sleep(RETRY).await;
    }
}

/// The feeder's view of the call.
async fn view<E>(
    ask: &mut impl AsyncFnMut(&Op) -> Result<Answer, E>,
) -> Result<Result<CallView, Failed>, E> {
    Ok(match ask(&Op::GetCall).await? {
        Answer::Call(view) => Ok(view),
        Answer::Refused(refusal) => Err(Failed::Refused(refusal)),
        other => Err(Failed::Unexpected(Box::new(other))),
    })
}
