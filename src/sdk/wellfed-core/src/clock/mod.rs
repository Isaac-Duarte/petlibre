//! Time: the extended hardware tick counter, the wall clock it carries,
//! the owner's zone rule and the NTP exchange that feeds the wall clock.

pub mod ntp;
pub mod tick;
pub mod wall;
pub mod zone;
