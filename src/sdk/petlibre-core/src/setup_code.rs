//! FW45 setup code: the owner carries a one-time code from the client's
//! screen to the feeder's buttons, and both sides prove they hold it.
//!
//! BLE setup's Noise XX handshake trusts whichever feeder answers, so a
//! relay in radio range could sit between the phone and the feeder and get
//! its own key enrolled. The code closes that gap. The client draws it and
//! shows it as [`PRESSES`] button presses; the owner keys it in on the
//! feeder, which has no display. Then each side proves the code to the
//! other, bound to its own view of the handshake hash. A relay has a
//! different handshake on each leg, so it cannot pass a proof from one leg
//! to the other and must guess the presses.
//!
//! A single proof of the whole code would let a relay try every code
//! offline against it, so the exchange goes one press at a time, as
//! Bluetooth's passkey entry does: each side commits to press `i` before
//! either opens it. A relay must commit to every press before it learns it,
//! on one leg or the other. It does learn one press for free by failing a
//! leg, so it fools one side with odds of 4^-(PRESSES - 1) and gets enrolled
//! with odds of 4^-PRESSES. The code is used once, so what it learns of one
//! code is worth nothing for the next.
//!
//! Each message of the exchange, either way, opens the sender's previous
//! commitment and commits to its next press ([`Round`]). The feeder writes
//! first. [`Exchange`] keeps the turns, so neither side opens a press before
//! the other's commitment to it has arrived.

use crate::buttons::Button;
use crate::mac::{hmac_sha256, same};

/// Presses in a code: two bits each, from the four panel buttons.
pub const PRESSES: usize = 6;
/// Every commitment starts with this, so it means nothing anywhere else.
const TAG: &[u8] = b"petlibre-setup-code-v1";

/// The code the owner keys in. Its presses never appear in `Debug`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Code([Button; PRESSES]);

impl core::fmt::Debug for Code {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Code(..)")
    }
}

impl Code {
    pub const fn new(presses: [Button; PRESSES]) -> Self {
        Self(presses)
    }

    /// A code from uniformly random bits, two per press.
    pub fn from_bits(bits: u16) -> Self {
        let mut presses = [Button::Reset; PRESSES];
        for (i, press) in presses.iter_mut().enumerate() {
            *press = Button::ALL[usize::from(bits >> (2 * i)) & 3];
        }
        Self(presses)
    }

    pub const fn presses(&self) -> &[Button; PRESSES] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Writes the first message.
    Feeder,
    Client,
}

impl Side {
    const fn byte(self) -> u8 {
        match self {
            Self::Feeder => b'F',
            Self::Client => b'C',
        }
    }

    const fn peer(self) -> Self {
        match self {
            Self::Feeder => Self::Client,
            Self::Client => Self::Feeder,
        }
    }
}

/// One message of the exchange. Message `j` opens press `j - 1` (none in
/// the first) and commits to press `j` (none in the last).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Round {
    pub open: Option<[u8; 32]>,
    pub commit: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Called out of turn, after the end or after a failure.
    WrongTurn,
    /// The message lacks what its place in the exchange needs, or carries
    /// what it must not.
    Malformed,
    /// The peer's opening does not match its commitment for our press: it
    /// holds another code, or another handshake.
    Mismatch,
}

/// One side of the exchange.
#[derive(Clone)]
pub struct Exchange {
    side: Side,
    hash: [u8; 32],
    code: Code,
    /// Derives each press's nonce; fresh for every exchange.
    seed: [u8; 32],
    sent: usize,
    received: usize,
    /// The peer's commitment to the next press it will open.
    peer_commit: Option<[u8; 32]>,
    failed: bool,
}

impl core::fmt::Debug for Exchange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Exchange")
            .field("side", &self.side)
            .field("sent", &self.sent)
            .field("received", &self.received)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl Exchange {
    /// `hash` is this side's Noise handshake hash; `seed` a fresh random
    /// secret, never used before.
    pub const fn new(side: Side, hash: [u8; 32], code: Code, seed: [u8; 32]) -> Self {
        Self {
            side,
            hash,
            code,
            seed,
            sent: 0,
            received: 0,
            peer_commit: None,
            failed: false,
        }
    }

    /// Whether the next call is ours to write.
    pub fn is_my_turn(&self) -> bool {
        let turn = match self.side {
            Side::Feeder => self.sent == self.received,
            Side::Client => self.received == self.sent + 1,
        };
        !self.failed && self.sent <= PRESSES && turn
    }

    /// Every press of the peer's has opened against ours: the peer holds
    /// the code, under this handshake. The client knows before its last
    /// message, the feeder only after reading it.
    pub fn peer_proven(&self) -> bool {
        !self.failed && self.received == PRESSES + 1
    }

    /// Our next message.
    pub fn write(&mut self) -> Result<Round, Error> {
        if !self.is_my_turn() {
            return Err(Error::WrongTurn);
        }

        let j = self.sent;
        let open = j.checked_sub(1).map(|i| self.nonce(i));
        let commit = self.code.0.get(j).map(|&press| {
            let nonce = self.nonce(j);
            commitment(self.side, &self.hash, j, press, &nonce)
        });

        self.sent += 1;
        Ok(Round { open, commit })
    }

    /// The peer's next message. Any error ends the exchange.
    pub fn read(&mut self, round: Round) -> Result<(), Error> {
        let result = self.check(round);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn check(&mut self, round: Round) -> Result<(), Error> {
        let peers_turn = match self.side {
            Side::Feeder => self.sent == self.received + 1,
            Side::Client => self.sent == self.received,
        };
        if self.failed || self.received > PRESSES || !peers_turn {
            return Err(Error::WrongTurn);
        }

        let j = self.received;
        if round.open.is_some() != (j > 0) || round.commit.is_some() != (j < PRESSES) {
            return Err(Error::Malformed);
        }
        if let (Some(i), Some(open)) = (j.checked_sub(1), round.open) {
            let committed = self.peer_commit.ok_or(Error::WrongTurn)?;
            let press = self.code.0.get(i).copied().ok_or(Error::WrongTurn)?;
            let expected = commitment(self.side.peer(), &self.hash, i, press, &open);
            if !same(&expected, &committed) {
                return Err(Error::Mismatch);
            }
        }

        self.peer_commit = round.commit;
        self.received += 1;
        Ok(())
    }

    fn nonce(&self, index: usize) -> [u8; 32] {
        hmac_sha256(&self.seed, &[&[byte(index)]])
    }
}

/// `side`'s commitment to `press` at `index`, under one handshake. Keyed by
/// the random nonce, so it hides the press until the nonce is opened, and
/// opening it to another press would need an HMAC collision.
fn commitment(
    side: Side,
    hash: &[u8; 32],
    index: usize,
    press: Button,
    nonce: &[u8; 32],
) -> [u8; 32] {
    hmac_sha256(
        nonce,
        &[TAG, &[side.byte(), byte(index), press as u8], hash],
    )
}

/// A press index as one byte; every index is below [`PRESSES`].
fn byte(index: usize) -> u8 {
    u8::try_from(index).unwrap_or(u8::MAX)
}
