//! `/pets`: the household's pets, their weight logs and which feeders
//! serve them. Owners write them; everyone reads them.

use jiff::Timestamp;
use jiff::civil::Date;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{FeederId, PetId, WeightId, present};

/// What kind of animal a pet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Species {
    Cat,
    Dog,
    Other,
}

impl Species {
    /// The stored spelling, the same as the JSON one.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Cat => "cat",
            Self::Dog => "dog",
            Self::Other => "other",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        [Self::Cat, Self::Dog, Self::Other]
            .into_iter()
            .find(|species| species.code() == code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Pet {
    pub id: PetId,
    /// 1-64 characters.
    pub name: String,
    pub species: Species,
    pub birth_date: Option<Date>,
    /// The calories a day the pet should eat (1-10000 kcal).
    pub daily_kcal: Option<u32>,
    /// The feeders that serve this pet. A feeder may serve several pets,
    /// and its intake is then split between them as an estimate.
    pub feeders: Vec<FeederId>,
}

/// A span of time a feeder served a pet: from `from` until `to`, when it
/// was unassigned, or still when `to` is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PetLink {
    pub pet: PetId,
    pub feeder: FeederId,
    pub from: Timestamp,
    pub to: Option<Timestamp>,
}

/// A new pet; it starts with no feeders and no weights.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewPet {
    pub name: String,
    pub species: Species,
    #[serde(default)]
    pub birth_date: Option<Date>,
    #[serde(default)]
    pub daily_kcal: Option<u32>,
}

/// A change to a pet; an absent field is left as it is, and `null` clears
/// an optional one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PetPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub species: Option<Species>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<Date>, nullable)]
    pub birth_date: Option<Option<Date>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<u32>, nullable)]
    pub daily_kcal: Option<Option<u32>>,
}

/// One entry of a pet's weight log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Weight {
    pub id: WeightId,
    /// When the pet weighed this.
    pub at: Timestamp,
    pub grams: u32,
}

/// A new weight-log entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NewWeight {
    /// 1-200000.
    pub grams: u32,
    /// When it was weighed, for a reading taken earlier (a vet's); now when
    /// absent.
    #[serde(default)]
    pub at: Option<Timestamp>,
}

/// A page of a weight log, oldest reading first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WeightPage {
    pub items: Vec<Weight>,
    /// The `after` for the next page; `None` at the end.
    pub next: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn species_spellings_round_trip() {
        for species in [Species::Cat, Species::Dog, Species::Other] {
            assert_eq!(Species::from_code(species.code()), Some(species));
            assert_eq!(
                serde_json::to_string(&species).unwrap(),
                format!("\"{}\"", species.code())
            );
        }
    }

    #[test]
    fn an_absent_field_is_kept_and_null_clears_it() {
        let parse = |json| serde_json::from_str::<PetPatch>(json).unwrap();

        assert_eq!(parse("{}"), PetPatch::default());
        let cleared = parse(r#"{"birth_date":null,"daily_kcal":null}"#);
        assert_eq!(
            (cleared.birth_date, cleared.daily_kcal),
            (Some(None), Some(None))
        );
        let set = parse(r#"{"birth_date":"2020-05-01","daily_kcal":250}"#);
        assert_eq!(set.birth_date, Some(Some(jiff::civil::date(2020, 5, 1))));
        assert_eq!(set.daily_kcal, Some(Some(250)));
    }
}
