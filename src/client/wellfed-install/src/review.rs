//! The replacement an install puts on the feeder: a frozen review's
//! `build-manifest.json` (the startup identity the steps wait for, and how
//! the image is provisioned) and its `image.bin` (the OTA container stock
//! downloads, and bank 0 later writes into bank 1). The image server checks
//! the identity against the review's digests before serving.

use std::path::{Path, PathBuf};

use wellfed_formats::manifest::{BuildManifest, Provisioning};
use wellfed_signing::Key;

use crate::Error;

/// A review, read once before the feeder is touched.
#[derive(Debug, Clone)]
pub struct Review {
    pub(crate) directory: PathBuf,
    pub(crate) build: String,
    pub(crate) manifest: BuildManifest,
    pub(crate) container: Vec<u8>,
}

impl Review {
    /// The review in `directory`; refused without a 64-hex startup
    /// identity.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("build-manifest.json");
        let manifest: BuildManifest =
            wellfed_formats::load(&path).map_err(|source| Error::ReviewManifest { source })?;
        let id = manifest.startup_identity.as_deref().unwrap_or_default();
        wellfed_formats::require(
            id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            || Error::NoStartupIdentity,
        )?;
        let build = id.to_ascii_lowercase();

        let image = directory.join("image.bin");
        let container = std::fs::read(&image).map_err(|source| Error::Read {
            path: image,
            source,
        })?;
        Ok(Self {
            directory: directory.to_path_buf(),
            build,
            manifest,
            container,
        })
    }

    /// Check the review in `directory` as the image server does before it
    /// serves one: the image and the build manifest against the review's
    /// digests. A platform that carries its image with it runs this before
    /// offering to install it.
    pub fn check_served(directory: &Path) -> Result<(), Error> {
        wellfed_stock::ota::image::Build::load(directory)
            .map(|_| ())
            .map_err(|error| Error::Images(error.into()))
    }

    /// The startup identity, 64 lowercase hex.
    pub fn build(&self) -> &str {
        &self.build
    }

    pub fn manifest(&self) -> &BuildManifest {
        &self.manifest
    }

    pub fn provisioning(&self) -> Provisioning {
        self.manifest.provisioning
    }

    /// Whether the image builds `key` in, so the feeder trusts it without
    /// an enrollment.
    pub fn builds_in(&self, key: &Key) -> bool {
        let public = wellfed_formats::hex::encode(&key.public());
        self.manifest
            .ota_public_key
            .as_deref()
            .is_some_and(|built| built.eq_ignore_ascii_case(&public))
    }
}

#[cfg(test)]
mod tests {
    use wellfed_formats::credentials::KeyScope;

    use super::*;

    #[test]
    fn a_review_without_a_build_identity_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("build-manifest.json"), "{}").unwrap();
        assert!(matches!(
            Review::open(dir.path()),
            Err(Error::NoStartupIdentity)
        ));
    }

    /// A key the image builds in needs no enrollment.
    #[test]
    fn a_key_is_built_in_only_when_the_manifest_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-key.json");
        wellfed_signing::keygen(&path, KeyScope::Owner).unwrap();
        let key = Key::load(&path).unwrap();

        let mut review = Review {
            directory: dir.path().to_path_buf(),
            build: "ab".repeat(32),
            manifest: BuildManifest::default(),
            container: Vec::new(),
        };
        assert!(!review.builds_in(&key));
        review.manifest.ota_public_key =
            Some(wellfed_formats::hex::encode(&key.public()).to_uppercase());
        assert!(review.builds_in(&key));
        review.manifest.ota_public_key = Some("ab".repeat(32));
        assert!(!review.builds_in(&key));
    }
}
