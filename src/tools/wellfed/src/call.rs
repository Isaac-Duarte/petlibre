//! The meal call over the device API: show the stored clip, upload a new
//! one from a WAV file, or delete it (docs/firmware/schedules.md#meal-call).
//!
//! The file is converted here (`wellfed_client::call`), so the feeder only
//! stores and plays its own stream. An upload erases the clip the feeder
//! held, sends the stream a chunk at a time and waits while the feeder
//! reads it back against its CRC. Nothing here feeds; a plan plays the
//! call only once its `--call` names how many times.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::call::{Clip, Sent};
use wellfed_model::{Answer, CallView, Op, Refusal};

use crate::device::Connection;
use crate::{Target, ask_and_print, connect, print_answer, print_json};

#[derive(Args)]
pub(crate) struct CallArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// The stored clip, if any.
    Show(Target),
    /// Convert a WAV recording and upload it as the meal call, in place of
    /// the clip the feeder holds. At most about 8 s once trimmed.
    Upload {
        #[command(flatten)]
        target: Target,
        /// A WAV file: integer PCM or 32-bit float, any rate and channels.
        file: PathBuf,
    },
    /// Erase the stored clip; plans then play no call.
    Delete(Target),
}

/// The pause before a chunk the feeder was still writing for is sent
/// again.
const RETRY: Duration = Duration::from_millis(50);
/// How long an upload waits on chunks the feeder keeps refusing `Busy`:
/// past any feed, during which no store is written.
const BUSY_WAIT: Duration = Duration::from_secs(180);

pub(crate) async fn command(args: CallArgs) -> Result<()> {
    match args.action {
        Action::Show(target) => ask_and_print(&target, &Op::GetCall).await,
        Action::Delete(target) => ask_and_print(&target, &Op::DeleteCall).await,
        Action::Upload { target, file } => {
            let bytes = std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            let clip = Clip::from_wav(&bytes)
                .with_context(|| format!("convert {} to the meal call", file.display()))?;
            eprintln!(
                "{}: {} ms a play, {} bytes.",
                file.display(),
                clip.ms(),
                clip.bytes()
            );
            let (_, _, mut connection) = connect(&target).await?;
            let view = upload(&mut connection, &clip).await?;
            print_json(&view)
        }
    }
}

/// Upload `clip` whole: the feeder's view once it has stored it.
async fn upload(connection: &mut Connection, clip: &Clip) -> Result<CallView> {
    let answer = connection.ask(&clip.start()).await?;
    if !matches!(answer, Answer::Done { .. }) {
        print_answer(&answer)?;
        bail!("the feeder did not start the upload");
    }

    let mut offset = 0;
    let mut busy_since = None;
    while let Some(op) = clip.chunk(offset) {
        let answer = connection.ask(&op).await?;
        match clip.after_chunk(offset, &answer)? {
            Sent::Next(next) => {
                offset = next;
                busy_since = None;
            }
            Sent::Retry => {
                let since = *busy_since.get_or_insert_with(Instant::now);
                if since.elapsed() > BUSY_WAIT {
                    bail!("the feeder stayed busy for {} s", BUSY_WAIT.as_secs());
                }
                tokio::time::sleep(RETRY).await;
            }
            Sent::Ask => offset = clip.resume(&call(connection).await?)?,
        }
    }

    // The last chunk is in: the feeder reads the stream back, then stores
    // it, on its writer's next turns.
    let deadline = Instant::now() + BUSY_WAIT;
    loop {
        let view = call(connection).await?;
        if clip.settled(&view)? {
            return Ok(view);
        }
        if Instant::now() > deadline {
            bail!("the feeder did not store the clip in time");
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// The feeder's view of the call.
async fn call(connection: &mut Connection) -> Result<CallView> {
    match connection.ask(&Op::GetCall).await? {
        Answer::Call(view) => Ok(view),
        Answer::Refused(Refusal::Unavailable) => bail!("this feeder keeps no meal call"),
        answer => {
            print_answer(&answer)?;
            bail!("the feeder did not return its meal call");
        }
    }
}
