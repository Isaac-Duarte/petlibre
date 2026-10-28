//! What every server test builds, in one place, so a test file holds only
//! what it asserts.
//!
//! - [`TestService`]: the service on a temporary store, with a fixed
//!   [`Clock`](wellfed_service::Clock) and a seeded
//!   [`Random`](wellfed_service::Random) ([`fixed`]), stopped and reopened
//!   on the same database.
//! - [`TestServer`]: the HTTP server in process on a localhost port, in
//!   front of a [`TestService`], reached through
//!   [`wellfed_server_client::Client`] so the tests and the CLI use one
//!   client of the routes.
//! - [`release`]: release files signed with a test key the simulator and
//!   the server can both be told to trust.
//! - [`feeder`]: the simulated feeder over real TCP, its owner's key and
//!   the invites the server redeems.
//!
//! The simulator runs on the current [`LocalSet`], so a test that starts
//! one runs inside [`local`]. Nothing here touches hardware.
//!
//! This crate is a dev-dependency only. It panics on a broken setup, with
//! the step that failed, as a test would.
#![forbid(unsafe_code)]

pub mod feeder;
pub mod fixed;
pub mod release;
mod server;
mod service;

use std::fmt::Debug;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use tokio::task::LocalSet;
use uuid::Uuid;
use wellfed_api::{Credentials, ErrorCode, Role, Setup, User, UserId};
use wellfed_server_client::StatusCode;

pub use server::TestServer;
pub use service::{TempStore, TestService, config};

/// How long a test waits for something the server does on its own.
pub const WAIT: Duration = Duration::from_secs(10);

/// Run `test` on a current-thread runtime inside a [`LocalSet`], where the
/// simulator's tasks run.
pub fn local(test: impl Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a test runtime builds");
    runtime.block_on(LocalSet::new().run_until(test));
}

/// Wait until `check` holds, polling, for at most [`WAIT`]. Panics naming
/// `what` when it never does.
pub async fn until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let polled = tokio::time::timeout(WAIT, async {
        while !check().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(polled.is_ok(), "{what}: not within {WAIT:?}");
}

/// The household's first owner, the user a service-level write comes
/// from.
pub fn ada() -> User {
    owner_user(1, "ada")
}

/// The first owner's login.
pub fn ada_login() -> Credentials {
    Credentials {
        name: "ada".into(),
        password: "correct horse".into(),
    }
}

/// The first owner's setup, in the zone the household's feeders keep.
pub fn ada_setup() -> Setup {
    let Credentials { name, password } = ada_login();
    Setup {
        name,
        password,
        zone: ZONE.into(),
    }
}

/// The address a service test logs in from.
pub const HERE: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// The household's zone at setup.
pub const ZONE: &str = "Europe/Berlin";

/// A household owner with id `n`.
pub fn owner_user(n: u128, name: &str) -> User {
    User {
        id: UserId(Uuid::from_u128(n)),
        name: name.into(),
        role: Role::Owner,
        expires: None,
    }
}

/// Assert that the server refused with `status` and `code`, as a typed
/// `application/problem+json` body whose own `status` agrees.
#[track_caller]
pub fn refused<T: Debug>(
    result: Result<T, wellfed_server_client::Error>,
    status: StatusCode,
    code: ErrorCode,
) {
    refused_for("the request", result, status, code);
}

/// [`refused`], naming the request in the failure: one row of a table.
#[track_caller]
pub fn refused_for<T: Debug>(
    request: &str,
    result: Result<T, wellfed_server_client::Error>,
    status: StatusCode,
    code: ErrorCode,
) {
    match result {
        Err(wellfed_server_client::Error::Problem {
            status: got,
            problem,
            ..
        }) => {
            assert_eq!(
                (got, problem.code),
                (status, code),
                "{request}: {problem:?}"
            );
            assert_eq!(problem.status, status.as_u16(), "{request}: {problem:?}");
        }
        other => panic!("{request}: expected {status} {code:?}, got {other:?}"),
    }
}
