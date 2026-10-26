//! KM4 supervision: the TIM2 tick and feedback capture, the feeding owner's
//! beat, the watchdog and when it is fed, the breadcrumb a silent reset
//! leaves, and the bench-derived constants they run with.

pub(crate) mod active;
pub mod beat;
pub mod config;
pub mod crumb;
pub mod events;
pub mod supervise;
pub mod watchdog;
