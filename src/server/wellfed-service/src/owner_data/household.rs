//! The household: its name, how it shows weights and times, and its zone.
//!
//! The zone is the feeders' too. It is stored as its canonical IANA name;
//! the mirror sets each feeder's clock in it and gives each feeder its
//! POSIX rule, so a feeder keeps local time across DST with the server
//! down. A zone no rule describes is refused, since a feeder could not keep
//! it.

use wellfed_api::{AuditAction, ErrorCode, Household, HouseholdPatch};
use wellfed_mirror::{Zone, ZoneError};
use tracing::error;

use crate::{Caller, Invalid, Result, Service};

/// The household's settings.
#[derive(Debug, thiserror::Error)]
pub enum HouseholdError {
    #[error("the household is made at setup")]
    NotSetUp,
    #[error(transparent)]
    Zone(ZoneError),
}

impl HouseholdError {
    /// A real zone no POSIX rule describes is `zone_unsupported`; a name the
    /// tzdb does not know is a plain `invalid`.
    pub(crate) fn code(&self) -> ErrorCode {
        match self {
            Self::NotSetUp => ErrorCode::NotFound,
            Self::Zone(ZoneError::Unknown(_)) => ErrorCode::Invalid,
            Self::Zone(
                ZoneError::NoRule(_) | ZoneError::Unsupported { .. } | ZoneError::Irregular { .. },
            ) => ErrorCode::ZoneUnsupported,
        }
    }
}

impl Service {
    pub async fn household(&self) -> Result<Household> {
        let household = self.shared.store.household().await?;
        household.ok_or_else(|| HouseholdError::NotSetUp.into())
    }

    /// Change the household, as `caller` (an owner). A new zone goes to
    /// every feeder: the online ones now, the others when they reconnect.
    pub async fn update_household(
        &self,
        caller: &Caller,
        patch: HouseholdPatch,
    ) -> Result<Household> {
        let _one_at_a_time = self.owner_data.household.lock().await;
        let mut household = self.household().await?;
        if let Some(name) = patch.name {
            household.name = household_name(&name)?.to_owned();
        }
        if let Some(units) = patch.units {
            household.units = units;
        }
        if let Some(format) = patch.time_format {
            household.time_format = format;
        }
        let zone = patch
            .zone
            .map(|name| Zone::new(&name, self.shared.clock.now()))
            .transpose()
            .map_err(HouseholdError::Zone)?;
        if let Some(zone) = &zone {
            household.zone = zone.name().to_owned();
        }

        self.shared.store.set_household(&household).await?;
        if let Some(zone) = zone {
            self.mirror.set_zone(zone);
        }
        let action = AuditAction::HouseholdChanged {
            household: household.clone(),
        };
        self.record(caller, action).await;
        Ok(household)
    }

    /// Give the mirror the stored zone, at start. A zone the tzdb no
    /// longer knows leaves the feeders' clocks in UTC until an owner picks
    /// another; it is logged rather than stopping the server.
    pub(crate) async fn load_zone(&self) -> Result<()> {
        let Some(household) = self.shared.store.household().await? else {
            return Ok(());
        };
        match Zone::stored(&household.zone, self.shared.clock.now()) {
            Ok(zone) => self.mirror.set_zone(zone),
            Err(refused) => error!("the household's zone: {refused}; feeders' clocks stay in UTC"),
        }
        Ok(())
    }
}

fn household_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::HouseholdName.into());
    }
    Ok(name)
}
