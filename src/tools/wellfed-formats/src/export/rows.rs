//! One type per exported table, each field a column under its database
//! name, so the store reads and writes them with checked queries. Integers
//! are SQLite's `i64`; ids are UUIDs and other bytes lowercase hex.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ArchiveError;
use super::csv::field_names;

/// Every table and its row type, in import order.
macro_rules! tables {
    ($($variant:ident => $name:literal: $row:ident,)+) => {
        /// An exported table.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Table {
            $($variant,)+
        }

        impl Table {
            /// Every table, in the order an archive holds them and an
            /// import inserts them: each after the tables its rows name.
            pub const ALL: &[Self] = &[$(Self::$variant,)+];

            /// Its name in the database and the archive.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            /// Its columns, in the row type's order.
            pub fn columns(self) -> &'static [&'static str] {
                match self {
                    $(Self::$variant => field_names::<$row>(),)+
                }
            }
        }

        /// One row of any table.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum Row {
            $($variant($row),)+
        }

        impl Row {
            pub const fn table(&self) -> Table {
                match self {
                    $(Self::$variant(_) => Table::$variant,)+
                }
            }

            /// Read one line of `table`'s JSON file.
            pub fn parse(table: Table, line: &[u8]) -> Result<Self, ArchiveError> {
                let row = match table {
                    $(Table::$variant => serde_json::from_slice(line).map(Self::$variant),)+
                };
                row.map_err(|error| ArchiveError::Row(table, error))
            }

            pub(super) fn value(&self) -> Result<Value, ArchiveError> {
                let value = match self {
                    $(Self::$variant(row) => serde_json::to_value(row),)+
                };
                value.map_err(|error| ArchiveError::Encode(self.table(), error.into()))
            }
        }

        $(
            impl From<$row> for Row {
                fn from(row: $row) -> Self {
                    Self::$variant(row)
                }
            }
        )+
    };
}

tables! {
    Users => "users": User,
    Household => "household": Household,
    Feeders => "feeders": Feeder,
    FeederState => "feeder_state": FeederState,
    PhoneKeys => "phone_keys": PhoneKey,
    KeptClients => "kept_clients": KeptClient,
    Events => "events": Event,
    Gaps => "gaps": Gap,
    Samples => "samples": Sample,
    SampleRollups => "sample_rollups": SampleRollup,
    SampleMarks => "sample_marks": SampleMarks,
    ProjectionVersion => "projection_version": ProjectionVersion,
    Feeds => "feeds": Feed,
    LockChanges => "lock_changes": LockChange,
    Boots => "boots": Boot,
    ClockSyncs => "clock_syncs": ClockSync,
    BlockedChanges => "blocked_changes": BlockedChange,
    FeedTraces => "feed_traces": FeedTrace,
    FeederStats => "feeder_stats": FeederStat,
    FoodReadings => "food_readings": FoodReading,
    Commands => "commands": Command,
    Audit => "audit": AuditEntry,
    Installs => "installs": Install,
    Pets => "pets": Pet,
    PetWeights => "pet_weights": PetWeight,
    PetFeeders => "pet_feeders": PetFeeder,
    Foods => "foods": Food,
    FeederFoods => "feeder_foods": FeederFood,
    Refills => "refills": Refill,
}

/// An account, without its password hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    pub created_ms: i64,
    pub expires_ms: Option<i64>,
}

/// The household: one row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Household {
    pub name: String,
    pub units: String,
    pub time_format: String,
    pub zone: String,
}

/// A feeder, without the server's sealed key for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feeder {
    pub id: Uuid,
    #[serde(with = "hex_bytes")]
    pub identity: Vec<u8>,
    pub name: String,
    pub role: String,
    pub address: Option<String>,
    pub synced: Option<i64>,
    pub epoch: Option<i64>,
    pub created_ms: i64,
}

/// A feeder's last status, settings and plans read, as the JSON the
/// server cached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeederState {
    pub feeder_id: Uuid,
    pub status: Option<String>,
    pub status_ms: Option<i64>,
    pub settings: Option<String>,
    pub settings_ms: Option<i64>,
    pub plans: Option<String>,
    pub plans_ms: Option<i64>,
}

/// A phone key the server made an invite for: who it is for, and its
/// revocation while one is pending. `key` is the phone's public key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhoneKey {
    pub feeder_id: Uuid,
    #[serde(with = "hex_bytes")]
    pub key: Vec<u8>,
    pub user_id: Uuid,
    pub token_id: Option<Uuid>,
    pub role: String,
    pub invited_ms: i64,
    pub invite_until_ms: i64,
    pub revoke: Option<String>,
    pub revoke_ms: Option<i64>,
    pub attempt: Option<String>,
}

/// A client the server did not invite that an owner chose to keep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeptClient {
    pub feeder_id: Uuid,
    #[serde(with = "hex_bytes")]
    pub key: Vec<u8>,
    pub kept_ms: i64,
}

/// An event as the feeder recorded it: `body` is its postcard bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub seq: i64,
    pub at_s: Option<i64>,
    pub received_ms: i64,
    pub kind: String,
    #[serde(with = "hex_bytes")]
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub from_seq: i64,
    pub to_seq: i64,
    pub received_ms: i64,
}

/// A live sample: `value` is the metric's postcard bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub id: i64,
    pub feeder_id: Uuid,
    pub received_ms: i64,
    pub uptime_ms: i64,
    pub metric: String,
    pub number: Option<i64>,
    #[serde(with = "hex_bytes")]
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleRollup {
    pub feeder_id: Uuid,
    pub metric: String,
    pub hour_ms: i64,
    pub count: i64,
    pub min: i64,
    pub max: i64,
    pub sum: i64,
}

