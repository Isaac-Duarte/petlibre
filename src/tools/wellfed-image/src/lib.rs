//! The PLAF103 OTA image as the host tools read it: the container and the
//! core images it must hold ([`ota`]), the two banks an image is installed
//! into ([`bank`]), the `private/` evidence root every input and output
//! lives under ([`root`]), and the frozen reviews `wellfed-build review`
//! makes and the flash client, the signer and the audits accept
//! ([`review`]).
//!
//! None of it opens a serial port. It is separate from `wellfed-flash` so
//! the builder, the signer and the audits check images exactly as the flash
//! client does without linking the writer, which only ever runs as its own
//! process.

#![forbid(unsafe_code)]

pub mod bank;
mod error;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
pub mod ota;
pub mod review;
pub mod root;

pub use error::{Error, Result};
