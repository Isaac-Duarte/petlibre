//! Feeding: the controller and the inputs, outputs and durable interface it
//! runs between, plus the synthetic check that drives them together.

pub mod controller;
pub mod diagnostic;
pub mod feedback;
pub mod food_read;
pub mod motor_gate;
pub mod observe;
pub mod owner;
pub mod progress;
pub mod sensor_inputs;
pub mod storage;
