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

use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{config, storage};

/// The record's extension, after the config's own stem: `installer.json`'s
/// record is `installer.device.json`. Named for its config, so an
/// installer's config beside the bench's captured one (both in `private/`)
/// never lends the bench its enrollment.
const EXTENSION: &str = "device.json";

/// Prefix of the server's own client ID, which no feeder may claim.
const SERVER_PREFIX: &str = "local-server";

/// The feeder that enrolled: its own MQTT identity, and where it first
/// connected from. Its client ID is its device ID.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub client_id: String,
    pub username: String,
    pub password: String,
    pub address: IpAddr,
    pub enrolled_at: DateTime<Utc>,
}

impl Enrollment {
    /// The record beside `config_path`.
    pub fn path(config_path: &Path) -> PathBuf {
        config_path.with_extension(EXTENSION)
    }

    /// The record at `path`, if one exists.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match fs::read(path) {
            Ok(bytes) => {
                let record: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid enrollment {}", path.display()))?;
                Ok(Some(record))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
        }
    }

    /// A CONNECT that looks like a stock feeder's: a client ID stock's
    /// topics can carry, which is not the server's, and credentials.
    pub fn plausible(client_id: &str, username: &str, password: &[u8]) -> bool {
        config::valid_device_id(client_id)
            && !client_id.starts_with(SERVER_PREFIX)
            && !username.is_empty()
            && !password.is_empty()
    }

    /// Record this identity at `path`; refused if any record exists there.
    pub fn create(&self, path: &Path) -> Result<()> {
        storage::write_new_json(path, self)
    }
}

#[cfg(test)]
mod tests {
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

        record().create(&path).unwrap();
        let read = Enrollment::load(&path).unwrap().unwrap();
        assert_eq!(read.client_id, "AF_NEW");
        assert_eq!(read.address.to_string(), "192.168.4.20");

        let mut other = record();
        other.client_id = "AF_OTHER".into();
        assert!(other.create(&path).is_err(), "never replaced");
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
    fn only_a_stock_shaped_identity_is_plausible() {
        assert!(Enrollment::plausible("AF_NEW", "stock", b"pw"));
        assert!(!Enrollment::plausible("local-server-AF_NEW", "x", b"pw"));
        assert!(!Enrollment::plausible("", "stock", b"pw"));
        assert!(!Enrollment::plausible("AF/../x", "stock", b"pw"));
        assert!(!Enrollment::plausible("AF_NEW", "", b"pw"));
        assert!(!Enrollment::plausible("AF_NEW", "stock", b""));
        assert!(!Enrollment::plausible(&"A".repeat(65), "stock", b"pw"));
    }
}
