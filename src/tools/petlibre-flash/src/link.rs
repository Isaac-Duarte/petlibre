//! The layered transport to the chip, lowest first: the physical UART, the
//! ROM loader and pinned helper protocol over it, bounded sector transport
//! over an established helper session, and the flash status reads that guard
//! every mutation boundary.

pub mod protocol;
pub mod sector;
pub mod serial;
pub mod status;
