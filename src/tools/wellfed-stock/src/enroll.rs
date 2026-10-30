//! Trust on first use: during an install, the broker admits the first
//! stock feeder that connects with credentials it has never seen, and pins
//! it ([consumer installation]).
//!
//! Stock's MQTT client ID, username and password are its own, loaded from
//! its flash at boot (static analysis), so a feeder the setup frame pointed
//! here arrives with nothing the server could have known. Instead of
//! importing them from a capture, the broker records them the first time,
//! only while an installer opened an enrollment window, and only once: the
//! record is created, never replaced, so a second feeder (or a stranger on
//! the LAN after the first) is refused like any wrong password. From then on
//! [`crate::config::load`] reads the device from this record.
//!
//! The record holds working credentials, so like the config it is written
//! mode 0600 and has no `Debug`.
//!
//! [consumer installation]: ../../../../docs/firmware/consumer-installation.md

use std::{net::IpAddr, path::Path};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
pub use wellfed_formats::stock::Enrollment;

use crate::{config, storage};

/// Prefix of the server's own client ID, which no feeder may claim.
const SERVER_PREFIX: &str = "local-server";

/// A CONNECT that looks like a stock feeder's: a client ID stock's topics
/// can carry, which is not the server's, and credentials.
pub fn plausible(client_id: &str, username: &str, password: &[u8]) -> bool {
    config::valid_device_id(client_id)
        && !client_id.starts_with(SERVER_PREFIX)
        && !username.is_empty()
        && !password.is_empty()
}

/// Record `record` at `path`; refused if any record exists there.
pub fn create(record: &Enrollment, path: &Path) -> Result<()> {
    storage::write_new_json(path, record)
}

/// A second identity seen inside the window. Trust on first use cannot
/// tell which of two feeders on the LAN is the owner's: a host that raced
/// the real feeder could be walked through the whole install, so the run
/// must stop. Its credentials are not kept.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Contest {
    pub client_id: String,
    pub address: Option<IpAddr>,
    pub at: DateTime<Utc>,
}

impl Contest {
    /// Mark the enrollment beside the record at `contested` as contested;
    /// the first contest is kept.
    pub fn record(&self, contested: &Path) -> Result<()> {
        if contested.exists() {
            return Ok(());
        }
        storage::write_new_json(contested, self)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn record() -> Enrollment {
        Enrollment {
            client_id: "AF_NEW".into(),
            username: "stock".into(),
            password: "its-own".into(),
            address: "192.168.4.20".parse().unwrap(),
            enrolled_at: Utc::now(),
        }
    }

    #[test]
    fn a_record_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = Enrollment::path(&dir.path().join("server.json"));
        assert!(Enrollment::load(&path).unwrap().is_none());

        create(&record(), &path).unwrap();
        let read = Enrollment::load(&path).unwrap().unwrap();
        assert_eq!(read.client_id, "AF_NEW");
        assert_eq!(read.address.to_string(), "192.168.4.20");

        let mut other = record();
        other.client_id = "AF_OTHER".into();
        assert!(create(&other, &path).is_err(), "never replaced");
        assert_eq!(
            Enrollment::load(&path).unwrap().unwrap().client_id,
            "AF_NEW"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn an_unreadable_record_is_an_error_not_a_captured_device() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("installer.json");
        assert!(Enrollment::view(&config).unwrap().is_none());

        fs::write(Enrollment::path(&config), "{ torn").unwrap();
        assert!(Enrollment::view(&config).is_err());
    }

    #[test]
    fn only_a_stock_shaped_identity_is_plausible() {
        assert!(plausible("AF_NEW", "stock", b"pw"));
        assert!(!plausible("local-server-AF_NEW", "x", b"pw"));
        assert!(!plausible("", "stock", b"pw"));
        assert!(!plausible("AF/../x", "stock", b"pw"));
        assert!(!plausible("AF_NEW", "", b"pw"));
        assert!(!plausible("AF_NEW", "stock", b""));
        assert!(!plausible(&"A".repeat(65), "stock", b"pw"));
    }
}
