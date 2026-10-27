//! The feeder's stock firmware: the verified backup the server keeps, its
//! export, its upload or read from the feeder, and "Restore stock".
//! Anyone reads its state; the rest is an owner's.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use wellfed_api::{FeederId, Install, Problem, StockBackup, StockRestore, StockState};
use wellfed_service::ReleaseAdded;

use super::{ApiResult, IdPath, JsonBody, TAG_FEEDERS};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

/// A stock image's bytes: the vendor's OTA container, or the bank's
/// contents inside it.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
#[expect(
    dead_code,
    reason = "only its schema is used: the body travels as bytes"
)]
pub(crate) struct StockFile(Vec<u8>);

/// The feeder's kept stock backup, what the feeder says of its banks now,
/// and its running or last restore.
#[utoipa::path(
    get,
    path = "/feeders/{id}/stock",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = StockState),
        (status = 404, body = Problem, description = "not_found: no such feeder"),
    ),
)]
pub(crate) async fn stock(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::Read>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Json<StockState>> {
    Ok(Json(state.service.stock(&caller, id).await?))
}

/// The kept backup, checked again and sent byte for byte: the vendor's
/// stock OTA container, as read from this feeder or uploaded for it.
#[utoipa::path(
    get,
    path = "/feeders/{id}/stock/backup",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 200, body = StockFile, content_type = "application/octet-stream"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: no such feeder; no_stock_backup: none kept, or the kept file no longer verifies"),
    ),
)]
pub(crate) async fn export_stock(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<Response> {
    let (backup, file) = state.service.stock_file(id).await?;
    let name = format!("attachment; filename=\"stock-{}.bin\"", backup.version);
    let mut response = file.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(name) = HeaderValue::from_str(&name) {
        headers.insert(header::CONTENT_DISPOSITION, name);
    }
    Ok(response)
}

/// Upload a copy of this feeder's stock (the one its installer kept, or
/// an export). It is checked against the pinned stock image and kept; a
/// copy kept already is never replaced.
#[utoipa::path(
    post,
    path = "/feeders/{id}/stock/upload",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    request_body(content = StockFile, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = StockBackup, description = "verified and kept"),
        (status = 200, body = StockBackup, description = "the server kept this copy already"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: no such feeder"),
        (status = 409, body = Problem, description = "stock_differs: another copy is kept"),
        (status = 413, body = Problem, description = "too_large"),
        (status = 422, body = Problem, description = "stock_refused: not the pinned stock image"),
    ),
)]
pub(crate) async fn upload_stock(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(file, _): WithRejection<Bytes, ApiError>,
) -> ApiResult<(StatusCode, Json<StockBackup>)> {
    let kept = state.service.upload_stock(id, file.into()).await?;
    Ok(kept_status(kept))
}

/// Read bank 1 from the feeder and keep it, while the replacement runs
/// from bank 0 with stock still in bank 1 (before promotion). The request
/// waits for the read (about 1.3 MB); a verified copy kept already is
/// answered without reading.
#[utoipa::path(
    post,
    path = "/feeders/{id}/stock/read",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    responses(
        (status = 201, body = StockBackup, description = "read, verified and kept"),
        (status = 200, body = StockBackup, description = "the server kept a verified copy already"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: no such feeder"),
        (status = 409, body = Problem, description = "feeder_refused: `forbidden` (the server's key is not an owner's), `unavailable` (the feeder does not serve bank 1: not running from bank 0, or a build without the read), `busy`; install_running: an install runs on this feeder; stock_differs: another copy is kept"),
        (status = 422, body = Problem, description = "stock_refused: bank 1 is not the pinned stock image; nothing was kept"),
        (status = 503, body = Problem, description = "feeder_offline"),
        (status = 504, body = Problem, description = "outcome_unknown: the feeder stopped answering the read; nothing was kept, and asking again reads it again"),
    ),
)]
pub(crate) async fn read_stock(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(Path(id), _): IdPath,
) -> ApiResult<(StatusCode, Json<StockBackup>)> {
    let kept = state.service.read_stock(id).await?;
    Ok(kept_status(kept))
}

/// Restore stock: install the kept backup into bank 1, as the owner signed
/// it. It runs in the background as an install does, one at a time per
/// server: this answers once it has started, and the event stream carries
/// its progress and its end as `install` updates. Nothing is offered again
/// on the owner's behalf. Once activated the feeder runs stock, which the
/// server no longer reaches; a feeder that stopped answering after the
/// offer ends `unreachable` with `offered`, and only the feeder can tell.
#[utoipa::path(
    post,
    path = "/feeders/{id}/stock/restore",
    tag = TAG_FEEDERS,
    params(("id" = FeederId, Path)),
    request_body = StockRestore,
    responses(
        (status = 202, body = Install, description = "started"),
        (status = 400, body = Problem, description = "invalid: a malformed body"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 404, body = Problem, description = "not_found: no such feeder; no_stock_backup: none kept, or the kept file no longer verifies"),
        (status = 409, body = Problem, description = "install_running: an install or restore is running; feeder_refused: `forbidden` (the server's key is not an owner's), `unavailable` (a build that takes no updates)"),
        (status = 422, body = Problem, description = "restore_mismatch: the manifest does not offer the kept backup as the stock image; nothing was sent"),
        (status = 503, body = Problem, description = "feeder_offline"),
    ),
)]
pub(crate) async fn restore_stock(
    State(state): State<AppState>,
    Allowed(caller, _): Allowed<need::ServerData>,
    WithRejection(Path(id), _): IdPath,
    WithRejection(Json(request), _): JsonBody<StockRestore>,
) -> ApiResult<(StatusCode, Json<Install>)> {
    let install = state.service.restore_stock(&caller, id, request).await?;
    Ok((StatusCode::ACCEPTED, Json(install)))
}

fn kept_status((backup, added): (StockBackup, ReleaseAdded)) -> (StatusCode, Json<StockBackup>) {
    let status = match added {
        ReleaseAdded::New => StatusCode::CREATED,
        ReleaseAdded::Known => StatusCode::OK,
    };
    (status, Json(backup))
}
