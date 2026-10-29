//! The bench server's `/device/*` routes: the one owner session the server
//! holds, lent to a local client (the `wellfed` CLI) one request at a time.
//!
//! The server resends nothing but a read, and that once. A write that went
//! out and got no answer comes back [`Asked::Uncertain`]: it may have run,
//! so the client must not send it again on another path either.

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

/// The 503 (or, for a request id already out, 409) body of every device
/// route: the session cannot carry the request, and nothing was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{reason}")]
pub struct DeviceUnavailable {
    pub reason: Unavailable,
}

/// `GET /device/session`: whom the server's session speaks as and with,
/// from its key file, so a client can check its key before asking, and,
/// once the session is open, the feeder's build and capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// The checkout the server runs for.
    pub root: PathBuf,
    /// Canonical: the owner key file the server opens the session with.
    pub keys: PathBuf,
    /// That key's public half, lowercase hex.
    pub owner: String,
    /// The feeder's identity, lowercase hex.
    pub identity: String,
    /// `None` until the session is open.
    pub opened: Option<Opened>,
}

/// What the feeder's welcome said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opened {
    pub build: String,
    pub role: ClientRole,
    pub capabilities: Capabilities,
}

/// Which of the session's pushed messages a page reads: the history
/// (events and gaps) or the debug log, each kept to its own bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Events,
    Logs,
}

/// `GET /device/pushed?stream=&from=N&wait=MS`: the messages the session
/// has received from number `from` on. The server keeps the newest few
/// thousand of each stream; a reader that fell behind starts at `first`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushedPage {
    /// The number of `messages[0]`.
    pub first: u64,
    /// The number the next message will have: the reader's next `from`.
    pub next: u64,
    /// For the events: every event after this sequence number is here (or
    /// was pushed since), and none before it is. `None` when the ring holds
    /// the feeder's history from its oldest event.
    pub floor: Option<u64>,
    pub messages: Vec<Pushed>,
}
