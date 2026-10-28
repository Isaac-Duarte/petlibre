//! A feeder's clients and its restart, over the server: `clients` lists
//! whose each is, `revoke` drops one through the journal, `reboot`
//! restarts the feeder. All three are an owner's, and the feeder runs a
//! revoke or a restart only for a server adopted with an owner's invite.

use anyhow::{Result, bail};
use clap::Args;
use uuid::Uuid;
use wellfed_server_client::Client;
use wellfed_server_client::api::{ClientOwner, FeederClient};

use super::feeders::Which;
use super::{Output, Settled, Written, text, write};

#[derive(Args)]
pub(crate) struct RevokeArgs {
    #[command(flatten)]
    which: Which,
    /// The client's key, or at least its first 8 hex digits, as `wellfed
    /// server clients` printed it.
    client: String,
}

pub(super) async fn clients(client: &Client, which: &Which, out: Output) -> Result<Settled> {
    let id = which.id(client).await?;
    let clients = client.clients(id).await?;
    out.show(&clients, |clients| {
        clients.iter().map(line).collect::<Vec<_>>().join("\n")
    })?;
    Ok(Settled::Known)
}

/// Revoke one client, sent once under a fresh key. The server's own key is
/// refused: removing the feeder revokes it.
pub(super) async fn revoke(client: &Client, args: RevokeArgs, out: Output) -> Result<Settled> {
    let prefix = args.client.to_lowercase();
    if prefix.len() < 8 {
        bail!("give at least 8 hex digits of the client's key");
    }
    let id = args.which.id(client).await?;
    let clients = client.clients(id).await?;
    let matching: Vec<&FeederClient> = clients
        .iter()
        .filter(|listed| listed.key.to_string().starts_with(&prefix))
        .collect();
    let [chosen] = matching.as_slice() else {
        bail!("{} clients match {}", matching.len(), args.client);
    };

    let key = Uuid::new_v4();
    let sent = client.revoke_client(id, chosen.key, key);
    let Written::Answered(()) = write(out, key, sent).await? else {
        return Ok(Settled::Unknown);
    };
    out.text(format_args!("Revoked {}.", chosen.key));
    Ok(Settled::Known)
}

/// Restart the feeder, sent once: one whose answer is lost is never sent
/// again, and the boot count in its status says whether it ran.
pub(super) async fn reboot(client: &Client, which: &Which, out: Output) -> Result<Settled> {
    let id = which.id(client).await?;

    let key = Uuid::new_v4();
    let Written::Answered(()) = write(out, key, client.reboot(id, key)).await? else {
        return Ok(Settled::Unknown);
    };
    out.text("The feeder restarts.");
    Ok(Settled::Known)
}

/// One client as a line: its key, role, label and whose it is.
fn line(client: &FeederClient) -> String {
    let owner = match &client.owner {
        ClientOwner::Server => "the server".to_owned(),
        ClientOwner::Phone { user, revoking, .. } => match revoking {
            Some(revoking) => format!(
                "phone of {user}, being revoked ({})",
                text::word(&revoking.reason)
            ),
            None => format!("phone of {user}"),
        },
        ClientOwner::Kept => "kept".to_owned(),
        ClientOwner::Unowned => "unowned".to_owned(),
    };
    format!(
        "{}  {:<6}  {}  {owner}",
        client.key,
        text::word(&client.role),
        client.label
    )
}
