//! The one error Dart sees: a closed [`ErrorKind`] the screens choose their
//! words from, and the underlying message for logs.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The system random source could not be read.
    Random,
    /// A stored client secret is not 32 bytes.
    Key,
    /// The phone's name is not 1-16 bytes without NUL.
    Label,
    /// The SSID or passphrase fails the feeder's rules.
    Credentials,
    /// The feeder did not prove the code: mistyped, or another device is
    /// answering or relaying.
    CodeMismatch,
    /// The feeder spoke out of turn or not this protocol.
    Protocol,
    /// The address is not `ip` or `ip:port`.
    Address,
    /// Nothing answered at the address.
    Connect,
    /// The feeder did not answer in time.
    Timeout,
    /// The feeder closed the connection (for recovery: the window is not
    /// open).
    Closed,
    /// The feeder does not know this phone's key.
    UnknownKey,
    /// The feeder serves another protocol version.
    Version,
    /// Every session slot on the feeder is taken.
    Full,
    /// The session already ended.
    Ended,
    /// The app's own file could not be read or written.
    Store,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeError {
    pub kind: ErrorKind,
    pub message: String,
}

impl BridgeError {
    pub(crate) fn new(kind: ErrorKind, message: impl fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for BridgeError {}

impl From<wellfed_client::setup::Error> for BridgeError {
    fn from(error: wellfed_client::setup::Error) -> Self {
        use wellfed_client::setup::Error;
        let kind = match error {
            Error::Label => ErrorKind::Label,
            Error::Credentials => ErrorKind::Credentials,
            Error::WrongCode => ErrorKind::CodeMismatch,
            Error::Noise(_) | Error::Unexpected | Error::Malformed | Error::TooLong(_) => {
                ErrorKind::Protocol
            }
        };
        Self::new(kind, error)
    }
}

impl From<wellfed_connect::Error> for BridgeError {
    fn from(error: wellfed_connect::Error) -> Self {
        use wellfed_connect::Error;
        let kind = match &error {
            Error::Address(_) => ErrorKind::Address,
            Error::ConnectTimeout(_)
            | Error::Connect { .. }
            | Error::Mdns(_)
            | Error::NotFound(_) => ErrorKind::Connect,
            Error::Timeout | Error::Silent => ErrorKind::Timeout,
            Error::Closed | Error::Io(_) => ErrorKind::Closed,
            Error::UnknownKey => ErrorKind::UnknownKey,
            Error::Version(_) => ErrorKind::Version,
            Error::Full => ErrorKind::Full,
            Error::Session(_) | Error::Encode | Error::ReservedId => ErrorKind::Protocol,
        };
        Self::new(kind, error)
    }
}

impl From<wellfed_formats::Error> for BridgeError {
    fn from(error: wellfed_formats::Error) -> Self {
        Self::new(ErrorKind::Store, error)
    }
}
