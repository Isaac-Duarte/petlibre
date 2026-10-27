//! Everything the server holds, exported as one archive, and imported onto
//! a fresh server.

use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::WithRejection;
use futures_util::{StreamExt, TryStreamExt};
use wellfed_api::{ExportFormat, ExportQuery, Imported, Problem};

use super::{ApiResult, TAG_EXPORT, insert};
use crate::AppState;
use crate::auth::{Allowed, need};
use crate::error::ApiError;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// An archive's bytes: a tar, as the body documents it.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
#[expect(
    dead_code,
    reason = "only its schema is used: the archive streams as bytes"
)]
pub(crate) struct Archive(Vec<u8>);

/// Everything the server holds, as a tar streamed from one snapshot of
/// the database: `manifest.json` (the format version, the server's version,
/// and each table's row count, size and SHA-256), then a file per table,
/// `.jsonl` or `.csv`. No password, token or session hashes and no feeder
/// keys. A body cut short means the export failed; the manifest's sizes and
/// hashes show it.
#[utoipa::path(
    get,
    path = "/export",
    tag = TAG_EXPORT,
    params(ExportQuery),
    responses(
        (status = 200, body = Archive, content_type = "application/x-tar"),
        (status = 400, body = Problem, description = "invalid: an unknown format"),
        (status = 403, body = Problem, description = "forbidden: owners only"),
        (status = 409, body = Problem, description = "export_running: another export is being written"),
    ),
)]
pub(crate) async fn export(
    State(state): State<AppState>,
    _: Allowed<need::ServerData>,
    WithRejection(Query(query), _): WithRejection<Query<ExportQuery>, ApiError>,
) -> ApiResult<Response> {
    let chunks = state.service.export(query.format, VERSION).await?;
    // The body ends when the server stops, so a slow reader never holds
    // up its graceful shutdown; the cut tar shows it.
    let stopping = Box::pin(state.shutdown.clone().cancelled_owned());
    let chunks = chunks.take_until(stopping);

    let name = match query.format {
        ExportFormat::Json => "wellfed-export.tar",
        ExportFormat::Csv => "wellfed-export-csv.tar",
    };
    let mut response = Body::from_stream(chunks).into_response();
    insert(&mut response, header::CONTENT_TYPE, "application/x-tar");
    let disposition = format!("attachment; filename=\"{name}\"");
    insert(&mut response, header::CONTENT_DISPOSITION, &disposition);
    Ok(response)
}

/// Restore a JSON export onto this fresh server, in one transaction, read
/// as it is uploaded. Like setup, it needs no login and runs only while no
/// account exists; it logs no one in. Imported accounts have no password
/// until the host's `wellfed-server reset-owner` gives an owner one, and
/// imported feeders are archived until they are adopted again.
#[utoipa::path(
    post,
    path = "/import",
    tag = TAG_EXPORT,
    request_body(content = Archive, content_type = "application/x-tar"),
    responses(
        (status = 201, body = Imported),
        (status = 409, body = Problem, description = "import_closed: an account exists"),
        (status = 422, body = Problem, description = "archive_invalid: not an archive this server imports, or not what its manifest says; nothing was imported"),
    ),
)]
pub(crate) async fn import(
    State(state): State<AppState>,
    body: Body,
) -> ApiResult<(StatusCode, Json<Imported>)> {
    // The upload ends when the server stops: cut short, nothing is kept.
    let stopping = Box::pin(state.shutdown.clone().cancelled_owned());
    let upload = body
        .into_data_stream()
        .map_err(axum::Error::into_inner)
        .take_until(stopping);
    let imported = state.service.import(upload).await?;
    Ok((StatusCode::CREATED, Json(imported)))
}
