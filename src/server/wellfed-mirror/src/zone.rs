//! The household's zone as the feeders keep it: its offsets for each clock
//! sync, and its POSIX rule for the feeder's time settings, so a feeder
//! follows DST with the server down.
//!
//! Both come from the tzdb bundled with `wellfed-client`, as the app's and
//! the CLI's do, so every client sends a feeder the same rule for a name
//! whatever the host's own zone files say. The rule is checked against the
//! bundled zone only: that is the zone the syncs' offsets come from too.

use jiff::Timestamp;
use jiff::tz::TimeZone;
pub use wellfed_client::time::RuleError as ZoneError;

/// A zone the feeders can be given.
#[derive(Clone, Debug)]
pub struct Zone {
    name: String,
    tz: TimeZone,
    /// `None` for a stored zone whose rule no longer fits (a tzdb update):
    /// its syncs still carry the offsets, but no rule is pushed.
    rule: Option<String>,
}

impl Zone {
    /// The zone `name` (any spelling the tzdb knows), refused when no POSIX
    /// rule describes it from `now` on.
    pub fn new(name: &str, now: Timestamp) -> Result<Self, ZoneError> {
        let (name, tz) = bundled(name)?;
        let rule = wellfed_client::time::rule(&name, now, None)?;
        Ok(Self {
            name,
            tz,
            rule: Some(rule),
        })
    }

    /// The zone as stored, accepted with no rule when it no longer has
    /// one, so a feeder keeps the zone's offsets; refused only when the
    /// tzdb no longer knows the name.
    pub fn stored(name: &str, now: Timestamp) -> Result<Self, ZoneError> {
        match Self::new(name, now) {
            Err(ZoneError::Unknown(name)) => Err(ZoneError::Unknown(name)),
            Err(error) => {
                tracing::warn!("{error}; the feeders keep their clock syncs' offsets only");
                let (name, tz) = bundled(name)?;
                Ok(Self {
                    name,
                    tz,
                    rule: None,
                })
            }
            Ok(zone) => Ok(zone),
        }
    }

    /// The canonical IANA name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The POSIX rule the feeders keep.
    pub fn rule(&self) -> Option<&str> {
        self.rule.as_deref()
    }

    pub fn tz(&self) -> &TimeZone {
        &self.tz
    }
}

/// The canonical name and the zone the bundled tzdb holds for `name`.
fn bundled(name: &str) -> Result<(String, TimeZone), ZoneError> {
    let unknown = || ZoneError::Unknown(name.to_owned());
    let (canonical, tzif) = jiff_tzdb::get(name).ok_or_else(unknown)?;
    let tz = TimeZone::tzif(canonical, tzif).map_err(|_| unknown())?;
    Ok((canonical.to_owned(), tz))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-26T00:00:00Z.
    const NOW: i64 = 1_793_059_200;

    fn now() -> Timestamp {
        Timestamp::from_second(NOW).unwrap()
    }

    #[test]
    fn a_zone_has_its_canonical_name_and_rule() {
        let zone = Zone::new("america/denver", now()).unwrap();
        assert_eq!(zone.name(), "America/Denver");
        assert_eq!(zone.rule(), Some("MST7MDT,M3.2.0,M11.1.0"));
        assert_eq!(zone.tz().to_offset(now()).seconds(), -6 * 3600);
    }

    #[test]
    fn an_unknown_or_irregular_zone_is_refused_by_name() {
        assert_eq!(
            Zone::new("Mars/Olympus", now()).err(),
            Some(ZoneError::Unknown("Mars/Olympus".into()))
        );
        assert!(Zone::stored("Mars/Olympus", now()).is_err());

        // A few zones list irregular changes years ahead, which no POSIX
        // rule says; which ones depends on the tzdb release.
        let irregular = jiff_tzdb::available().find_map(|name| match Zone::new(name, now()) {
            Err(ZoneError::Irregular { name: named, .. }) => Some((name, named)),
            _ => None,
        });
        let Some((name, named)) = irregular else {
            panic!("the bundled tzdb lists no irregular zone to refuse");
        };
        assert!(named.eq_ignore_ascii_case(name), "{named} names {name}");
        let stored = Zone::stored(name, now()).unwrap();
        assert_eq!(stored.rule(), None, "kept for its offsets, with no rule");
    }
}
