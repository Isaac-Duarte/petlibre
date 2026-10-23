//! Stock-firmware PLAF103 local server.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod broker;
pub mod build_info;
pub mod compat;
pub mod config;
pub mod controller;
pub mod firmware_ota;
pub mod mqtt;
pub mod network;
pub mod ota;
pub mod ota_image;
pub mod ota_trial;
pub mod probe;
pub mod protocol;
pub mod replacement;
pub mod request_auth;
pub mod speak;
pub mod stock_setup;
pub mod storage;
pub mod wifi_ble;
