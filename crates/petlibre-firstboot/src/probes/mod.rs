//! Bench probes selected by the diagnostic application. Each one reads back
//! stopped motor outputs; only the FW16 pulse probe, built with its own
//! feature, ever drives the motor.

pub mod adc;
pub mod flash;
pub mod optics;
pub mod pulse;
pub mod supervision;
pub mod wifi;
