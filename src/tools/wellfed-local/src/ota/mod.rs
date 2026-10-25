//! Replacement-firmware updates over MQTT: the firmware fetches a signed
//! update over the `ota/get` topic (`firmware`). Installing from stock is
//! `wellfed_stock::ota`.

pub mod firmware;
