//! Joining a feeder with an invite someone made on theirs. Rust reads the
//! invite's text and runs the exchange, so the app never holds its format.

use flutter_rust_bridge::frb;
use wellfed_client::api::Invite;
use wellfed_model::Joined;

use crate::api::error::{BridgeError, ErrorKind};
use crate::api::feeder::FeederRefusal;
use crate::api::keys::{ClientKey, random};
use crate::api::model::ClientRole;
use crate::requests::refusal_of;

/// The identity key of the feeder `text` invites to, or `None` when `text` is
/// not an invite.
#[frb(sync)]
pub fn invite_feeder(text: &str) -> Option<Vec<u8>> {
    Invite::parse(text.trim())
        .ok()
        .map(|invite| invite.feeder.to_vec())
}

/// What the feeder said to an invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAnswer {
    /// This phone is enrolled with this role and opens sessions now.
    Joined {
        role: ClientRole,
    },
    Refused {
        refusal: FeederRefusal,
    },
}

/// Redeem `invite` at `address`, listed as `label`. An invite works once, so
/// this runs once: a timeout leaves it unknown whether it was spent, and only
/// the screen can say so.
pub async fn join_feeder(
    key: &ClientKey,
    invite: String,
    label: String,
    address: String,
) -> Result<JoinAnswer, BridgeError> {
    let invite =
        Invite::parse(invite.trim()).map_err(|error| BridgeError::new(ErrorKind::Invite, error))?;
    let address = wellfed_connect::parse_address(&address)?;

    let joined = wellfed_connect::join(key.keypair(), invite, &label, address, random()?)
        .await
        .map_err(|error| match error {
            // A reset is not the feeder's quiet close of a bad invite: the hello
            // went out, and it may have been spent.
            wellfed_connect::Error::Io(_) => BridgeError::new(ErrorKind::Interrupted, error),
            error => error.into(),
        })?;
    Ok(match joined {
        Joined::Enrolled { role } => JoinAnswer::Joined { role },
        Joined::Refused(refusal) => JoinAnswer::Refused {
            refusal: refusal_of(refusal),
        },
    })
}
