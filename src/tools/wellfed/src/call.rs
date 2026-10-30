//! The meal call over the device API: show the stored clip, upload a new
//! one from a WAV file, or delete it (docs/firmware/schedules.md#meal-call).
//!
//! The file is converted here (`wellfed_client::call`), so the feeder only
//! stores and plays its own stream. An upload erases the clip the feeder
//! held, sends the stream a chunk at a time and waits while the feeder
//! reads it back against its CRC. Nothing here feeds; a plan plays the
//! call only once its `--call` names how many times.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use wellfed_client::call::Clip;
use wellfed_connect::call::upload;
use wellfed_model::Op;

use crate::{Target, ask_and_print, connect, print_json};

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
            let view = upload(&clip, async |op: &Op| connection.ask(op).await).await??;
            print_json(&view)
        }
    }
}
