//! EXPERIMENTAL Bluetooth LE firmware on the Wellfed SDK (FW34): one task
//! brings up the AmebaD BLE stack beside Wi-Fi and, once the feeder has been
//! offline for two minutes, advertises a setup service through which a host
//! sends signed Wi-Fi credentials (`wellfed-local wifi --ble`).
//!
//! Stock uses BLE for the same job with the vendor app; this firmware keeps
//! the role with the FW25 `wifi` request and its signature. The setup logic
//! is the SDK's (`wellfed_hal::ble`, host-tested there); this crate only
//! declares `bluetooth = true`, which links the device's BLE peripheral, and
//! runs the task. It never touches the motor or the infrared emitters.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-ble target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
