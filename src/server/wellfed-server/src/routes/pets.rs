//! The household's pets, their weight logs and the feeders that serve
//! them. Everyone reads them; owners write them.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use wellfed_api::{
    FeederId, IntakeQuery, NewPet, NewWeight, PageQuery, Pet, PetId, PetIntake, PetPatch, Problem,
    Weight, WeightId, WeightPage,
};

use super::{ApiResult, JsonBody, TAG_PETS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

type PetPath = WithRejection<Path<PetId>, ApiError>;

/// Every pet, by name, each with the feeders that serve it.
#[utoipa::path(
    get,
    path = "/pets",
    tag = TAG_PETS,
    responses((status = 200, body = Vec<Pet>), (status = 401, body = Problem)),
)]
pub(crate) async fn pets(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
) -> ApiResult<Json<Vec<Pet>>> {
    Ok(Json(state.service.pets().await?))
}

/// Add a pet. It starts with no feeders and an empty weight log.
#[utoipa::path(
    post,
    path = "/pets",
    tag = TAG_PETS,
    request_body = NewPet,
    responses(
        (status = 201, body = Pet),
        (status = 400, body = Problem, description = "invalid: a name, or a daily target out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn create_pet(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Json(request), _): JsonBody<NewPet>,
) -> ApiResult<(StatusCode, Json<Pet>)> {
    let pet = state.service.create_pet(request).await?;
    Ok((StatusCode::CREATED, Json(pet)))
}

#[utoipa::path(
    get,
    path = "/pets/{id}",
    tag = TAG_PETS,
    params(("id" = PetId, Path)),
    responses(
        (status = 200, body = Pet),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn pet(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): PetPath,
) -> ApiResult<Json<Pet>> {
    Ok(Json(state.service.pet(id).await?))
}

/// Change a pet; `null` clears its birth date or daily target.
#[utoipa::path(
    patch,
    path = "/pets/{id}",
    tag = TAG_PETS,
    params(("id" = PetId, Path)),
    request_body = PetPatch,
    responses(
        (status = 200, body = Pet),
        (status = 400, body = Problem, description = "invalid: a name, or a daily target out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn update_pet(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): PetPath,
    WithRejection(Json(patch), _): JsonBody<PetPatch>,
) -> ApiResult<Json<Pet>> {
    Ok(Json(state.service.update_pet(id, patch).await?))
}

/// Delete a pet, with its weight log and its feeders' links.
#[utoipa::path(
    delete,
    path = "/pets/{id}",
    tag = TAG_PETS,
    params(("id" = PetId, Path)),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn delete_pet(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): PetPath,
) -> ApiResult<StatusCode> {
    state.service.delete_pet(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Let a feeder serve the pet. A feeder that already does is left as it
/// is.
#[utoipa::path(
    put,
    path = "/pets/{id}/feeders/{feeder}",
    tag = TAG_PETS,
    params(("id" = PetId, Path), ("feeder" = FeederId, Path)),
    responses(
        (status = 200, body = Pet),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: the pet or the feeder"),
    ),
)]
pub(crate) async fn assign_feeder(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path((id, feeder)), _): WithRejection<Path<(PetId, FeederId)>, ApiError>,
) -> ApiResult<Json<Pet>> {
    Ok(Json(state.service.assign_feeder(id, feeder).await?))
}

/// Stop a feeder serving the pet.
#[utoipa::path(
    delete,
    path = "/pets/{id}/feeders/{feeder}",
    tag = TAG_PETS,
    params(("id" = PetId, Path), ("feeder" = FeederId, Path)),
    responses(
        (status = 200, body = Pet),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: the pet, or the feeder does not serve it"),
    ),
)]
pub(crate) async fn unassign_feeder(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path((id, feeder)), _): WithRejection<Path<(PetId, FeederId)>, ApiError>,
) -> ApiResult<Json<Pet>> {
    Ok(Json(state.service.unassign_feeder(id, feeder).await?))
}

/// The pet's weight log, oldest reading first.
#[utoipa::path(
    get,
    path = "/pets/{id}/weights",
    tag = TAG_PETS,
    params(("id" = PetId, Path), PageQuery),
    responses(
        (status = 200, body = WeightPage),
        (status = 400, body = Problem, description = "invalid: a bad `after` or `limit`"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn weights(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): PetPath,
    WithRejection(Query(query), _): WithRejection<Query<PageQuery>, ApiError>,
) -> ApiResult<Json<WeightPage>> {
    Ok(Json(state.service.weights(id, query).await?))
}

/// Log a weight, taken now unless `at` says when.
#[utoipa::path(
    post,
    path = "/pets/{id}/weights",
    tag = TAG_PETS,
    params(("id" = PetId, Path)),
    request_body = NewWeight,
    responses(
        (status = 201, body = Weight),
        (status = 400, body = Problem, description = "invalid: a weight out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn add_weight(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): PetPath,
    WithRejection(Json(request), _): JsonBody<NewWeight>,
) -> ApiResult<(StatusCode, Json<Weight>)> {
    let weight = state.service.add_weight(id, request).await?;
    Ok((StatusCode::CREATED, Json(weight)))
}

/// Remove an entry from the pet's weight log.
#[utoipa::path(
    delete,
    path = "/pets/{id}/weights/{weight}",
    tag = TAG_PETS,
    params(("id" = PetId, Path), ("weight" = WeightId, Path)),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn delete_weight(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path((id, weight)), _): WithRejection<Path<(PetId, WeightId)>, ApiError>,
) -> ApiResult<StatusCode> {
    state.service.delete_weight(id, weight).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// What the pet's feeders dispensed for it each day, in the household's
/// zone, against its daily target, and the feeds among them that went
/// wrong. A feeder that served several pets has its feeds split evenly
/// between them, and those days are marked an estimate.
#[utoipa::path(
    get,
    path = "/pets/{id}/intake",
    tag = TAG_PETS,
    params(("id" = PetId, Path), IntakeQuery),
    responses(
        (status = 200, body = PetIntake),
        (status = 400, body = Problem, description = "invalid: `from` after `to`, or more than 366 days"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn intake(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): PetPath,
    WithRejection(Query(query), _): WithRejection<Query<IntakeQuery>, ApiError>,
) -> ApiResult<Json<PetIntake>> {
    Ok(Json(state.service.pet_intake(id, query).await?))
}
