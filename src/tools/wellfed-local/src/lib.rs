//! Stock-firmware PLAF103 local server.

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

pub mod api;
pub mod build_info;
pub mod config;
pub mod controller;
pub mod mqtt;
pub mod network;
pub mod ota;
pub mod probe;
pub mod protocol;
pub mod replacement;
pub mod stock_setup;
pub mod storage;
