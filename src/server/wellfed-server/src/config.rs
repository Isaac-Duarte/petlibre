//! The server's settings: `server.toml` in the data directory, under the
//! flags and environment variables that override it.
//!
//! The file holds what is neither a secret nor a household setting: the
//! sealing key has its own file ([`crate::key`]) and the household's zone
//! lives in the database. Every setting has a default, so a server with no
//! file (or an empty one) listens on localhost:8080 with discovery on.
//!
//! A flag, or the environment variable it names, replaces the file's value
//! whole: `--host` given once replaces the file's `hosts` rather than adding
//! to them. Values are checked while the file is read, so a bad one is
//! refused with its key's path and its line, not later by whatever uses it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

/// The settings file, in the data directory.
pub const FILE: &str = "server.toml";

/// The database, in the data directory.
pub const DATABASE: &str = "server.db";

/// The label the server enrolls under on each feeder, unless set.
const LABEL: &str = "wellfed-server";

/// How many daily backups are kept, unless set.
const BACKUPS_KEPT: NonZeroU32 = NonZeroU32::MIN.saturating_add(6);

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The file is not TOML.
    #[error("{} is not valid TOML", path.display())]
    Syntax {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A key is unknown, or its value is the wrong type or out of range.
    /// `field` is the key's dotted path (`retention.raw_sample_days`,
    /// `hosts[1]`), an unknown key's included; `source` has its line.
    #[error("{}: `{field}` is invalid", path.display())]
    Invalid {
        path: PathBuf,
        field: String,
        #[source]
        source: Box<toml::de::Error>,
    },
}

/// A value refused by its own type, whether it came from the file or a flag.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvalidValue {
    #[error("a label is 1-16 bytes with no NUL; {len} bytes were given")]
    Label { len: usize },
    #[error("`{name}` is not a host name (letters, digits, `-` and `.`, up to 253)")]
    HostName { name: String },
}

/// The settings the server runs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    /// Where the HTTP API listens.
    pub listen: SocketAddr,
    /// Names the household reaches the server by, besides IP addresses and
    /// `localhost`, which always work.
    pub hosts: Vec<HostName>,
    /// The label the server enrolls under on each feeder.
    pub label: Label,
    pub discovery: Discovery,
    /// Reverse proxies whose `X-Forwarded-*` headers are believed. None by
    /// default: a request's scheme and address are its own connection's.
    pub trusted_proxies: Vec<IpAddr>,
    pub retention: Retention,
    pub releases: Releases,
    pub backups: Backups,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retention {
    /// How many days raw samples are kept; forever when `None`. Hourly
    /// rollups are always kept.
    pub raw_sample_days: Option<NonZeroU32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Releases {
    /// Whether the project's GitHub releases are checked daily. Uploads work
    /// either way, and nothing installs without an owner asking.
    pub fetch: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backups {
    /// How many daily backups are kept.
    pub keep: NonZeroU32,
}

impl Default for Backups {
    fn default() -> Self {
        Self { keep: BACKUPS_KEPT }
    }
}

/// Whether a feeder whose stored address fails is looked up over mDNS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Discovery {
    #[default]
    Mdns,
    /// Adopt and reconnect by address only, as on a bridged container
    /// network where multicast does not reach the feeders.
    Off,
}

impl From<Discovery> for wellfed_service::Discovery {
    fn from(discovery: Discovery) -> Self {
        match discovery {
            Discovery::Mdns => Self::Mdns,
            Discovery::Off => Self::Off,
        }
    }
}

/// The label the server enrolls under on a feeder: 1-16 bytes with no NUL,
/// the most the feeder's client table keeps.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Label(String);

impl Label {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Label> for String {
    fn from(label: Label) -> Self {
        label.0
    }
}

impl TryFrom<String> for Label {
    type Error = InvalidValue;

    fn try_from(label: String) -> Result<Self, InvalidValue> {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 16 || bytes.contains(&0) {
            return Err(InvalidValue::Label { len: bytes.len() });
        }
        Ok(Self(label))
    }
}

impl FromStr for Label {
    type Err = InvalidValue;

    fn from_str(label: &str) -> Result<Self, InvalidValue> {
        Self::try_from(label.to_owned())
    }
}

/// A DNS name the server answers to, lowercased.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct HostName(String);

impl HostName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<HostName> for String {
    fn from(name: HostName) -> Self {
        name.0
    }
}

impl TryFrom<String> for HostName {
    type Error = InvalidValue;

    fn try_from(name: String) -> Result<Self, InvalidValue> {
        let labels_ok = name.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
        if !labels_ok || name.len() > 253 {
            return Err(InvalidValue::HostName { name });
        }
        Ok(Self(name.to_ascii_lowercase()))
    }
}

impl FromStr for HostName {
    type Err = InvalidValue;

    fn from_str(name: &str) -> Result<Self, InvalidValue> {
        Self::try_from(name.to_owned())
    }
}

/// The flags, each with its environment variable, that override the file.
/// An empty list means the flag was not given.
#[derive(Debug, Default, clap::Args)]
pub struct Overrides {
    /// Where the HTTP API listens (`listen`; default 127.0.0.1:8080): this
    /// computer only, unless the household's other devices should reach it
    /// (`0.0.0.0:8080`).
    #[arg(long, env = "WELLFED_SERVER_LISTEN")]
    pub listen: Option<SocketAddr>,
    /// A name the household reaches the server by (`hosts`), such as
    /// `feeder.lan`; IP addresses and `localhost` always work. Repeat for
    /// more names.
    #[arg(
        long = "host",
        env = "WELLFED_SERVER_HOSTS",
        value_delimiter = ',',
        value_name = "NAME"
    )]
    pub hosts: Vec<HostName>,
    /// The label the server enrolls under on each feeder (`label`).
    #[arg(long, env = "WELLFED_SERVER_LABEL")]
    pub label: Option<Label>,
    /// Whether a feeder whose address fails is looked up over mDNS
    /// (`discovery`; default mdns).
    #[arg(
        long,
        env = "WELLFED_SERVER_DISCOVERY",
        conflicts_with = "no_discovery"
    )]
    pub discovery: Option<Discovery>,
    /// Never look feeders up over mDNS: `--discovery off`.
    #[arg(long)]
    pub no_discovery: bool,
    /// A reverse proxy whose forwarded headers are believed
    /// (`trusted_proxies`). Repeat for more.
    #[arg(
        long = "trusted-proxy",
        env = "WELLFED_SERVER_TRUSTED_PROXIES",
        value_delimiter = ',',
        value_name = "ADDRESS"
    )]
    pub trusted_proxies: Vec<IpAddr>,
    /// Days raw samples are kept (`retention.raw_sample_days`; default
    /// forever).
    #[arg(long, env = "WELLFED_SERVER_RAW_SAMPLE_DAYS")]
    pub raw_sample_days: Option<NonZeroU32>,
    /// Whether GitHub releases are checked daily (`releases.fetch`; default
    /// false).
    #[arg(long, env = "WELLFED_SERVER_RELEASES_FETCH", value_name = "BOOL")]
    pub releases_fetch: Option<bool>,
    /// Daily backups kept (`backups.keep`; default 7).
    #[arg(long, env = "WELLFED_SERVER_BACKUPS_KEEP")]
    pub backups_keep: Option<NonZeroU32>,
}

