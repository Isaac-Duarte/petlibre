//! Pairing a new owner (BLE setup, or the recovery window over the LAN):
//! the feeder's replies a client acts on. The code proof's own rounds stay
//! inside the client's session.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// Key the client's code in on the feeder within `seconds`.
    EnterCode {
        seconds: u16,
    },
    /// The client is an owner, and the Wi-Fi is saved for the next join.
    /// The feeder reboots without BLE shortly after.
    Done {
        enrolled: Enrolled,
        wifi: WifiSaved,
    },
    Refused(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enrolled {
    New,
    /// This key was already enrolled; nothing changed.
    Already,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WifiSaved {
    Saved,
    /// The request carried no Wi-Fi.
    Unchanged,
}

/// Why the feeder refused to pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// Another setup is waiting for its code.
    Busy,
    /// Nobody keyed the code in in time, or the proof stalled.
    Unconfirmed,
    /// The code keyed in on the feeder is not the client's, or someone is
    /// relaying the session.
    WrongCode,
    /// Every client slot is taken; an owner must revoke one first.
    Full,
    /// The feeder's store cannot be written now.
    NotWritable,
    /// The request's label or credentials failed the feeder's checks.
    Invalid,
}
