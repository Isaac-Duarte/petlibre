//! Device API, the feeder's side ([`wellfed_proto::api`]).
//!
//! A [`Server`] holds [`SESSIONS`] fixed slots, one per TCP connection, and
//! never touches a socket: the firmware's driver copies received bytes into
//! a slot ([`Server::rx`], [`Server::received`]), sends what a slot holds
//! ([`Server::tx`], [`Server::sent`]) and closes it when asked
//! ([`Server::closing`]). Everything else happens in [`Server::step`], once
//! per pass of the loop task:
//! - **One handshake operation per pass,** across all slots, and none while
//!   a feed runs: an X25519 costs about 27 ms on the KM4, so four clients
//!   connecting at once cannot stack up to hold the loop.
//! - **One frame per slot per pass,** and a slot writes again only once its
//!   last frame is sent, so a flood costs only its own turn.
//! - **Every write has an id** the server remembers per client key, so a
//!   retry, even after a reconnect, gets the first answer instead of
//!   running again.
//! - **Writes carry their base generation;** a stale one is refused with
//!   the current generation.
//! - **A feed is answered once it is durably reserved,** with the number
//!   the journal holds it under; its result is a `Fed` event, which only
//!   sessions whose hello carries [`Capabilities::FEED`] are sent.
//! - **Updates are an owner's** ([`Capabilities::UPDATE`]). The device
//!   checks an offer outside the loop (its signatures need more stack than
//!   the loop has) and answers on a later pass. An update request waits
//!   for no feed: an offer, a cancel or a key statement is refused `Busy`
//!   while a feed is queued or runs, and a feed is refused `Busy` while a
//!   request or an install holds the flash. Chunks are
//!   idempotent by offset, so they are not remembered writes. An update
//!   key is enrolled only once someone at the feeder presses Dispense.
//! - **The debug log is an owner's** ([`Capabilities::LOG`]): after
//!   `TailLog`, a session is pushed the lines the device's RAM tail keeps,
//!   in batches that fit a frame, only once its last frame is out and
//!   after any event or sample waiting.
//!
//! Sessions authenticate with Noise IK under the feeder's identity, against
//! the enrolled clients the settings writer publishes. A revoked key's
//! sessions close as soon as the new table arrives. A new client redeems an
//! invite with XXpsk3 and is enrolled through the keys mailbox.
//!
//! The firmware reaches the rest of the feeder through [`Device`], and the
//! host tests through a fake of it.
//!
//! Beside the server sit what its requests hand the rest of the feeder
//! ([`mailbox`]), who may become a client and when ([`pairing`]), the setup
//! exchange that BLE and the recovery window share ([`setup`]), and the
//! client and Wi-Fi values the feeder's stores keep ([`clients`], [`wifi`]).
//! Like the feeder's product crate above it, it knows no hardware, keeps no
//! clock and allocates nothing.
#![no_std]
#![deny(unsafe_code)]

pub mod clients;
mod device;
pub mod mailbox;
pub mod pairing;
mod record;
mod server;
pub mod setup;
pub mod wifi;

pub use device::{
    Device, EventRead, KeyChange, KeyOutcome, UpdateAnswer, UpdateKeyChange, UpdateRequest,
};
pub use record::{Reason, Record};
pub use server::handshake::fingerprint;
pub use server::request::proto_role;
pub use server::{Pass, Server};

use wellfed_proto::api::{FRAME_MAX, MESSAGE_MAX, UPDATE_CHUNK_MAX};
use wellfed_session::frame;

/// Connections served at once (lwIP leaves room for four).
pub const SESSIONS: usize = 4;
/// Write ids remembered per client.
pub const REMEMBERED: usize = 32;
/// A connection that has not finished its handshake by then is dropped.
pub const HANDSHAKE_MS: u32 = 10_000;
/// A session that sends nothing for this long is dropped; clients send a
/// `Status` as a keepalive.
pub const IDLE_MS: u32 = 90_000;
/// After `Reboot`, long enough for the answer to leave.
pub const REBOOT_MS: u32 = 1_000;

const BUFFER: usize = frame::PREFIX + FRAME_MAX;

// A whole chunk request seals into one frame: the request id, the op's tag,
// its offset and the data's length prefix, with room to spare.
const _: () = assert!(UPDATE_CHUNK_MAX + 16 + 16 <= MESSAGE_MAX);
