//! The bench server's `/device/*` routes: the one owner session the server
//! holds, lent to a local client (the `wellfed` CLI) one request at a time.
//!
//! The server never resends a request. One that went out and got no answer
//! comes back [`Asked::Uncertain`]: it may have run, so the client must not
//! send it again on another path either.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wellfed_model::{Answer, Capabilities, ClientRole, Op, Pushed};

use super::live::Unavailable;

/// `POST /device/ask`: one request under the client's own id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    pub id: [u8; 16],
    pub op: Op,
}

/// What became of an [`Ask`] the session took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "lowercase")]
pub enum Asked {
    Answered {
        answer: Answer,
    },
    /// Sent, and no answer came before the session ended or its answer
    /// time ran out: the request may have run.
    Uncertain,
}

/// The 503 body of every device route: the session cannot carry a request
/// now, and nothing was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{reason}")]
pub struct DeviceUnavailable {
    pub reason: Unavailable,
}

/// `GET /device/session`: whom the server's session is with, so a client
/// can check the key and the feeder's capabilities before asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Canonical: the owner key file the server opened the session with.
    pub keys: PathBuf,
    /// The feeder's identity, lowercase hex.
    pub identity: String,
    pub build: String,
    pub role: ClientRole,
    pub capabilities: Capabilities,
}

/// `GET /device/pushed?from=N&wait=MS`: the messages the session has
/// received from number `from` on. The server keeps the newest few
/// thousand; a reader that fell behind starts at `first`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushedPage {
    /// The number of `messages[0]`.
    pub first: u64,
    /// The number the next message will have: the reader's next `from`.
    pub next: u64,
    pub messages: Vec<Pushed>,
}
