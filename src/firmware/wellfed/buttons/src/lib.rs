//! EXPERIMENTAL FW20 bring-up firmware for the PLAF103 on the Wellfed SDK:
//! the read-only panel-button probe (`wellfed_probes::buttons`), logging
//! every press, release and bounce for as long as the build runs.
#![cfg_attr(target_os = "none", no_std)]

#[cfg(all(target_os = "none", not(feature = "km4")))]
compile_error!("wellfed-fw-buttons target builds are KM4-only");

#[cfg(target_os = "none")]
mod app;
