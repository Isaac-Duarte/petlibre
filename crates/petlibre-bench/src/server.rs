//! What the bench reads from and sends to `petlibre-local`: its status
//! document (`petlibre-local status`, `GET /api/status`), the bodies of
//! `POST /api/replacement/request` and the request forger's lines.
//!
//! Health and replies are the firmware's own JSON (`petlibre-core`'s
//! transport writes them), relayed by the server unchanged. Only the fields
//! the bench checks are typed; each defaults when absent, so a newer server
//! or firmware that adds fields still reads, and a missing one fails the
//! check that needs it rather than the whole document.

use std::fmt::Display;
use std::ops::Not;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A `petlibre-local` status document.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct LocalStatus {
    /// `None` when the server has no replacement-firmware role.
    #[serde(default)]
    pub replacement: Option<Replacement>,
}

/// The server's view of the replacement firmware.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct Replacement {
    pub health: Option<Health>,
    /// RFC 3339; each transport session's first health gives a new one.
    pub received_at: Option<String>,
    pub age_ms: Option<i64>,
    pub online: bool,
    pub stale: bool,
    pub replies: Vec<Relayed>,
}

/// A document without `stale` is not taken as fresh.
impl Default for Replacement {
    fn default() -> Self {
        Self {
            health: None,
            received_at: None,
            age_ms: None,
            online: false,
            stale: true,
            replies: Vec::new(),
        }
    }
}

/// The firmware's health message.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(crate) struct Health {
    /// The startup identity, hex in either case.
    pub build: String,
    pub bank: Option<u64>,
    pub seq: Option<u64>,
    pub outputs: Option<String>,
    pub faults: Vec<String>,
    /// Absent from builds before FW21.
    pub clock: Option<Clock>,
    pub wifi: Wifi,
    pub transport: Transport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Clock {
    pub syncs: u64,
    /// Absent until the clock is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Wifi {
    /// Which credentials joined this boot: `build`, `stored` or `fallback`.
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Transport {
    pub connects: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Relayed {
    pub message: Reply,
}

/// A firmware reply to a request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_ms: Option<u64>,

    /// `settings` and `wifi`: whether the store accepts a change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    /// `settings`: 0 or 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// `wifi`: the stored network's identifier, `None` when nothing is stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Everything else the firmware sent, kept for failure messages.
    #[serde(flatten)]
    pub other: Map<String, Value>,
}

impl Reply {
    /// The settings request that would set this reply's values again.
    pub(crate) fn settings(&self) -> SettingsChange {
        SettingsChange {
            lights: Some(self.lights == Some(1)),
            sound: Some(self.sound == Some(1)),
            volume: self.volume,
        }
    }

    /// Whether this settings reply holds every field `change` sets.
    pub(crate) fn shows(&self, change: &SettingsChange) -> bool {
        let flag =
            |want: Option<bool>, have: Option<u8>| want.is_none_or(|on| have == Some(u8::from(on)));

        flag(change.lights, self.lights)
            && flag(change.sound, self.sound)
            && change
                .volume
                .is_none_or(|volume| self.volume == Some(volume))
    }
}

/// One line of `request_forger` output: a case and the reply it got.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ForgedCase {
    pub case: String,
    /// `None` when no reply came within the forger's window.
    #[serde(default)]
    pub reply: Option<Reply>,
}

/// A `POST /api/replacement/request` body: one of `petlibre_core::transport::Op`
/// with its fields. The server signs it; the bench only reads or sets values
/// again, so a repeat is harmless.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(crate) enum Request {
    Ping,
    Health,
    /// No fields reads the settings.
    Settings(SettingsChange),
    /// No fields reads the Wi-Fi store.
    Wifi(WifiChange),
}

impl Request {
    pub(crate) fn op(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Health => "health",
            Self::Settings(_) => "settings",
            Self::Wifi(_) => "wifi",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SettingsChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lights: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
}

/// Plain-text credentials (the server hex-encodes them), or a clear.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct WifiChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub psk: Option<String>,
    #[serde(skip_serializing_if = "<&bool>::not")]
    pub clear: bool,
}

/// Why a status document does not show what a wait is for, yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotReady {
    #[error("the server has no replacement-firmware role")]
    NoRole,
    #[error("no health received yet")]
    NoHealth,
    #[error("health reports the firmware offline")]
    Offline,
    #[error("health is stale (age {} ms)", shown(.0))]
    Stale(Option<i64>),
    /// The first 12 digits of the build health reports.
    #[error("health reports build {0}")]
    Build(String),
    #[error("health reports bank {}", shown(.0))]
    Bank(Option<u64>),
    /// Health from the session before a boot.
    #[error("health predates the boot")]
    Predates,
    #[error("health has no clock (a pre-FW21 build)")]
    NoClock,
    #[error("the clock never synced: {0}")]
    Unsynced(String),
    #[error("the clock is not set: {0}")]
    ClockUnset(String),
    #[error("status has no received_at")]
    NoReceivedAt,
    #[error("the firmware clock is {0} ms behind the server")]
    Skew(i64),
    #[error("Wi-Fi source {0:?}")]
    WifiSource(Option<String>),
}

fn shown<T: Display>(value: &Option<T>) -> String {
    value
        .as_ref()
        .map_or_else(|| "?".into(), ToString::to_string)
}

impl LocalStatus {
    pub(crate) fn replacement(&self) -> Result<&Replacement, NotReady> {
        self.replacement.as_ref().ok_or(NotReady::NoRole)
    }

    /// When the server last received health.
    pub(crate) fn received_at(&self) -> Option<&str> {
        self.replacement.as_ref()?.received_at.as_deref()
    }

