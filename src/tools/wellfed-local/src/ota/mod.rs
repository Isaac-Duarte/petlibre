//! Firmware updates, stock and replacement.
//!
//! The stock firmware takes `OTA_UPGRADE` commands for the fixed hash-pinned
//! trial images (`trial`), fetches them from the single-peer image server
//! (`image`), and reports progress that `report` keeps a bounded record of.
//! The replacement firmware instead fetches a signed update over the MQTT
//! `ota/get` topic (`firmware`).

pub mod firmware;
pub mod image;
pub mod report;
pub mod trial;
