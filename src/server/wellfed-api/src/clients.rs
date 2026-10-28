//! A feeder's clients and the phones' own keys on it, and what an owner
//! administers on a feeder through the server: Wi-Fi, a restart, a tune
//! and removing it.
//!
//! An owner's or member's phone keeps a key of its own on every feeder, for
//! the LAN when the server is down. The server makes the invite it joins
//! with and records whose phone it is, so deleting, demoting or putting an
//! expiry on that account revokes the key on every feeder. A client the
//! server did not invite is listed as unowned, for an owner to keep or
//! revoke.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use wellfed_model::{ClientRole, Hex};

use crate::{Feeder, FeederId, FeederRefusal, TokenId, UserId};

/// One client the feeder has enrolled, and whose it is to the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FeederClient {
    /// Its public key.
    #[schema(value_type = String)]
    pub key: Hex<32>,
    pub label: String,
    pub role: ClientRole,
    pub owner: ClientOwner,
}

/// Whose a feeder's client is, as the server knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientOwner {
    /// The server's own key.
    Server,
    /// A phone the server invited for this account, through this token.
    Phone {
        user: UserId,
        token: Option<TokenId>,
        /// Set once the key is to be revoked and until it is.
        revoking: Option<Revoking>,
    },
    /// A client the server did not invite, which an owner chose to keep
    /// (the owner's CLI, say).
    Kept,
    /// A client the server did not invite: an owner keeps or revokes it.
    Unowned,
}

/// An invite for the caller's own phone, which joins the feeder with it on
/// the LAN. The phone sends the public key it will join with, so the
/// server knows the client as this account's once it enrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewPhoneInvite {
    #[schema(value_type = String)]
    pub key: Hex<32>,
}

/// A single-use invite for the caller's phone. The text holds the secret
/// the phone's handshake needs, so it is shown once and never stored.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneInvite {
    /// `wellfed-invite:1:…`, redeemed by the phone at the feeder.
    pub invite: String,
    /// The role the phone joins with: its account's.
    pub role: ClientRole,
    /// How long the feeder holds the invite.
    pub seconds: u16,
    /// Where the server last reached the feeder.
    pub address: Option<String>,
}

impl fmt::Debug for PhoneInvite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhoneInvite")
            .field("role", &self.role)
            .field("seconds", &self.seconds)
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// A phone's key on a feeder, as the server recorded it when it made the
/// phone's invite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PhoneKey {
    pub feeder: FeederId,
    #[schema(value_type = String)]
    pub key: Hex<32>,
    pub user: UserId,
    pub token: Option<TokenId>,
    /// The role it was invited with.
    pub role: ClientRole,
    pub invited: Timestamp,
    /// Until when an invite made for it may still be redeemed: the feeder
    /// does not bind an invite to a key, so a revoke that finds no such
    /// client before then is tried again.
    pub invite_until: Timestamp,
    /// Set once the key is to be revoked and until it is.
    pub revoking: Option<Revoking>,
}

/// A phone key the server is revoking: tried at once, and again each time
/// the feeder reconnects, until the feeder has dropped it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Revoking {
    pub reason: RevokeReason,
    pub since: Timestamp,
    /// What the last attempt came to; `None` before the first.
    pub last: Option<RevokeAttempt>,
}

/// Why a phone's key is revoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevokeReason {
    /// Its account was deleted.
    Deleted,
    /// Its account lost a role: a viewer holds no key, and a member's
    /// phone no owner's.
    Demoted,
    /// Its account was given an expiry, and an expiring account holds no
    /// key.
    Expiring,
    /// The token it was invited through was revoked.
    TokenRevoked,
    /// Its token was given an invite for another key: one key per token.
    Replaced,
}

impl RevokeReason {
    /// The stored spelling.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Demoted => "demoted",
            Self::Expiring => "expiring",
            Self::TokenRevoked => "token_revoked",
            Self::Replaced => "replaced",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [
            Self::Deleted,
            Self::Demoted,
            Self::Expiring,
            Self::TokenRevoked,
            Self::Replaced,
        ]
        .into_iter()
        .find(|reason| reason.code() == code)
    }
}

/// Why an attempt to revoke a key, or the server's own, did not land.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum RevokeAttempt {
    /// The server had no session with the feeder; nothing was sent.
    Offline,
    /// No answer came in time: the feeder may have dropped the key.
    Unknown,
    /// The feeder refused: `forbidden` when the server's key is not an
    /// owner's.
    Refused { refusal: FeederRefusal },
    /// The feeder holds no such client yet, but the key's invite may still
    /// be redeemed until `until`: tried again when a client joins, at
    /// `until`, and on each reconnect.
    InviteLive { until: Timestamp },
}

/// The feeder's Wi-Fi credentials for its next join, or `null` to clear
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WifiChange {
    pub wifi: Option<WifiCredentials>,
}

/// A network the feeder joins.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WifiCredentials {
    /// 1-32 bytes.
    pub ssid: String,
    /// 8-63 printable ASCII characters (WPA2-PSK). Never stored, logged or
    /// echoed.
    pub psk: String,
}

impl fmt::Debug for WifiCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WifiCredentials")
            .field("ssid", &self.ssid)
            .finish_non_exhaustive()
    }
}

/// A tune to play now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlayRequest {
    pub tune: TuneName,
}

/// The feeder's tunes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TuneName {
    Chime,
    Meal,
    Coin,
    Alert,
    Test,
    Twinkle,
}

/// A feeder the server no longer mirrors. Its history, samples and journal
/// stay readable; adopting it again continues them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Removal {
    pub feeder: Feeder,
    /// Whether the feeder dropped the server's own key.
    pub key: KeyRemoval,
    /// Phone keys still owed a revoke on the feeder, which removing it
    /// tried first. Adopting it again resumes revoking them; until then
    /// its owner revokes them on the feeder (`wellfed revoke`).
    pub phones_owed: Vec<PhoneKey>,
}

/// What came of revoking the server's own key on a feeder being removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum KeyRemoval {
    /// The feeder dropped it.
    Revoked,
    /// It could not be revoked: the feeder still holds the key, and its
    /// owner revokes it on the feeder (`wellfed revoke`).
    Failed { attempt: RevokeAttempt },
}
