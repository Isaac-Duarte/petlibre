//! A board under `src/firmware/devices/`, chosen by its directory name and
//! described by its `device.json` ([`DeviceConfig`]).
//!
//! The device decides the platform a build runs on and where its outputs go
//! (`private/firmware-build/<device>/<profile>`). `device.json` and every
//! file it names that a profile reads are identity inputs.

use std::fs;
use std::path::PathBuf;

use crate::catalog::DeviceEntry;
use wellfed_formats::device::{DeviceConfig, Stock};

use crate::platform::Platform;
use crate::{Error, Profile, Repo, Result, checked_name, require};

/// The directory every device lives in.
const DEVICES: &str = "src/firmware/devices";

/// The stock evidence of a device, as checkout paths.
#[derive(Debug, Clone)]
pub(crate) struct StockPaths {
    /// The verified stock OTA image: the pad table and layout reference.
    pub(crate) image: PathBuf,
    pub(crate) layout: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Device {
    name: String,
    config: DeviceConfig,
}

impl Device {
    /// `src/firmware/devices/<name>`, if this checkout has it.
    pub fn new(repo: &Repo, name: &str) -> Result<Self> {
        let name = checked_name("Device", name)?;
        let path = repo.path(&format!("{DEVICES}/{name}/device.json"));
        require(
            path.is_file(),
            format!("No device named {name} under {DEVICES}/"),
        )?;

        let config: DeviceConfig = wellfed_formats::load(&path)?;
        require(
            config.schema_version == 1,
            format!("Device {name} has an unknown schema version"),
        )?;
        Ok(Self {
            name: name.to_string(),
            config,
        })
    }

    /// Every device in this checkout, by name.
    pub fn all(repo: &Repo) -> Result<Vec<Self>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(repo.path(DEVICES))? {
            let entry = entry?;
            if entry.path().join("device.json").is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();

        names.iter().map(|name| Self::new(repo, name)).collect()
    }

    /// The named device, or the checkout's only one. With several devices a
    /// build must name its board rather than have one guessed.
    pub fn resolve(repo: &Repo, name: Option<&str>) -> Result<Self> {
        if let Some(name) = name {
            return Self::new(repo, name);
        }

        let mut devices = Self::all(repo)?;
        match devices.len() {
            1 => Ok(devices.remove(0)),
            0 => Err(Error::Refused(format!("No devices under {DEVICES}/"))),
            _ => {
                let names: Vec<&str> = devices.iter().map(|d| d.name()).collect();
                Err(Error::Refused(format!(
                    "Name the device with --device: {}",
                    names.join(", ")
                )))
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its root-relative `device.json`.
    pub(crate) fn config_path(&self) -> String {
        format!("{DEVICES}/{}/device.json", self.name)
    }

    pub(crate) fn platform(&self, repo: &Repo) -> Platform {
        Platform::new(repo, &self.config.platform)
    }

    /// Fetch (once) and verify its platform's pinned SDK and toolchain;
    /// returns the toolchain directory a build uses.
    pub fn bootstrap(&self, repo: &Repo) -> Result<PathBuf> {
        self.platform(repo).bootstrap(repo)
    }

    /// Its stock image and layout, which the stock-layout stage needs.
    pub(crate) fn stock(&self, repo: &Repo) -> Result<StockPaths> {
        let Some(Stock { image, layout }) = &self.config.stock else {
            return Err(Error::Refused(format!(
                "Device {} has no verified stock image and layout yet",
                self.name
            )));
        };
        Ok(StockPaths {
            image: repo.path(image),
            layout: repo.path(layout),
        })
    }

    /// Where a profile's build of this device goes.
    pub fn output(&self, repo: &Repo, profile: Profile) -> PathBuf {
        repo.private()
            .join("firmware-build")
            .join(&self.name)
            .join(profile.name())
    }

    /// What `wellfed-build list` prints for it.
    pub fn entry(&self) -> DeviceEntry {
        DeviceEntry {
            name: self.name.clone(),
            model: self.config.model.clone(),
            platform: self.config.platform.name().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_only_device_is_the_default() {
        let repo = Repo::here().unwrap();
        let devices = Device::all(&repo).unwrap();
        assert!(devices.iter().any(|d| d.name() == "plaf103"));
        if devices.len() == 1 {
            assert_eq!(Device::resolve(&repo, None).unwrap().name(), "plaf103");
        }
    }

    #[test]
    fn unknown_and_unsafe_names_are_refused() {
        let repo = Repo::here().unwrap();
        assert!(Device::new(&repo, "no-such-board").is_err());
        assert!(Device::new(&repo, "../plaf103").is_err());
    }
}
