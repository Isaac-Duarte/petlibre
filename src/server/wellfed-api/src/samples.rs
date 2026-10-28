//! The live samples the server keeps: raw as the feeder sent them, and
//! hourly rollups that outlive the raw rows.
//!
//! Each sample carries one number, the figure its metric is charted by
//! ([`sample_number`]), which the server decodes once as it stores the
//! sample. Rollups are kept over that number per UTC hour.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use wellfed_model::{Battery, MetricKind, SampleValue};

/// The version of [`sample_number`]. Bump it whenever what a sample's
/// number is changes, so the server decodes every stored sample again and
/// rebuilds the rollups the raw rows still cover.
pub const SAMPLE_NUMBER_VERSION: u32 = 1;

/// The one number a sample is charted and rolled up by, or `None` when it
/// has none:
///
/// | Metric | Number |
/// | --- | --- |
/// | `power` | `battery_mv` |
/// | `rssi` | `dbm` |
/// | `heap` | `free` |
/// | `loop` | `p99_us` |
/// | `gateway` | `round_trip_ms` |
/// | `wifi` | `ap_changes` |
/// | `supply` | the battery's millivolts, while it reads one |
pub fn sample_number(value: &SampleValue) -> Option<i64> {
    match *value {
        SampleValue::Power { battery_mv, .. } => battery_mv.map(i64::from),
        SampleValue::Rssi { dbm } => Some(i64::from(dbm)),
        SampleValue::Heap { free, .. } => Some(i64::from(free)),
        SampleValue::Loop { p99_us, .. } => Some(i64::from(p99_us)),
        SampleValue::Gateway { round_trip_ms } => round_trip_ms.map(i64::from),
        SampleValue::Wifi { ap_changes, .. } => Some(i64::from(ap_changes)),
        SampleValue::Supply { battery, .. } => match battery {
            Battery::Reading { mv } => Some(i64::from(mv)),
            Battery::Absent
            | Battery::Implausible { .. }
            | Battery::Unread
            | Battery::Stale { .. } => None,
        },
    }
}

/// How finely samples are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Each sample as the feeder sent it, while the owner's raw retention
    /// keeps it.
    #[default]
    Raw,
    /// One rollup per UTC hour, kept forever.
    Hour,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SampleQuery {
    pub metric: MetricKind,
    /// Samples received at or after this time; from the oldest when
    /// absent. An hour's rollup is in when its hour starts in the range.
    pub from: Option<Timestamp>,
    /// Samples received before this time; up to the newest when absent.
    pub to: Option<Timestamp>,
    /// `raw` when absent.
    pub resolution: Option<Resolution>,
    /// The `next` of the previous page, as it came.
    pub after: Option<String>,
    /// At most this many (1-1000, default 100).
    pub limit: Option<u32>,
}

/// One sample as the server received it, with its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RawSample {
    pub received: Timestamp,
    /// The feeder's uptime when it measured.
    pub uptime_ms: u32,
    pub value: SampleValue,
    /// The metric's number ([`sample_number`]); `None` when it has none.
    pub number: Option<i64>,
}

/// One UTC hour of a metric's numbers. Samples with no number are not
/// counted, and an hour with none is left out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HourSample {
    /// The hour's start.
    pub hour: Timestamp,
    pub count: u32,
    pub min: i64,
    pub max: i64,
    pub mean: f64,
}

/// A page of one feeder's samples of one metric, oldest first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum SamplePage {
    Raw {
        items: Vec<RawSample>,
        /// The `after` for the next page; `None` at the end.
        next: Option<String>,
    },
    Hour {
        items: Vec<HourSample>,
        /// The `after` for the next page; `None` at the end.
        next: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_without_its_figure_has_no_number() {
        let power = |battery_mv| SampleValue::Power {
            adapter: true,
            battery_mv,
        };

        assert_eq!(sample_number(&power(Some(3_900))), Some(3_900));
        assert_eq!(sample_number(&power(None)), None);
        assert_eq!(sample_number(&SampleValue::Rssi { dbm: -61 }), Some(-61));
        let supply = |battery| SampleValue::Supply {
            adapter: Some(true),
            battery,
            below_floor: None,
        };
        assert_eq!(
            sample_number(&supply(Battery::Reading { mv: 3_700 })),
            Some(3_700)
        );
        assert_eq!(sample_number(&supply(Battery::Implausible { mv: 9 })), None);
    }
}