/// `server.toml` as written: every key optional, none unknown.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    listen: Option<SocketAddr>,
    hosts: Option<Vec<HostName>>,
    label: Option<Label>,
    discovery: Option<Discovery>,
    trusted_proxies: Option<Vec<IpAddr>>,
    #[serde(default)]
    retention: RetentionFile,
    #[serde(default)]
    releases: ReleasesFile,
    #[serde(default)]
    backups: BackupsFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionFile {
    raw_sample_days: Option<NonZeroU32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasesFile {
    fetch: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupsFile {
    keep: Option<NonZeroU32>,
}

impl ServerConfig {
    /// The settings from `data`'s `server.toml` (the defaults when there is
    /// none) under `overrides`.
    pub fn load(data: &Path, overrides: Overrides) -> Result<Self, ConfigError> {
        let path = data.join(FILE);
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&path, &text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(source) => return Err(ConfigError::Read { path, source }),
        };
        Ok(Self::merge(file, overrides))
    }

    /// The settings `text` gives, under no overrides; `path` names it in an
    /// error.
    pub fn parse(path: &Path, text: &str) -> Result<Self, ConfigError> {
        Ok(Self::merge(parse(path, text)?, Overrides::default()))
    }

    fn merge(file: File, overrides: Overrides) -> Self {
        let discovery = if overrides.no_discovery {
            Some(Discovery::Off)
        } else {
            overrides.discovery
        };

        Self {
            listen: overrides
                .listen
                .or(file.listen)
                .unwrap_or(SocketAddr::from((Ipv4Addr::LOCALHOST, 8080))),
            hosts: given(overrides.hosts).or(file.hosts).unwrap_or_default(),
            label: overrides
                .label
                .or(file.label)
                .unwrap_or_else(|| Label(LABEL.to_owned())),
            discovery: discovery.or(file.discovery).unwrap_or_default(),
            trusted_proxies: given(overrides.trusted_proxies)
                .or(file.trusted_proxies)
                .unwrap_or_default(),
            retention: Retention {
                raw_sample_days: overrides.raw_sample_days.or(file.retention.raw_sample_days),
            },
            releases: Releases {
                fetch: overrides
                    .releases_fetch
                    .or(file.releases.fetch)
                    .unwrap_or(false),
            },
            backups: Backups {
                keep: overrides
                    .backups_keep
                    .or(file.backups.keep)
                    .unwrap_or(BACKUPS_KEPT),
            },
        }
    }
}

/// A list flag counts as given when it names at least one value.
fn given<T>(values: Vec<T>) -> Option<Vec<T>> {
    (!values.is_empty()).then_some(values)
}

fn parse(path: &Path, text: &str) -> Result<File, ConfigError> {
    let deserializer = toml::Deserializer::parse(text).map_err(|source| ConfigError::Syntax {
        path: path.to_owned(),
        source: Box::new(source),
    })?;

    serde_path_to_error::deserialize(deserializer).map_err(|error| ConfigError::Invalid {
        path: path.to_owned(),
        field: error.path().to_string(),
        source: Box::new(error.into_inner()),
    })
}
