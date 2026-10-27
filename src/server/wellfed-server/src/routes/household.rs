//! The household: its name, units, time format and zone.

use axum::Json;
use axum::extract::State;
use axum_extra::extract::WithRejection;
use wellfed_api::{Household, HouseholdPatch, Problem};

use super::{ApiResult, JsonBody, TAG_HOUSEHOLD};
use crate::AppState;
use crate::auth::{Allowed, need};

/// The household, as every user's pages show it.
#[utoipa::path(
    get,
    path = "/household",
    tag = TAG_HOUSEHOLD,
    responses((status = 200, body = Household), (status = 401, body = Problem)),
)]
pub(crate) async fn household(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
) -> ApiResult<Json<Household>> {
    Ok(Json(state.service.household().await?))
}

/// Change the household. A new zone goes to every feeder as its POSIX rule:
/// the online ones now, the others when they reconnect.
#[utoipa::path(
    patch,
    path = "/household",
    tag = TAG_HOUSEHOLD,
    request_body = HouseholdPatch,
    responses(
        (status = 200, body = Household),
        (status = 400, body = Problem, description = "invalid: a name, or an unknown zone"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 422, body = Problem, description = "zone_unsupported: no POSIX rule describes the zone"),
    ),
)]
pub(crate) async fn update_household(
    State(state): State<AppState>,
    _: Allowed<need::Household>,
    WithRejection(Json(patch), _): JsonBody<HouseholdPatch>,
) -> ApiResult<Json<Household>> {
    Ok(Json(state.service.update_household(patch).await?))
}
