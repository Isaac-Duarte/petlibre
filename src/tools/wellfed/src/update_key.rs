//! The owner's update-signing key: create one, enroll it on a feeder with
//! a Dispense press there, and list the keys a feeder trusts this way.
//!
//! A feeder admits an update signed by a key built into its image or by
//! one an owner enrolled, so an enrolled key lets the owner sign their own
//! builds (`wellfed-build sign --key`) for that feeder. The bench key is
//! the same file, so it enrolls too: a feeder whose image embeds no key
//! then still takes the bench's signed updates. The enrollment itself is
//! `wellfed_install::update_key`'s, the one the install runs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::update_key::{self, EnrollError, Enrolled};
use wellfed_formats::credentials::KeyScope;
use wellfed_formats::hex;
use wellfed_model::{Client, ClientRole, Op};
use wellfed_signing::Key;

use crate::ask::Asker;
use crate::device::Connection;
use crate::report::Printer;
use crate::{Target, ask_and_print, connect};

#[derive(Args)]
pub(crate) struct UpdateKeyArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Create an owner's update-signing key; never replaces a file.
    New {
        /// Where to write it. It holds the private key: keep it private.
        #[arg(long)]
        output: PathBuf,
    },
    /// Enroll a key on the feeder: someone presses Dispense there within
    /// 60 s to confirm it. Owners only.
    Enroll(EnrollArgs),
    /// The update keys the feeder has enrolled.
    Show(Target),
    /// Send a key statement a built-in or the release key signed offline: a
    /// key the feeder then trusts or stops trusting, with no press there.
    Statement {
        #[command(flatten)]
        target: Target,
        /// The signed statement.
        file: PathBuf,
    },
}

#[derive(Args)]
pub(crate) struct EnrollArgs {
    #[command(flatten)]
    target: Target,
    /// The key file: one `update-key new` wrote, or the bench key.
    #[arg(long)]
    key: PathBuf,
    /// The name the feeder lists it under.
    #[arg(long, default_value = "update-key", value_parser = label)]
    label: String,
}

fn label(text: &str) -> Result<String, EnrollError> {
    update_key::label(text)?;
    Ok(text.to_owned())
}

pub(crate) async fn command(args: UpdateKeyArgs) -> Result<()> {
    match args.action {
        Action::New { output } => {
            let public = wellfed_signing::keygen(&output, KeyScope::Owner)
                .with_context(|| format!("write {}", output.display()))?;
            println!(
                "Wrote update key {}; public key {public}.",
                output.display()
            );
            eprintln!("Enroll it on each feeder with `wellfed update-key enroll --key`.");
            Ok(())
        }
        Action::Enroll(args) => {
            let (_, _, mut connection) = connect(&args.target).await?;
            enroll(&mut connection, &args.key, &args.label).await
        }
        Action::Show(target) => {
            let (_, _, mut connection) = connect(&target).await?;
            show(&mut connection).await
        }
        Action::Statement { target, file } => {
            let statement =
                std::fs::read(&file).with_context(|| format!("read {}", file.display()))?;
            ask_and_print(&target, &Op::UpdateKeyStatement { statement }).await
        }
    }
}

/// Enroll the key in `file` over `connection`, the request a press at the
/// feeder answers sent at most once.
async fn enroll(connection: &mut Connection, file: &Path, label: &str) -> Result<()> {
    let key = Key::load(file).with_context(|| format!("load {}", file.display()))?;
    let id = hex::encode(&key.id());
    let outcome = wellfed_install::update_key::enroll(
        connection,
        key.public(),
        key.id(),
        label,
        &mut Asker::terminal_notices(),
        &Printer,
    )
    .await?;
    match outcome {
        Ok(Enrolled::Now) => println!("Enrolled update key {id} as {label:?}."),
        Ok(Enrolled::Already) => {
            println!("The feeder already trusts update key {id}; nothing sent.")
        }
        Err(EnrollError::Uncertain) => bail!(
            "no answer came to the enrollment of update key {id}: a press may have enrolled \
             it all the same. Check with `wellfed update-key show`; the request is not sent \
             again"
        ),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn show(connection: &mut Connection) -> Result<()> {
    let signers: Vec<Client> = wellfed_install::update_key::clients(connection)
        .await?
        .into_iter()
        .filter(|client| client.role == ClientRole::UpdateSigner)
        .collect();
    if signers.is_empty() {
        eprintln!("The feeder has no enrolled update keys.");
    }
    for signer in &signers {
        let id = wellfed_client::release::key_id(&signer.key.0);
        println!(
            "{}  key id {}  {}",
            signer.key,
            hex::encode(&id),
            signer.label
        );
    }
    Ok(())
}
