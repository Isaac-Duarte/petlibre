//! DSD TECH SH-UR04A relay board: CP2102, 9600 8N1, `AT+CHn=0/1` and `AT+CHn=?`.
//!
//! The bench commands drive the feeder's power, boot strap and UART lines through
//! [`Relay`]. Every command must come back with `OK`, and a set must echo the
//! exact channel and value; anything else is an error, so a sequence stops
//! rather than assuming a relay moved. `FakeBoard` (the `fake` feature)
//! stands in for the board in tests.
//!
//! The crate knows the board and the channel wiring, nothing of the bench's
//! sequences: those belong to `wellfed-bench`, and the read-only page links
//! none of this.

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The board answered a command without `OK`.
    #[error("Relay refused {command}: {reply:?}")]
    Refused { command: String, reply: String },
    /// A set's reply did not echo the channel and value.
    #[error("Relay {channel} reply {reply:?}")]
    Reply { channel: u8, reply: String },
    /// A query's reply was neither on nor off.
    #[error("Relay {channel} state {reply:?}")]
    State { channel: u8, reply: String },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("Serial port: {0}")]
    Serial(#[from] serialport::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Switches the feeder's USB 5V.
pub const POWER: u8 = 1;
/// Holds feeder TX to GND through the 4.7 kΩ strap.
pub const STRAP: u8 = 2;
/// The UART adapter's two lines, each through one relay: 3 carries adapter
/// TXD to the feeder's RX, 4 the feeder's TX to adapter RXD (the strap stays
/// on the feeder side). A powered adapter holds both lines high, which can
/// back-power a feeder that is off, so they open while it is.
pub const UART: [u8; 2] = [3, 4];

/// One command line out, one reply line back.
pub trait Link: Send {
    /// Discard pending input, send `line` with CRLF, and return the reply up
    /// to its newline (or whatever arrived within the timeout).
    fn transact(&mut self, line: &str) -> io::Result<Vec<u8>>;
}

struct SerialLink(Box<dyn serialport::SerialPort>);

impl SerialLink {
    fn open(path: &str) -> Result<Self> {
        let builder = serialport::new(path, 9600).timeout(Duration::from_millis(100));
        // Exclusive, so a second bench process cannot share the board.
        #[cfg(unix)]
        let port: Box<dyn serialport::SerialPort> = {
            let mut port = builder.open_native()?;
            port.set_exclusive(true)?;
            Box::new(port)
        };
        #[cfg(not(unix))]
        let port = builder.open()?;
        Ok(Self(port))
    }
}

impl Link for SerialLink {
    fn transact(&mut self, line: &str) -> io::Result<Vec<u8>> {
        self.0.clear(serialport::ClearBuffer::Input)?;
        self.0.write_all(format!("{line}\r\n").as_bytes())?;
        self.0.flush()?;

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut reply = Vec::new();
        let mut byte = [0u8];
        while Instant::now() < deadline {
            match self.0.read(&mut byte) {
                Ok(1) => {
                    reply.push(byte[0]);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error),
            }
        }

        Ok(reply)
    }
}

pub struct Relay {
    link: Box<dyn Link>,
}

impl Relay {
    pub fn new(link: Box<dyn Link>) -> Self {
        Self { link }
    }

    pub fn open(path: &str) -> Result<Self> {
        Ok(Self::new(Box::new(SerialLink::open(path)?)))
    }

    /// No board: every command fails. For a command that never switches a
    /// relay, so it does not hold the board's port while it runs.
    pub fn detached() -> Self {
        Self::new(Box::new(Detached))
    }

    fn command(&mut self, text: &str) -> Result<Vec<u8>> {
        let raw = self.link.transact(text)?;
        let reply = trim(&raw).to_vec();
        if !reply.starts_with(b"OK") {
            return Err(Error::Refused {
                command: text.into(),
                reply: String::from_utf8_lossy(&reply).into_owned(),
            });
        }
        Ok(reply)
    }

    pub fn set(&mut self, channel: u8, on: bool) -> Result<()> {
        let value = u8::from(on);
        let reply = self.command(&format!("AT+CH{channel}={value}"))?;
        if reply != format!("OK+CH{channel}={value}").as_bytes() {
            return Err(Error::Reply {
                channel,
                reply: String::from_utf8_lossy(&reply).into_owned(),
            });
        }
        Ok(())
    }

    pub fn get(&mut self, channel: u8) -> Result<bool> {
        let reply = self.command(&format!("AT+CH{channel}=?"))?;
        let on = reply == format!("OK+CH{channel}=1").as_bytes();
        if !on && reply != format!("OK+CH{channel}=0").as_bytes() {
            return Err(Error::State {
                channel,
                reply: String::from_utf8_lossy(&reply).into_owned(),
            });
        }
        Ok(on)
    }
}

struct Detached;

impl Link for Detached {
    fn transact(&mut self, line: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::other(format!(
            "This command does not open the relay board; refused {line}"
        )))
    }
}