    /// The relayed reply to request `id`.
    pub(crate) fn reply(&self, id: &str) -> Option<&Reply> {
        self.replacement
            .iter()
            .flat_map(|replacement| &replacement.replies)
            .map(|relayed| &relayed.message)
            .find(|message| message.id.as_deref() == Some(id))
    }

    /// The health the server holds, when it is online and fresh.
    pub(crate) fn fresh_health(&self) -> Result<&Health, NotReady> {
        let replacement = self.replacement()?;
        let health = replacement.health.as_ref().ok_or(NotReady::NoHealth)?;
        if !replacement.online {
            return Err(NotReady::Offline);
        }
        if replacement.stale {
            return Err(NotReady::Stale(replacement.age_ms));
        }
        Ok(health)
    }

    /// Whether fresh health shows `build` (lowercase) running on bank 1: a
    /// one-line report of it.
    pub(crate) fn health_matches(&self, build: &str) -> Result<String, NotReady> {
        let health = self.fresh_health()?;

        let reported = health.build.to_ascii_lowercase();
        if reported != build {
            let short = reported.get(..12).filter(|s| !s.is_empty());
            return Err(NotReady::Build(short.unwrap_or("none").into()));
        }
        if health.bank != Some(1) {
            return Err(NotReady::Bank(health.bank));
        }

        let faults = if health.faults.is_empty() {
            "none".into()
        } else {
            health.faults.join(", ")
        };
        let mut report = format!(
            "Health: build {} on bank 1, seq {}, outputs {}, faults {faults}",
            &build[..build.len().min(12)],
            shown(&health.seq),
            health.outputs.as_deref().unwrap_or("?"),
        );
        if let Some(clock) = &health.clock {
            let valid = if clock.unix_ms.is_some() {
                "valid"
            } else {
                "not set"
            };
            report += &format!(", clock {valid} (syncs {})", clock.syncs);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NEW: &str = "8a13e59efa15e56b5f5e32825909dc45e381a2027abc65d8de6658c40b302447";
    const OLD: &str = "5b8433a4f795cb2178c7a38a45087a6727e7a2621fc557d146cff607556f991e";

    fn status(health: Value, online: bool, stale: bool) -> LocalStatus {
        serde_json::from_value(json!({"replacement": {
            "health": health, "online": online, "stale": stale, "age_ms": 5}}))
        .unwrap()
    }

    #[test]
    fn health_must_show_the_build_on_bank_1_online_and_fresh() {
        let good = json!({"build": NEW.to_uppercase(), "bank": 1, "seq": 3, "outputs": "sensing",
                          "faults": ["watchdog", "flash_hook"], "clock": {"syncs": 0}});
        assert_eq!(
            status(good.clone(), true, false)
                .health_matches(NEW)
                .unwrap(),
            "Health: build 8a13e59efa15 on bank 1, seq 3, outputs sensing, faults watchdog, flash_hook, clock not set (syncs 0)"
        );

        let mut bank0 = good.clone();
        bank0["bank"] = json!(0);
        let mut old = good.clone();
        old["build"] = json!(OLD);
        let unconfigured: LocalStatus =
            serde_json::from_value(json!({"replacement": null})).unwrap();
        for (status, reason) in [
            (unconfigured, NotReady::NoRole),
            (status(Value::Null, false, true), NotReady::NoHealth),
            (
                status(old, true, false),
                NotReady::Build("5b8433a4f795".into()),
            ),
            (status(bank0, true, false), NotReady::Bank(Some(0))),
            (status(good.clone(), false, false), NotReady::Offline),
            (status(good, true, true), NotReady::Stale(Some(5))),
        ] {
            assert_eq!(status.health_matches(NEW).unwrap_err(), reason);
        }
        assert_eq!(
            NotReady::Stale(Some(5)).to_string(),
            "health is stale (age 5 ms)"
        );
    }

    #[test]
    fn a_document_without_freshness_is_stale() {
        let status: LocalStatus = serde_json::from_value(
            json!({"replacement": {"health": {"build": NEW}, "online": true}}),
        )
        .unwrap();
        assert_eq!(status.fresh_health().unwrap_err(), NotReady::Stale(None));
    }

    #[test]
    fn requests_carry_their_op_and_only_the_fields_set() {
        let body = |request: &Request| serde_json::to_value(request).unwrap();

        assert_eq!(body(&Request::Ping), json!({"op": "ping"}));
        assert_eq!(
            body(&Request::Settings(SettingsChange::default())),
            json!({"op": "settings"})
        );
        assert_eq!(
            body(&Request::Settings(SettingsChange {
                lights: Some(false),
                volume: Some(35),
                ..SettingsChange::default()
            })),
            json!({"op": "settings", "lights": false, "volume": 35})
        );
        assert_eq!(
            body(&Request::Wifi(WifiChange {
                clear: true,
                ..WifiChange::default()
            })),
            json!({"op": "wifi", "clear": true})
        );
    }

    #[test]
    fn replies_keep_unread_fields_and_set_their_settings_again() {
        let reply: Reply = serde_json::from_value(json!({"id": "a", "op": "settings", "ok": true,
            "pending": false, "lights": 1, "sound": 0, "volume": 40, "writable": true}))
        .unwrap();

        assert_eq!(
            reply.settings(),
            SettingsChange {
                lights: Some(true),
                sound: Some(false),
                volume: Some(40),
            }
        );
        assert!(reply.shows(&reply.settings()));
        assert!(!reply.shows(&SettingsChange {
            sound: Some(true),
            ..SettingsChange::default()
        }));
        assert_eq!(serde_json::to_value(&reply).unwrap()["pending"], false);
    }
}
