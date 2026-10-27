//! Foods, the food each feeder serves over time, and the refills owners
//! record. Everyone reads them; owners write them.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum_extra::extract::WithRejection;
use wellfed_api::{
    FeederFood, FeederId, Food, FoodChoice, FoodHistoryPage, FoodId, FoodPatch, FoodPeriod,
    NewFood, NewRefill, PageQuery, Problem, Refill, RefillId, RefillPage,
};

use super::{ApiResult, IdPath, JsonBody, TAG_FOODS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

type FoodPath = WithRejection<Path<FoodId>, ApiError>;
type PageParams = WithRejection<Query<PageQuery>, ApiError>;

/// Every food, by brand and name.
#[utoipa::path(
    get,
    path = "/foods",
    tag = TAG_FOODS,
    responses((status = 200, body = Vec<Food>), (status = 401, body = Problem)),
)]
pub(crate) async fn foods(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
) -> ApiResult<Json<Vec<Food>>> {
    Ok(Json(state.service.foods().await?))
}

/// Add a food. A cup is 110 g unless `grams_per_cup` says otherwise.
#[utoipa::path(
    post,
    path = "/foods",
    tag = TAG_FOODS,
    request_body = NewFood,
    responses(
        (status = 201, body = Food),
        (status = 400, body = Problem, description = "invalid: a brand, name or amount out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
    ),
)]
pub(crate) async fn create_food(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Json(request), _): JsonBody<NewFood>,
) -> ApiResult<(StatusCode, Json<Food>)> {
    let food = state.service.create_food(request).await?;
    Ok((StatusCode::CREATED, Json(food)))
}

#[utoipa::path(
    get,
    path = "/foods/{id}",
    tag = TAG_FOODS,
    params(("id" = FoodId, Path)),
    responses(
        (status = 200, body = Food),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn food(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): FoodPath,
) -> ApiResult<Json<Food>> {
    Ok(Json(state.service.food(id).await?))
}

/// Change a food. Feeders' histories keep naming it, so the change applies
/// to past feeds too: a new recipe is a new food.
#[utoipa::path(
    patch,
    path = "/foods/{id}",
    tag = TAG_FOODS,
    params(("id" = FoodId, Path)),
    request_body = FoodPatch,
    responses(
        (status = 200, body = Food),
        (status = 400, body = Problem, description = "invalid: a brand, name or amount out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn update_food(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): FoodPath,
    WithRejection(Json(patch), _): JsonBody<FoodPatch>,
) -> ApiResult<Json<Food>> {
    Ok(Json(state.service.update_food(id, patch).await?))
}

/// Delete a food that no feeder has served and no refill names.
#[utoipa::path(
    delete,
    path = "/foods/{id}",
    tag = TAG_FOODS,
    params(("id" = FoodId, Path)),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "food_in_use: a feeder's history or a refill names it"),
    ),
)]
pub(crate) async fn delete_food(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): FoodPath,
) -> ApiResult<StatusCode> {
    state.service.delete_food(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The food the feeder serves now, and its newest refill.
#[utoipa::path(
    get,
    path = "/feeders/{id}/food",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = FeederFood),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn feeder_food(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<FeederFood>> {
    Ok(Json(state.service.feeder_food(id).await?))
}

/// Serve another food from now on; past feeds keep the food they served.
/// Choosing the food the feeder already serves keeps its period.
#[utoipa::path(
    put,
    path = "/feeders/{id}/food",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path)),
    request_body = FoodChoice,
    responses(
        (status = 200, body = FoodPeriod),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: the feeder or the food"),
    ),
)]
pub(crate) async fn set_feeder_food(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Json(choice), _): JsonBody<FoodChoice>,
) -> ApiResult<Json<FoodPeriod>> {
    Ok(Json(state.service.set_feeder_food(id, choice.food).await?))
}

/// The foods the feeder has served, oldest first, each with when it
/// started and ended.
#[utoipa::path(
    get,
    path = "/feeders/{id}/food/history",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path), PageQuery),
    responses(
        (status = 200, body = FoodHistoryPage),
        (status = 400, body = Problem, description = "invalid: a bad `after` or `limit`"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn food_history(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Query(query), _): PageParams,
) -> ApiResult<Json<FoodHistoryPage>> {
    Ok(Json(state.service.food_history(id, query).await?))
}

/// The feeder's refills, in the order they were recorded.
#[utoipa::path(
    get,
    path = "/feeders/{id}/refills",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path), PageQuery),
    responses(
        (status = 200, body = RefillPage),
        (status = 400, body = Problem, description = "invalid: a bad `after` or `limit`"),
        (status = 401, body = Problem),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn refills(
    State(state): State<AppState>,
    _: Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Query(query), _): PageParams,
) -> ApiResult<Json<RefillPage>> {
    Ok(Json(state.service.refills(id, query).await?))
}

/// Record a refill made now: a weighed amount, or a full bag of the
/// feeder's current food.
#[utoipa::path(
    post,
    path = "/feeders/{id}/refills",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path)),
    request_body = NewRefill,
    responses(
        (status = 201, body = Refill),
        (status = 400, body = Problem, description = "invalid: an amount out of range"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
        (status = 409, body = Problem, description = "no_current_food: a full bag on a feeder with no food set"),
    ),
)]
pub(crate) async fn add_refill(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::OwnerData>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Json(request), _): JsonBody<NewRefill>,
) -> ApiResult<(StatusCode, Json<Refill>)> {
    let refill = state.service.add_refill(&caller.user, id, request).await?;
    Ok((StatusCode::CREATED, Json(refill)))
}

/// Remove a refill recorded by mistake.
#[utoipa::path(
    delete,
    path = "/feeders/{id}/refills/{refill}",
    tag = TAG_FOODS,
    params(("id" = FeederId, Path), ("refill" = RefillId, Path)),
    responses(
        (status = 204),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem),
    ),
)]
pub(crate) async fn delete_refill(
    State(state): State<AppState>,
    _: Allowed<need::OwnerData>,
    WithRejection(Path((id, refill)), _): WithRejection<Path<(FeederId, RefillId)>, ApiError>,
) -> ApiResult<StatusCode> {
    state.service.delete_refill(id, refill).await?;
    Ok(StatusCode::NO_CONTENT)
}
