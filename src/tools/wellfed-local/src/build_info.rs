//! Source identity recorded by `build.rs`; trial dispatch requires a clean commit.
use wellfed_stock::build_info::Build;

pub const BUILD: Build = Build {
    commit: env!("WELLFED_BUILD_COMMIT"),
    clean: matches!(env!("WELLFED_BUILD_CLEAN").as_bytes(), b"true"),
};

/// Test-only escape hatch: integration tests run from uncommitted trees.
pub const ALLOW_UNREVIEWED_ENV: &str = "WELLFED_ALLOW_UNREVIEWED_OTA_BUILD";
