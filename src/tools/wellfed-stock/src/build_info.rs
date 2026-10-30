//! The serving binary's source identity, which the status document and
//! `/health` report. `build.rs` records it; trial dispatch requires a clean
//! commit of every build input, which covers the crates that decide and
//! send the install request (this one, `wellfed-client`, `wellfed-install`
//! and the app's bridge), whichever process links them.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Build {
    pub commit: &'static str,
    pub clean: bool,
}

/// A build that may not dispatch stock's install: its sources cannot be
/// reviewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "dispatching stock's install requires a binary built from a clean commit (this build: {commit}, clean: {clean})"
)]
pub struct Unreviewed {
    pub commit: &'static str,
    pub clean: bool,
}

impl Build {
    pub fn reviewable(self) -> bool {
        self.clean && self.commit != "unknown"
    }

    /// Whether this build may dispatch stock's install: reviewable, or
    /// run under the tests' escape hatch ([`ALLOW_UNREVIEWED_ENV`]).
    pub fn require_reviewed(self) -> Result<(), Unreviewed> {
        if self.reviewable() || std::env::var_os(ALLOW_UNREVIEWED_ENV).is_some() {
            return Ok(());
        }
        Err(Unreviewed {
            commit: self.commit,
            clean: self.clean,
        })
    }
}

/// This build's identity, as `build.rs` recorded it.
pub const BUILD: Build = Build {
    commit: env!("WELLFED_BUILD_COMMIT"),
    clean: matches!(env!("WELLFED_BUILD_CLEAN").as_bytes(), b"true"),
};

/// Test-only escape hatch: integration tests run from uncommitted trees.
pub const ALLOW_UNREVIEWED_ENV: &str = "WELLFED_ALLOW_UNREVIEWED_OTA_BUILD";
