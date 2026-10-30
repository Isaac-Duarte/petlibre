//! The owner's update-signing key: create one, enroll it on a feeder with
//! a Dispense press there, and list the keys a feeder trusts this way.
//!
//! A feeder admits an update signed by a key built into its image or by
//! one an owner enrolled, so an enrolled key lets the owner sign their own
//! builds (`wellfed-build sign --key`) for that feeder. The bench key is
//! the same file, so it enrolls too: a feeder whose image embeds no key
//! then still takes the bench's signed updates. What is sent and what the
//! answers mean is `wellfed_client::update_key`'s; this is the IO.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use wellfed_client::update_key::{self, Enroll, EnrollError, Enrolled, Next};
use wellfed_formats::bench::prompt::{DispensePurpose, Prompt};
use wellfed_formats::credentials::KeyScope;
use wellfed_formats::hex;
use wellfed_model::{Answer, Client, ClientRole, Op};
use wellfed_signing::Key;

use crate::ask::Asker;
use crate::device::Connection;
use crate::{Target, connect};

/// How long after the enrollment goes out the press is asked for. The
/// feeder takes a Dispense press as the confirmation only once the request
/// waits for it, and nothing tells the run when that is: this is a timing
/// assumption (a LAN request arrives within milliseconds; the window is
/// 60 s), not a signal. If it fails, the press comes first: on a consumer
/// image's panel, locked since its boot, that press does nothing, the
/// window closes `Unconfirmed`, and the run stops with no second request.
const PRESS_AFTER: Duration = Duration::from_secs(1);

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
    }
}

/// Enroll the key in `file` over `connection`, the request a press at the
/// feeder answers sent at most once.
async fn enroll(connection: &mut Connection, file: &Path, label: &str) -> Result<()> {
    let key = Key::load(file).with_context(|| format!("load {}", file.display()))?;
    let id = hex::encode(&key.id());
    match enrolled(connection, &key, label, &mut Asker::terminal_notices()).await? {
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

/// Run one enrollment of `key` over `connection` to its end, `asker`
/// announcing the press: the outer error is a session refused before
/// anything was asked, the inner one how the feeder ended it. The
/// enrollment goes at most once.
pub(crate) async fn enrolled<W: Write>(
    connection: &mut Connection,
    key: &Key,
    label: &str,
    asker: &mut Asker<W>,
) -> Result<Result<Enrolled, EnrollError>> {
    let id = hex::encode(&key.id());
    let mut enroll = match Enroll::new(
        key.public(),
        label,
        connection.role(),
        connection.capabilities(),
    ) {
        Ok(enroll) => enroll,
        Err(error) => return Ok(Err(error)),
    };

    loop {
        let answered = match enroll.poll() {
            Next::Send(op) => connection.ask(&op).await,
            Next::Press(op) => {
                // A prompt answered earlier is read before the request goes,
                // so no wait for it spends the feeder's window.
                asker.settle().await?;
                pressed(connection, &op, &id, asker).await
            }
            Next::Done(enrolled) => return Ok(Ok(enrolled)),
            Next::Failed(error) => return Ok(Err(error)),
        };
        match answered {
            Ok(answer) => enroll.answer(&answer),
            Err(error) => {
                eprintln!("No answer: {error:#}");
                enroll.unanswered();
            }
        }
    }
}

/// Send the enrollment `op` and, once it has had [`PRESS_AFTER`] to reach
/// the feeder unanswered, announce the Dispense press; its answer. A
/// refusal that comes back at once (busy, say) asks for no press. The
/// announcement never stops the request: one already sent is judged by its
/// answer alone, so a failed announcement is only reported.
async fn pressed<W: Write>(
    connection: &mut Connection,
    op: &Op,
    id: &str,
    asker: &mut Asker<W>,
) -> Result<Answer> {
    let asked = connection.ask(op);
    tokio::pin!(asked);
    tokio::select! {
        answer = &mut asked => answer,
        () = tokio::time::sleep(PRESS_AFTER) => {
            let prompt = Prompt::PressDispense {
                purpose: DispensePurpose::EnrollUpdateKey,
            };
            let terminal = format!(
                "Press Dispense on the feeder within 60 s to enroll update key {id}."
            );
            if let Err(error) = asker.announce(prompt, &terminal).await {
                eprintln!("Could not ask for the Dispense press: {error}");
            }
            asked.await
        }
    }
}

async fn show(connection: &mut Connection) -> Result<()> {
    let signers: Vec<Client> = clients(connection)
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

pub(crate) async fn clients(connection: &mut Connection) -> Result<Vec<Client>> {
    match connection.ask(&Op::ListClients).await? {
        Answer::Clients(clients) => Ok(clients),
        Answer::Refused(refusal) => bail!("the feeder refused to list its clients: {refusal:?}"),
        other => bail!("the feeder answered {other:?} to a client list request"),
    }
}

/// The CLI's own path against a simulated feeder: the bench key's file
/// enrolls as an owner's does.
#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wellfed_feeder_sim::console;
    use wellfed_feeder_sim::harness::Threaded;
    use wellfed_session::noise::Keypair;

    use super::*;

    const OWNER: [u8; 32] = [0x21; 32];

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_bench_key_file_enrolls_with_a_press_and_then_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ota-key.json");
        wellfed_signing::keygen(&file, KeyScope::Bench).unwrap();
        let public = Key::load(&file).unwrap().public();

        let feeder = Threaded::start(*Keypair::from_secret(OWNER).public());
        let keypair = Keypair::from_secret(OWNER);
        let direct =
            wellfed_connect::Connection::open(&keypair, feeder.identity, feeder.address, [9; 32])
                .await
                .unwrap();
        let mut connection = Connection::from(direct);
        for control in console::parse("hold lock").unwrap() {
            feeder.control(control);
        }

        let hands = async {
            // After the press is asked for, as a person or the bench would.
            tokio::time::sleep(PRESS_AFTER + Duration::from_millis(500)).await;
            for control in console::parse("press dispense").unwrap() {
                feeder.control(control);
            }
        };
        let mut asker = crate::ask::scripted(crate::ask::Prompts::Lines, &[]);
        let key = Key::load(&file).unwrap();
        let (enrolled, ()) =
            tokio::join!(enrolled(&mut connection, &key, "bench", &mut asker), hands);
        assert_eq!(enrolled.unwrap(), Ok(Enrolled::Now));
        // The press was asked for as a prompt line once the request was
        // out, and its answer left for the run to read before its next.
        assert_eq!(
            crate::ask::written(&asker),
            [r#"{"kind":"press-dispense","purpose":"enroll-update-key"}"#]
        );
        let listed = clients(&mut connection).await.unwrap();
        let entry = listed.iter().find(|c| c.key.0 == public).unwrap();
        assert_eq!(entry.role, ClientRole::UpdateSigner);

        // Again: the list says it is enrolled, so nothing else goes out
        // (an enrollment now would wait its 60 s window for a press).
        let again = tokio::time::timeout(
            Duration::from_secs(5),
            enroll(&mut connection, &file, "bench"),
        )
        .await;
        assert!(matches!(again, Ok(Ok(()))), "{again:?}");
    }
}
