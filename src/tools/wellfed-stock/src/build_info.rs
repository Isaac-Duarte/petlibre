//! The serving binary's source identity, which the status document reports.
//! The binary records it at build time; trial dispatch requires a clean commit.
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
