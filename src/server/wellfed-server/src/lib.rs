//! The owner's server ([API design]): the HTTP API over
//! `wellfed-service`. This crate holds the routes, authentication, the
//! event stream and error mapping; it never sees SQL or the device
//! protocol.
//!
//! [`app`] is the whole API as an axum router, so tests drive it in
//! process; [`openapi`] is its document, committed as `openapi.json`.
//!
//! [API design]: ../../../../docs/api/README.md
#![forbid(unsafe_code)]

mod auth;
mod error;
mod extract;
mod routes;

pub use auth::Hosts;
use axum::Router;
use wellfed_service::Service;
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
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
        (name = routes::TAG_FEEDERS, description = "Adopted feeders, their settings, lock and history"),
    ),
)]
struct ApiDoc;

fn router() -> OpenApiRouter<AppState> {
    use routes::{auth, events, feeders, stream};

    let api = OpenApiRouter::new()
        .routes(routes!(auth::setup_state, auth::setup))
        .routes(routes!(auth::session, auth::login, auth::logout))
        .routes(routes!(feeders::feeders, feeders::adopt))
        .routes(routes!(feeders::feeder))
        .routes(routes!(feeders::settings, feeders::put_settings))
        .routes(routes!(feeders::lock))
        .routes(routes!(events::events))
        .routes(routes!(stream::stream));
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::health))
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
    router
        .layer(axum::middleware::from_fn(auth::same_origin))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::known_host,
        ))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The API's OpenAPI document.
pub fn openapi() -> utoipa::openapi::OpenApi {
    router().split_for_parts().1
}
