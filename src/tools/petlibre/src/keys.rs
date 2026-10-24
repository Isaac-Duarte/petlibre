//! The owner's client key file (`petlibre_formats::owner`): created once
//! from the system's random source, then reused for every feeder.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use petlibre_core::noise::Keypair;
use petlibre_formats::hex;
use petlibre_formats::owner::{EnrolledFeeder, OwnerKeyFile, VERSION};
use petlibre_formats::private::{self, SYSTEM_RANDOM};

/// The label a new key file gets unless `--label` names one.
pub(crate) const DEFAULT_LABEL: &str = "petlibre-cli";

pub(crate) struct OwnerKey {
    path: PathBuf,
    file: OwnerKeyFile,
    keypair: Keypair,
}

impl OwnerKey {
    /// The key at `path`, or a new one written there. `label` replaces the
    /// stored label when given.
    pub(crate) fn open(path: &Path, label: Option<&str>, random: &Path) -> Result<Self> {
        let mut file = if path.exists() {
            let file: OwnerKeyFile = petlibre_formats::load(path)
                .with_context(|| format!("read the key file {}", path.display()))?;
            ensure!(
                file.version == VERSION,
                "{}: key file version {} (this tool reads {VERSION})",
                path.display(),
                file.version
            );
            file
        } else {
            let secret: [u8; 32] =
                private::random(random).context("read the system random source")?;
            OwnerKeyFile {
                version: VERSION,
                secret: hex::encode(&secret),
                label: DEFAULT_LABEL.into(),
                feeders: Vec::new(),
            }
        };
        if let Some(label) = label {
            file.label = label.into();
        }

        let secret = hex::decode::<32>(&file.secret)
            .with_context(|| format!("{}: the secret is not 32 bytes of hex", path.display()))?;
        let key = Self {
            path: path.to_path_buf(),
            keypair: Keypair::from_secret(secret),
            file,
        };
        key.save()?;
        Ok(key)
    }

    /// The default random source.
    pub(crate) fn system() -> &'static Path {
        Path::new(SYSTEM_RANDOM)
    }

    pub(crate) fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    pub(crate) fn label(&self) -> &str {
        &self.file.label
    }

    /// The feeder to talk to: the one whose identity starts with `prefix`,
    /// or the only one this key knows.
    pub(crate) fn feeder(&self, prefix: Option<&str>) -> Result<[u8; 32]> {
        let matching: Vec<&EnrolledFeeder> = self
            .file
            .feeders
            .iter()
            .filter(|f| prefix.is_none_or(|p| f.identity.starts_with(p)))
            .collect();
        let [feeder] = matching.as_slice() else {
            anyhow::bail!(
                "{}: {} feeders match; pass --feeder with a prefix of one",
                self.path.display(),
                matching.len()
            );
        };
        hex::decode::<32>(&feeder.identity).context("a feeder identity in the key file")
    }

    /// Note a feeder that enrolled this key.
    pub(crate) fn remember(&mut self, identity: &[u8; 32]) -> Result<()> {
        let identity = hex::encode(identity);
        if !self.file.feeders.iter().any(|f| f.identity == identity) {
            self.file.feeders.push(EnrolledFeeder { identity });
        }
        self.save()
    }

    fn save(&self) -> Result<()> {
        private::write_json(&self.path, &self.file)
            .with_context(|| format!("write the key file {}", self.path.display()))
    }
}

/// The identity's first 8 bytes, as the feeder logs them (`PLID1`).
pub(crate) fn fingerprint(identity: &[u8; 32]) -> String {
    hex::encode(&identity[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_created_once_and_remembers_its_feeders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.json");
        let random = dir.path().join("random");
        std::fs::write(&random, [9u8; 32]).unwrap();

        let mut key = OwnerKey::open(&path, None, &random).unwrap();
        assert_eq!(key.label(), DEFAULT_LABEL);
        let public = *key.keypair().public();
        key.remember(&[5; 32]).unwrap();
        key.remember(&[5; 32]).unwrap();

        std::fs::write(&random, [1u8; 32]).unwrap();
        let again = OwnerKey::open(&path, Some("laptop"), &random).unwrap();
        assert_eq!(*again.keypair().public(), public, "the same key");
        assert_eq!(again.label(), "laptop");
        assert_eq!(again.file.feeders.len(), 1);
        assert_eq!(fingerprint(&[0xab; 32]), "abababababababab");
    }

    #[test]
    fn a_damaged_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("owner.json");
        std::fs::write(
            &path,
            r#"{"version":1,"secret":"00","label":"x","feeders":[]}"#,
        )
        .unwrap();
        assert!(OwnerKey::open(&path, None, &path).is_err());
    }
}
