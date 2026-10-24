//! FW45 BLE setup: a new client's first session with a feeder in setup mode.
//!
//! The client (the app, or the owner's `wellfed` CLI) runs a Noise XX
//! handshake as initiator over the feeder's setup characteristic, with
//! [`PROLOGUE`]. XX gives each side the other's static key, so the client
//! learns the feeder's identity and the feeder learns the key it will
//! enroll. The third handshake message carries the client's [`Request`],
//! encrypted, so the Wi-Fi passphrase never crosses the air in the clear.
//! The feeder answers with transport messages: [`Reply::Confirming`] while
//! it waits for a Dispense press, then [`Reply::Done`] or
//! [`Reply::Refused`].

use serde::{Deserialize, Serialize};

/// Bound into the handshake hash, so a session meant for another protocol
/// or version fails at its first encrypted message.
pub const PROLOGUE: &[u8] = b"wellfed-setup-v1";

/// The client's label as the feeder stores it: 1-16 bytes, no NUL.
pub const LABEL_MAX: usize = 16;

/// What a new client asks for, in the handshake's third message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request<'a> {
    /// A name for the client, shown when the owner lists clients.
    pub label: &'a str,
    /// Credentials for the feeder's next join; `None` keeps what it has.
    #[serde(borrow)]
    pub wifi: Option<Wifi<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wifi<'a> {
    /// 1-32 bytes; an SSID need not be text.
    pub ssid: &'a [u8],
    /// 8-63 printable ASCII characters (WPA2-PSK).
    pub psk: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// Press Dispense on the feeder within `seconds` to confirm.
    Confirming {
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
    /// Another setup is waiting for its press.
    Busy,
    /// Nobody pressed Dispense in time.
    Unconfirmed,
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
        let request = Request {
            label: "phone",
            wifi: Some(Wifi {
                ssid: b"Lab \xff",
                psk: "pass word!",
            }),
        };
        let mut out = [0; 64];
        let len = encode(&request, &mut out).unwrap();
        assert_eq!(decode::<Request<'_>>(&out[..len]), Ok(request));
    }

    #[test]
    fn replies_round_trip() {
        for reply in [
            Reply::Confirming { seconds: 60 },
            Reply::Done {
                enrolled: Enrolled::New,
                wifi: WifiSaved::Saved,
            },
            Reply::Refused(Refusal::Unconfirmed),
        ] {
            let mut out = [0; 8];
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

        let request = Request {
            label: "a label",
            wifi: None,
        };
        assert_eq!(encode(&request, &mut [0; 3]), Err(Error::BufferTooSmall));
    }
}
