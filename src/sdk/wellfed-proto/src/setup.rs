//! BLE setup: a new client's first session with a feeder in setup mode.
//!
//! The client (the app, or the owner's `wellfed` CLI) runs a Noise XX
//! handshake as initiator over the feeder's setup characteristic, with
//! [`PROLOGUE`]. XX gives each side the other's static key, so the client
//! learns the feeder's identity and the feeder learns the key it will
//! enroll. The third handshake message carries the client's [`Request`],
//! encrypted.
//!
//! XX alone trusts whichever feeder answers, so the owner then carries a
//! one-time code from the client's screen to the feeder's buttons
//! (`wellfed_core::setup_code`):
//! 1. The feeder replies [`Reply::EnterCode`] and waits for the presses.
//! 2. Both sides prove the code to each other, one press at a time, bound
//!    to the handshake: [`Reply::Round`] from the feeder, [`Answer::Round`]
//!    from the client.
//! 3. Only once the feeder has proven the code does the client send its
//!    Wi-Fi, in [`Answer::Last`], so a relay never sees the passphrase.
//! 4. The feeder answers [`Reply::Done`] or [`Reply::Refused`].

use serde::{Deserialize, Serialize};

/// Bound into the handshake hash, so a session meant for another protocol
/// or version fails at its first encrypted message.
pub const PROLOGUE: &[u8] = b"wellfed-setup-v2";

/// The client's label as the feeder stores it: 1-16 bytes, no NUL.
pub const LABEL_MAX: usize = 16;

/// What a new client asks for, in the handshake's third message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request<'a> {
    /// A name for the client, shown when the owner lists clients.
    pub label: &'a str,
}

/// The client's messages after the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Answer<'a> {
    /// One step of the code proof (`wellfed_core::setup_code::Round`).
    Round {
        open: Option<[u8; 32]>,
        commit: Option<[u8; 32]>,
    },
    /// The proof's last step, sent once the feeder has proven the code.
    Last {
        open: [u8; 32],
        /// Credentials for the feeder's next join; `None` keeps what it has.
        #[serde(borrow)]
        wifi: Option<Wifi<'a>>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wifi<'a> {
    /// 1-32 bytes; an SSID need not be text.
    pub ssid: &'a [u8],
    /// 8-63 printable ASCII characters (WPA2-PSK).
    pub psk: &'a str,
}

/// The longest [`Reply`] encoded: a round with both fields.
pub const REPLY_MAX: usize = 1 + 2 * 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// Key the client's code in on the feeder within `seconds`.
    EnterCode {
        seconds: u16,
    },
    /// One step of the code proof, once the code is in.
    Round {
        open: Option<[u8; 32]>,
        commit: Option<[u8; 32]>,
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
pub enum Enrolled {
    New,
    /// This key was already enrolled; nothing changed.
    Already,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WifiSaved {
    Saved,
    /// The request carried no Wi-Fi.
    Unchanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, decode, encode};

    #[test]
    fn requests_round_trip_and_borrow_their_bytes() {
        let request = Request { label: "phone" };
        let mut out = [0; 64];
        let len = encode(&request, &mut out).unwrap();
        assert_eq!(decode::<Request<'_>>(&out[..len]), Ok(request));

        let last = Answer::Last {
            open: [5; 32],
            wifi: Some(Wifi {
                ssid: b"Lab \xff",
                psk: "pass word!",
            }),
        };
        let len = encode(&last, &mut out).unwrap();
        assert_eq!(decode::<Answer<'_>>(&out[..len]), Ok(last));
    }

    #[test]
    fn replies_round_trip() {
        for reply in [
            Reply::EnterCode { seconds: 60 },
            Reply::Round {
                open: Some([1; 32]),
                commit: None,
            },
            Reply::Done {
                enrolled: Enrolled::New,
                wifi: WifiSaved::Saved,
            },
            Reply::Refused(Refusal::Unconfirmed),
        ] {
            let mut out = [0; REPLY_MAX];
            let len = encode(&reply, &mut out).unwrap();
            assert_eq!(decode::<Reply>(&out[..len]), Ok(reply));
        }
    }

    #[test]
    fn trailing_bytes_and_short_buffers_are_refused() {
        let mut out = [0; 8];
        let len = encode(&Reply::Refused(Refusal::Busy), &mut out).unwrap();
        assert_eq!(decode::<Reply>(&out[..=len]), Err(Error::Malformed));
        assert_eq!(decode::<Reply>(&[]), Err(Error::Malformed));

        let request = Request { label: "a label" };
        assert_eq!(encode(&request, &mut [0; 3]), Err(Error::BufferTooSmall));
    }
}
