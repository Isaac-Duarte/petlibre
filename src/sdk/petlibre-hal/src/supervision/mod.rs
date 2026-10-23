//! KM4 supervision: the TIM2 tick and feedback capture, the watchdog and when
//! it is fed, and the bench-derived constants they run with.

pub(crate) mod active;
pub mod config;
pub mod events;
pub mod supervise;
pub mod watchdog;
