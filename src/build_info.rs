//! Source identity recorded by `build.rs`; trial dispatch requires a clean commit.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Build {
    pub commit: &'static str,
    pub clean: bool,
}

pub const BUILD: Build = Build {
    commit: env!("PETLIBRO_BUILD_COMMIT"),
    clean: matches!(env!("PETLIBRO_BUILD_CLEAN").as_bytes(), b"true"),
};

/// Test-only escape hatch: integration tests run from uncommitted trees.
pub const ALLOW_UNREVIEWED_ENV: &str = "PETLIBRO_ALLOW_UNREVIEWED_OTA_BUILD";

impl Build {
    pub fn reviewable(self) -> bool {
        self.clean && self.commit != "unknown"
    }
}