/// The board ends replies with CRLF and a trailing NUL.
fn trim(raw: &[u8]) -> &[u8] {
    let junk = |b: &u8| matches!(b, b'\0' | b'\r' | b'\n');
    let start = raw.iter().position(|b| !junk(b)).unwrap_or(raw.len());
    let end = raw.iter().rposition(|b| !junk(b)).map_or(start, |i| i + 1);
    &raw[start..end]
}

/// Answers like the SH-UR04A, including its trailing NUL byte; for tests.
#[cfg(any(test, feature = "fake"))]
#[derive(Clone, Default)]
pub struct FakeBoard {
    pub inner: std::sync::Arc<std::sync::Mutex<FakeState>>,
}

#[cfg(any(test, feature = "fake"))]
#[derive(Default)]
pub struct FakeState {
    /// Indexed by channel number; slot 0 is unused.
    pub on: [bool; 5],
    /// Every set, in order: (channel, on).
    pub log: Vec<(u8, bool)>,
    pub refuse: bool,
}

#[cfg(any(test, feature = "fake"))]
impl FakeBoard {
    /// A test that panicked holding the lock leaves plain data behind.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn state(&self) -> [bool; 4] {
        let s = self.lock();
        [s.on[1], s.on[2], s.on[3], s.on[4]]
    }

    pub fn log(&self) -> Vec<(u8, bool)> {
        self.lock().log.clone()
    }
}

#[cfg(any(test, feature = "fake"))]
impl Link for FakeBoard {
    fn transact(&mut self, line: &str) -> io::Result<Vec<u8>> {
        let mut s = self.lock();
        let parsed = line
            .strip_prefix("AT+CH")
            .and_then(|rest| rest.split_once('='))
            .and_then(|(channel, value)| Some((channel.parse::<u8>().ok()?, value)))
            .filter(|(channel, value)| {
                (1..=4).contains(channel) && ["0", "1", "?"].contains(value)
            });
        let Some((channel, value)) = parsed.filter(|_| !s.refuse) else {
            return Ok(b"Error\r\n\0".to_vec());
        };

        let slot = usize::from(channel);
        if value != "?" {
            s.on[slot] = value == "1";
            s.log.push((channel, value == "1"));
        }
        Ok(format!("OK+CH{channel}={}\r\n\0", u8::from(s.on[slot])).into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_query_match_device_replies() {
        let board = FakeBoard::default();
        let mut relay = Relay::new(Box::new(board.clone()));

        relay.set(1, true).unwrap();

        assert!(relay.get(1).unwrap());
        assert!(!relay.get(2).unwrap());
        assert_eq!(board.log(), [(1, true)]);
    }

    #[test]
    fn refusal_stops() {
        let board = FakeBoard::default();
        board.inner.lock().unwrap().refuse = true;
        let mut relay = Relay::new(Box::new(board));

        assert!(relay.set(1, true).is_err());
    }

    #[test]
    fn a_detached_relay_switches_nothing() {
        assert!(Relay::detached().set(1, false).is_err());
    }

    #[test]
    fn replies_are_trimmed_of_line_ends_and_nul() {
        assert_eq!(trim(b"\0OK+CH1=0\r\n\0"), b"OK+CH1=0");
        assert_eq!(trim(b"\0\0"), b"");
    }
}
