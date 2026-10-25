//! Installing a replacement build from stock.
//!
//! Stock takes `OTA_UPGRADE` commands for a hash-pinned build (`trial`),
//! fetches it from the single-peer image server (`image`), and reports
//! progress that `report` keeps a bounded record of.

pub mod image;
pub mod report;
pub mod trial;
