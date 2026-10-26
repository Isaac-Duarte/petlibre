//! The serving binary's source identity, which the status document and
//! `/health` report. `build.rs` records it; trial dispatch requires a clean
//! commit of every build input.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Build {
    pub commit: &'static str,
    pub clean: bool,
}

impl Build {
    pub fn reviewable(self) -> bool {
        self.clean && self.commit != "unknown"
    }
}

/// This build's identity, as `build.rs` recorded it.
pub const BUILD: Build = Build {
    commit: env!("WELLFED_BUILD_COMMIT"),
    clean: matches!(env!("WELLFED_BUILD_CLEAN").as_bytes(), b"true"),
};

/// Test-only escape hatch: integration tests run from uncommitted trees.
pub const ALLOW_UNREVIEWED_ENV: &str = "WELLFED_ALLOW_UNREVIEWED_OTA_BUILD";
