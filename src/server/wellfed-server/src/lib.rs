//! The owner's server ([API design]): the HTTP API over
//! `wellfed-service`. This crate holds the routes, authentication, the
//! event stream and error mapping; it never sees SQL or the device
//! protocol.
//!
//! [`app`] is the whole API as an axum router, so tests drive it in
//! process; [`openapi`] is its document, committed as `openapi.json`.
//! [`config`] and [`key`] read what the server starts with from its data
//! directory: the settings file and the sealing key.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod auth;
pub mod config;
mod error;
mod extract;
pub mod key;
mod request_id;
mod routes;

pub use auth::Hosts;
use axum::Router;
use wellfed_service::Service;
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// Every request body this API takes is a few hundred bytes.
const BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct AppState {
    service: Service,
    /// Ends every open event stream.
    shutdown: CancellationToken,
    hosts: Hosts,
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Wellfed server",
        description = "The owner's server: mirrors each feeder's history and serves it to the web UI and the app. Unstable until the first release.",
        license(name = "MIT"),
    ),
    tags(
        (name = routes::TAG_AUTH, description = "First-owner setup and login"),
        (name = routes::TAG_USERS, description = "The household's accounts, managed by owners"),
        (name = routes::TAG_HOUSEHOLD, description = "The household's name, units, time format and zone"),
        (name = routes::TAG_FEEDERS, description = "Adopted feeders, their settings, lock and history"),
        (name = routes::TAG_HEALTH, description = "Liveness for anyone, readiness for an owner"),
        (name = routes::TAG_COMMANDS, description = "The command journal: every write sent through the server"),
    ),
)]
struct ApiDoc;

fn router() -> OpenApiRouter<AppState> {
    use routes::{auth, commands, events, feeders, health, household, stream, users};

    let api = OpenApiRouter::new()
        .routes(routes!(auth::setup_state, auth::setup))
        .routes(routes!(
            auth::session,
            auth::login,
            auth::change_password,
            auth::logout
        ))
        .routes(routes!(users::users, users::create_user))
        .routes(routes!(users::update_user, users::delete_user))
        .routes(routes!(household::household, household::update_household))
        .routes(routes!(feeders::feeders, feeders::adopt))
        .routes(routes!(feeders::feeder))
        .routes(routes!(feeders::settings, feeders::put_settings))
        .routes(routes!(feeders::lock))
        .routes(routes!(events::events))
        .routes(routes!(commands::commands))
        .routes(routes!(commands::command))
        .routes(routes!(stream::stream))
        .routes(routes!(health::status));
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health::health))
        .nest(wellfed_api::PREFIX, api)
}

/// The API, serving `service` under any IP address, `localhost` and
/// `hosts`. Cancelling `shutdown` ends the open event streams, so a
/// graceful shutdown can finish.
pub fn app(service: Service, shutdown: CancellationToken, hosts: Hosts) -> Router {
    let state = AppState {
        service,
        shutdown,
        hosts,
    };
    let (router, _) = router().split_for_parts();

    // Each layer wraps the ones above it. A request's own id is screened
    // and then the id is set, both outside the trace layer, so its span
    // carries the id. The id is copied onto the answer inside the span, so
    // a refusal from the host, origin or size checks carries it too.
    router
        .layer(axum::middleware::from_fn(auth::same_origin))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::known_host,
        ))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(PropagateRequestIdLayer::new(request_id::HEADER))
        .layer(TraceLayer::new_for_http().make_span_with(request_id::span))
        .layer(SetRequestIdLayer::new(request_id::HEADER, MakeRequestUuid))
        .layer(axum::middleware::map_request(request_id::screen))
        .with_state(state)
}

/// The API's OpenAPI document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    router().split_for_parts().1
}
