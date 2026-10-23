//! The KM4 side of the SDK flash-lock hooks: the lease provider each SDK
//! flash operation goes through, and the arming sequence every KM4 writer
//! runs before installing it.

pub mod arming;
pub mod provider;
