//! The owner's update-signing key: create one, enroll it on a feeder with
//! a Dispense press there, and list the keys a feeder trusts this way.
//!
//! A feeder admits an update signed by a key built into its image or by
//! one an owner enrolled, so an enrolled key lets the owner sign their own
//! builds (`wellfed-build sign --key`) for that feeder. The bench key is
//! the same file, so it enrolls too: a feeder whose image embeds no key
//! then still takes the bench's signed updates. What is sent and what the
//! answers mean is `wellfed_client::update_key`'s; this is the IO.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::update_key::{Enroll, LABEL_MAX, Listed};
use wellfed_formats::credentials::KeyScope;
use wellfed_formats::hex;
use wellfed_model::{Answer, Client, ClientRole, Op};
use wellfed_signing::Key;

use crate::device::Connection;
use crate::{Target, connect};

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

fn label(text: &str) -> Result<String, String> {
    if text.is_empty() || text.len() > LABEL_MAX || text.contains('\0') {
        return Err(format!("1-{LABEL_MAX} bytes"));
    }
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
        Action::Enroll(args) => enroll(args).await,
        Action::Show(target) => {
            let (_, _, mut connection) = connect(&target).await?;
            let signers: Vec<Client> = clients(&mut connection)
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
    }
}

async fn enroll(args: EnrollArgs) -> Result<()> {
    let key = Key::load(&args.key).with_context(|| format!("load {}", args.key.display()))?;
    let enroll = Enroll::new(key.public(), &args.label)?;
    let id = hex::encode(&key.id());

    let (_, _, mut connection) = connect(&args.target).await?;
    Enroll::session(connection.role(), connection.capabilities())?;
    if enroll.listed(&clients(&mut connection).await?)? == Listed::Enrolled {
        println!("The feeder already trusts update key {id}; nothing sent.");
        return Ok(());
    }

    eprintln!(
        "Press Dispense on the feeder within 60 s to enroll update key {id} (unlock the panel \
         first if it is locked)."
    );
    let answer = connection
        .ask(&enroll.request())
        .await
        .context("no answer came; the feeder may still be waiting for the press")?;
    enroll.answer(&answer)?;
    println!("Enrolled update key {id} as {:?}.", args.label);
    Ok(())
}

async fn clients(connection: &mut Connection) -> Result<Vec<Client>> {
    match connection.ask(&Op::ListClients).await? {
        Answer::Clients(clients) => Ok(clients),
        Answer::Refused(refusal) => bail!("the feeder refused to list its clients: {refusal:?}"),
        other => bail!("the feeder answered {other:?} to a client list request"),
    }
}
