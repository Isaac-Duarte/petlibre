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
//! [API design]: ../../../../docs/server/design.md
#![forbid(unsafe_code)]

mod auth;
pub mod config;
mod error;
mod extract;
pub mod jobs;
pub mod key;
mod proxy;
mod request_id;
mod routes;

use std::sync::Arc;

pub use auth::Hosts;
use axum::Router;
use axum::extract::DefaultBodyLimit;
pub use proxy::{ProxyRange, TrustedProxies};
use tokio_util::sync::CancellationToken;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use wellfed_service::{RELEASE_MAX, Service};

/// Every request body this API takes is a few hundred bytes, but for a
/// release or stock upload, which takes up to [`RELEASE_MAX`] (a stock
/// image, its container's header and a full bank, is shorter).
const BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) struct AppState {
    service: Service,
    /// Ends every open event stream.
    shutdown: CancellationToken,
    hosts: Hosts,
    /// Whose forwarded headers name the client and its scheme.
    proxies: Arc<TrustedProxies>,
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
        (name = routes::TAG_TOKENS, description = "Bearer tokens for phones and the CLI, each user's own"),
        (name = routes::TAG_HOUSEHOLD, description = "The household's name, units, time format and zone"),
        (name = routes::TAG_FEEDERS, description = "Adopted feeders, their settings, lock, feeds and history"),
        (name = routes::TAG_HEALTH, description = "Liveness for anyone, readiness for an owner"),
        (name = routes::TAG_COMMANDS, description = "The command journal: every write sent through the server"),
        (name = routes::TAG_AUDIT, description = "Who did what through the server: the journal and account changes"),
        (name = routes::TAG_RELEASES, description = "Verified releases kept for installs: fetched or uploaded"),
        (name = routes::TAG_PETS, description = "Pets, their weight logs and the feeders that serve them"),
        (name = routes::TAG_FOODS, description = "Foods, the food each feeder serves over time, and refills"),
        (name = routes::TAG_BACKUPS, description = "Copies of the server's database, kept in its data directory"),
        (name = routes::TAG_EXPORT, description = "Everything the server holds as one archive, and its import onto a fresh server"),
    ),
)]
struct ApiDoc;

fn router() -> OpenApiRouter<AppState> {
    use routes::{
        admin, audit, auth, backups, clients, commands, diagnostics, events, export, feeders,
        feeds, foods, health, household, installs, pets, releases, samples, stock, stream, tokens,
        users,
    };

    let small = OpenApiRouter::new()
        .routes(routes!(auth::setup_state, auth::setup))
        .routes(routes!(
            auth::session,
            auth::login,
            auth::change_password,
            auth::logout
        ))
        .routes(routes!(users::users, users::create_user))
        .routes(routes!(users::update_user, users::delete_user))
        .routes(routes!(tokens::tokens, tokens::create_token))
        .routes(routes!(tokens::revoke_token))
        .routes(routes!(household::household, household::update_household))
        .routes(routes!(feeders::feeders, feeders::adopt))
        .routes(routes!(feeders::feeder, admin::remove))
        .routes(routes!(feeders::settings, feeders::put_settings))
        .routes(routes!(feeders::lock))
        .routes(routes!(feeders::reset_desiccant))
        .routes(routes!(clients::clients))
        .routes(routes!(clients::revoke_client))
        .routes(routes!(clients::keep_client))
        .routes(routes!(clients::invite_phone))
        .routes(routes!(clients::phones))
        .routes(routes!(admin::put_wifi))
        .routes(routes!(admin::reboot))
        .routes(routes!(admin::play))
        .routes(routes!(feeds::feed))
        .routes(routes!(feeds::cancel))
        .routes(routes!(installs::install, installs::install_state))
        .routes(routes!(stock::stock))
        .routes(routes!(stock::export_stock))
        .routes(routes!(stock::read_stock))
        .routes(routes!(stock::restore_stock))
        .routes(routes!(events::events))
        .routes(routes!(samples::samples))
        .routes(routes!(diagnostics::trace))
        .routes(routes!(diagnostics::stats))
        .routes(routes!(diagnostics::log))
        .routes(routes!(commands::commands))
        .routes(routes!(commands::command))
        .routes(routes!(audit::audit))
        .routes(routes!(stream::stream))
        .routes(routes!(health::status))
        .routes(routes!(releases::releases))
        .routes(routes!(releases::check_releases))
        .routes(routes!(pets::pets, pets::create_pet))
        .routes(routes!(pets::pet, pets::update_pet, pets::delete_pet))
        .routes(routes!(pets::assign_feeder, pets::unassign_feeder))
        .routes(routes!(pets::weights, pets::add_weight))
        .routes(routes!(pets::delete_weight))
        .routes(routes!(foods::foods, foods::create_food))
        .routes(routes!(foods::food, foods::update_food, foods::delete_food))
        .routes(routes!(foods::feeder_food, foods::set_feeder_food))
        .routes(routes!(foods::food_history))
        .routes(routes!(foods::refills, foods::add_refill))
        .routes(routes!(foods::delete_refill))
        .routes(routes!(backups::backups, backups::create_backup))
        .routes(routes!(export::export))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT));
    let uploads = OpenApiRouter::new()
        .routes(routes!(releases::upload_release))
        .routes(routes!(stock::upload_stock))
        .layer(DefaultBodyLimit::max(RELEASE_MAX));
    // An import streams into its transaction as it arrives, so it is held
    // to no size: it runs only on a fresh server, where no data is at stake.
    let imports = OpenApiRouter::new().routes(routes!(export::import));
    let api = small.merge(uploads).merge(imports);
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health::health))
        .nest(wellfed_api::PREFIX, api)
}

/// The API, serving `service` under any IP address, `localhost` and
/// `hosts`, and believing the forwarded headers of `proxies` alone.
/// Cancelling `shutdown` ends the open event streams, exports and imports,
/// so a graceful shutdown can finish.
///
/// Serve it with its connections' addresses
/// (`into_make_service_with_connect_info::<SocketAddr>`): a login is
/// limited by its client's address, and refused without one.
pub fn app(
    service: Service,
    shutdown: CancellationToken,
    hosts: Hosts,
    proxies: TrustedProxies,
) -> Router {
    let state = AppState {
        service,
        shutdown,
        hosts,
        proxies: Arc::new(proxies),
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
