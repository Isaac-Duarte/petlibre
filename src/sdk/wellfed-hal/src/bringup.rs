//! The raw outputs, for bring-up firmwares and the probes they run.
//!
//! Product firmware never needs this module. It reaches the motor pins
//! through [`Board`](crate::board::Board) (inside the feeding owner), the
//! watchdog through the runtime's supervision, and storage through the
//! settings writer, the update path and the stores' slot I/O. A bring-up
//! firmware that has to touch a pin, the watchdog or a storage sector
//! directly takes it from here, and every function here is `unsafe`: its
//! caller takes over the ownership contract the HAL's owners otherwise keep
//! (no two writers of one pin, one watchdog owner, a flash write only under
//! the armed hooks' lease).

pub use crate::sdk::{
    gpio_init, gpio_port_write, gpio_write_bit, storage_erase, storage_program, wdg_cmd, wdg_init,
    wdg_refresh,
};