/// How far the samples are rolled up, and the code that numbered them.
/// One row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleMarks {
    pub rolled: i64,
    pub version: i64,
}

/// The projection code's version the projections were built by. One row,
/// absent before the first build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionVersion {
    pub version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub feed: i64,
    pub seq: i64,
    pub source: String,
    #[serde(with = "hex_option")]
    pub client: Option<Vec<u8>>,
    pub requested: i64,
    pub completed: i64,
    pub outcome: String,
    pub reason: String,
    pub at_s: Option<i64>,
    pub received_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockChange {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub seq: i64,
    pub locked: bool,
    pub source: String,
    #[serde(with = "hex_option")]
    pub client: Option<Vec<u8>>,
    pub at_s: Option<i64>,
    pub received_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boot {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub seq: i64,
    pub bank: i64,
    pub reset_flags: i64,
    pub at_s: Option<i64>,
    pub received_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSync {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub seq: i64,
    pub source: String,
    #[serde(with = "hex_option")]
    pub client: Option<Vec<u8>>,
    pub jump_ms: Option<i64>,
    pub at_s: Option<i64>,
    pub received_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockedChange {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub seq: i64,
    pub reason: Option<String>,
    pub at_s: Option<i64>,
    pub received_ms: i64,
}

/// A feed's trace as the feeder gave it: `summary` as JSON and `body` in
/// the feeder's own encoding, or the feeder's `refusal` as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedTrace {
    pub feeder_id: Uuid,
    pub epoch: i64,
    pub feed: i64,
    pub fetched_ms: i64,
    pub summary: Option<String>,
    pub stored: Option<bool>,
    #[serde(with = "hex_option")]
    pub body: Option<Vec<u8>>,
    pub refusal: Option<String>,
}

/// The latest read of one statistics section, as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeederStat {
    pub feeder_id: Uuid,
    pub section: String,
    pub received_ms: i64,
    pub boot_count: Option<i64>,
    pub stats: String,
}

/// One read of the food section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoodReading {
    pub feeder_id: Uuid,
    pub received_ms: i64,
    pub boot_count: Option<i64>,
    pub present: Option<bool>,
    pub emptied: i64,
    pub refilled: i64,
    pub feeds_since_refill: Option<i64>,
}

/// A command journal entry. `key` is the client's idempotency key and
/// `body_hash` the request's digest: neither opens anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub seq: i64,
    #[serde(with = "hex_bytes")]
    pub key: Vec<u8>,
    pub user_id: Uuid,
    pub token_id: Option<Uuid>,
    pub feeder_id: Uuid,
    pub kind: String,
    #[serde(with = "hex_bytes")]
    pub body_hash: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub request_id: Vec<u8>,
    pub boot_count: Option<i64>,
    pub state: String,
    pub outcome: Option<String>,
    pub feed: Option<i64>,
    pub sent_ms: i64,
    pub updated_ms: i64,
    pub history_epoch: Option<i64>,
    pub history_seq: Option<i64>,
}

/// An audit log entry: an account change (`action`, the API's JSON) or a
/// journal entry's (`command_seq`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEntry {
    pub seq: i64,
    pub at_ms: i64,
    pub user_id: Option<Uuid>,
    pub user_name: Option<String>,
    pub token_id: Option<Uuid>,
    pub action: Option<String>,
    pub command_seq: Option<i64>,
}

/// An install an owner asked for; `failure` is the API's JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Install {
    pub id: Uuid,
    pub feeder_id: Uuid,
    #[serde(with = "hex_bytes")]
    pub release_id: Vec<u8>,
    pub version: i64,
    pub user_id: Uuid,
    pub started_ms: i64,
    pub state: String,
    pub written: i64,
    pub image_len: i64,
    pub failure: Option<String>,
    pub finished_ms: Option<i64>,
    /// What it writes: `release`, or `stock` from the kept backup.
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pet {
    pub id: Uuid,
    pub name: String,
    pub species: String,
    pub birth_date: Option<String>,
    pub daily_kcal: Option<i64>,
    pub created_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PetWeight {
    pub id: Uuid,
    pub pet_id: Uuid,
    pub at_ms: i64,
    pub grams: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PetFeeder {
    pub seq: i64,
    pub pet_id: Uuid,
    pub feeder_id: Uuid,
    pub from_ms: i64,
    pub to_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Food {
    pub id: Uuid,
    pub brand: String,
    pub name: String,
    pub kcal_per_kg: i64,
    pub bag_grams: i64,
    pub grams_per_cup: i64,
    pub created_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeederFood {
    pub seq: i64,
    pub feeder_id: Uuid,
    pub food_id: Uuid,
    pub from_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refill {
    pub seq: i64,
    pub id: Uuid,
    pub feeder_id: Uuid,
    pub at_ms: i64,
    pub grams: i64,
    pub full_bag: bool,
    pub food_id: Option<Uuid>,
    pub user_id: Uuid,
}

/// Bytes as lowercase hex.
mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub(super) fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&crate::hex::encode(bytes))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode(&text).ok_or_else(|| D::Error::custom("not lowercase hex"))
    }

    /// Lowercase only, the one spelling an export writes, so a row reads
    /// back to the same bytes.
    pub(super) fn decode(text: &str) -> Option<Vec<u8>> {
        if !crate::hex::is_lower_hex(text, text.len()) {
            return None;
        }
        crate::hex::decode_vec(text)
    }
}

/// Optional bytes as lowercase hex, or `null`.
mod hex_option {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub(super) fn serialize<S: Serializer>(
        bytes: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => super::hex_bytes::serialize(bytes, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let Some(text) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        let bytes = super::hex_bytes::decode(&text);
        bytes
            .map(Some)
            .ok_or_else(|| D::Error::custom("not lowercase hex"))
    }
}
