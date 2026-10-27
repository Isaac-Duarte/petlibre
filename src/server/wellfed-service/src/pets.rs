//! The household's pets: their names, species, birth dates and daily
//! targets, their weight logs, and the feeders that serve them. Pets and
//! feeders are many-to-many: a feeder several pets share has its intake
//! split between them as an estimate, since it cannot tell who ate. The
//! links keep their history, so a past feed is split among the pets the
//! feeder served then.

use jiff::Timestamp;
use wellfed_api::{
    FeederId, NewPet, NewWeight, PageQuery, Pet, PetId, PetLink, PetPatch, Weight, WeightId,
    WeightPage,
};

use crate::page::{self, after_weight, weight_cursor};
use crate::{Invalid, OwnerDataError, Result, Service};

const KCAL_MAX: u32 = 10_000;
/// A large dog, with room to spare.
const WEIGHT_MAX: u32 = 200_000;

impl Service {
    /// Every pet, by name.
    pub async fn pets(&self) -> Result<Vec<Pet>> {
        Ok(self.shared.store.pets().await?)
    }

    pub async fn pet(&self, id: PetId) -> Result<Pet> {
        let pet = self.shared.store.pet(id).await?;
        pet.ok_or_else(|| OwnerDataError::PetNotFound.into())
    }

    pub async fn create_pet(&self, request: NewPet) -> Result<Pet> {
        let pet = Pet {
            id: PetId(self.shared.new_id()?),
            name: pet_name(&request.name)?.to_owned(),
            species: request.species,
            birth_date: request.birth_date,
            daily_kcal: request.daily_kcal.map(daily_kcal).transpose()?,
            feeders: Vec::new(),
        };

        self.shared
            .store
            .insert_pet(&pet, self.shared.clock.now())
            .await?;
        Ok(pet)
    }

    pub async fn update_pet(&self, id: PetId, patch: PetPatch) -> Result<Pet> {
        let mut pet = self.pet(id).await?;
        if let Some(name) = patch.name {
            pet.name = pet_name(&name)?.to_owned();
        }
        if let Some(species) = patch.species {
            pet.species = species;
        }
        if let Some(birth_date) = patch.birth_date {
            pet.birth_date = birth_date;
        }
        if let Some(kcal) = patch.daily_kcal {
            pet.daily_kcal = kcal.map(daily_kcal).transpose()?;
        }

        if !self.shared.store.update_pet(&pet).await? {
            return Err(OwnerDataError::PetNotFound.into());
        }
        Ok(pet)
    }

    /// Delete a pet with its weight log and its feeders' links.
    pub async fn delete_pet(&self, id: PetId) -> Result<()> {
        if !self.shared.store.delete_pet(id).await? {
            return Err(OwnerDataError::PetNotFound.into());
        }
        Ok(())
    }

    /// Let `feeder` serve the pet from now; one that already does is left
    /// as it is.
    pub async fn assign_feeder(&self, pet: PetId, feeder: FeederId) -> Result<Pet> {
        self.pet(pet).await?;
        self.row(feeder).await?;

        let now = self.shared.clock.now();
        self.shared.store.assign(pet, feeder, now).await?;
        self.pet(pet).await
    }

    /// Stop `feeder` serving the pet from now; the link stays in its
    /// history.
    pub async fn unassign_feeder(&self, pet: PetId, feeder: FeederId) -> Result<Pet> {
        self.pet(pet).await?;

        let now = self.shared.clock.now();
        if !self.shared.store.unassign(pet, feeder, now).await? {
            return Err(OwnerDataError::NotAssigned.into());
        }
        self.pet(pet).await
    }

    /// The pets the feeder served during `from..to` (`to` exclusive), each
    /// link with the span it ran, in the order they began. A feeder's
    /// intake is split among the pets it served when it fed.
    pub async fn feeder_pets(
        &self,
        feeder: FeederId,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<PetLink>> {
        self.row(feeder).await?;
        Ok(self.shared.store.feeder_links(feeder, from, to).await?)
    }

    /// A page of the pet's weight log, oldest reading first.
    pub async fn weights(&self, pet: PetId, query: PageQuery) -> Result<WeightPage> {
        let limit = page::limit(&query)?;
        let after = after_weight(&query)?;
        self.pet(pet).await?;

        let rows = self.shared.store.weights(pet, after, limit + 1).await?;
        let (items, next) = page::split(rows, limit, |weight| weight_cursor(weight.at, weight.id));
        Ok(WeightPage { items, next })
    }

    /// Log a weight, taken now unless the request says when; a reading
    /// from the past is a vet's, but one from the future is a mistake.
    pub async fn add_weight(&self, pet: PetId, request: NewWeight) -> Result<Weight> {
        if !(1..=WEIGHT_MAX).contains(&request.grams) {
            return Err(Invalid::Weight.into());
        }
        let now = self.shared.clock.now();
        if request.at.is_some_and(|at| at > now) {
            return Err(Invalid::WeightTime.into());
        }
        self.pet(pet).await?;

        let mut weight = Weight {
            id: WeightId(self.shared.new_id()?),
            at: request.at.unwrap_or(now),
            grams: request.grams,
        };
        weight.at = self.shared.store.insert_weight(pet, &weight).await?;
        Ok(weight)
    }

    pub async fn delete_weight(&self, pet: PetId, weight: WeightId) -> Result<()> {
        self.pet(pet).await?;

        if !self.shared.store.delete_weight(pet, weight).await? {
            return Err(OwnerDataError::WeightNotFound.into());
        }
        Ok(())
    }
}

fn pet_name(name: &str) -> Result<&str, Invalid> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(Invalid::PetName);
    }
    Ok(name)
}

fn daily_kcal(kcal: u32) -> Result<u32, Invalid> {
    if !(1..=KCAL_MAX).contains(&kcal) {
        return Err(Invalid::DailyKcal);
    }
    Ok(kcal)
}
