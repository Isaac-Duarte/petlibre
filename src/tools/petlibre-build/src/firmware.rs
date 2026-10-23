//! A firmware crate under `src/firmware/petlibre/`, and what its
//! `[package.metadata.petlibre]` table declares: the devices it builds for,
//! the startup policies of its normal first-boot image and its opt-in image
//! variants.
//!
//! That table is the builder's only firmware-specific knowledge, so a new
//! firmware or variant needs no builder change. `Cargo.toml` is an identity
//! input, so a changed declaration is a new identity.

use std::collections::BTreeMap;
use std::fs;

use crate::catalog::{FirmwareEntry, VariantEntry};
use serde::Deserialize;

use crate::{Error, Repo, Result, checked_name, require};

/// The firmware directory every firmware crate lives in.
const FIRMWARES: &str = "src/firmware/petlibre";

/// The startup policies a firmware's image carries: the manifest records
/// them, and the startup audit checks each against the image's markers, so
/// an unknown value is refused there rather than here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policies {
    pub optics: Option<String>,
    pub motion: Option<String>,
    pub trial: Option<String>,
    pub adc: Option<String>,
}

/// An opt-in image variant: the firmware's Cargo feature of the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    name: String,
    summary: String,
    /// They replace the normal image's policies.
    pub(crate) policies: Policies,
}

impl Variant {
    /// The manifest name, which is also the Cargo feature.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Drives the motor: never built unless named, and its manifest says so.
    pub fn motion(&self) -> bool {
        self.policies.motion.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    package: Package,
    #[serde(default)]
    features: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Debug, Default, Deserialize)]
struct Metadata {
    petlibre: Option<Declaration>,
}

/// `[package.metadata.petlibre]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    devices: Vec<String>,
    #[serde(default)]
    policies: Policies,
    #[serde(default)]
    variants: BTreeMap<String, VariantDeclaration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantDeclaration {
    summary: String,
    #[serde(default)]
    policies: Policies,
}

/// A firmware crate, chosen by its directory name. The set is open (anyone
/// may add a firmware), so this is a checked name rather than an enum. Its
/// Cargo package is `petlibre-fw-<name>`, a KM4 staticlib on `petlibre-sdk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    name: String,
    devices: Vec<String>,
    policies: Policies,
    variants: Vec<Variant>,
}

impl Firmware {
    /// `name`, if this checkout has `src/firmware/petlibre/<name>` with the
    /// matching package and a `[package.metadata.petlibre]` declaration.
    pub fn new(repo: &Repo, name: &str) -> Result<Self> {
        let name = checked_name("Firmware", name)?;
        let path = repo.path(&format!("{FIRMWARES}/{name}/Cargo.toml"));
        let text = fs::read_to_string(&path)
            .map_err(|_| Error::Refused(format!("No firmware named {name} under {FIRMWARES}/")))?;

        Self::parse(name, &text)
    }

    fn parse(name: &str, text: &str) -> Result<Self> {
        let manifest: Manifest = toml::from_str(text)
            .map_err(|error| Error::Refused(format!("Firmware {name}'s Cargo.toml: {error}")))?;
        let package = format!("petlibre-fw-{name}");
        require(
            manifest.package.name == package,
            format!("Firmware {name} must be the package {package}"),
        )?;
        let Some(declaration) = manifest.package.metadata.petlibre else {
            return Err(Error::Refused(format!(
                "Firmware {name} has no [package.metadata.petlibre] table"
            )));
        };
        require(
            !declaration.devices.is_empty(),
            format!("Firmware {name} declares no devices"),
        )?;

        let mut variants = Vec::new();
        for (variant, declared) in declaration.variants {
            require(
                manifest.features.contains_key(&variant),
                format!("Firmware {name}'s variant {variant} has no Cargo feature of that name"),
            )?;
            variants.push(Variant {
                name: variant,
                summary: declared.summary,
                policies: declared.policies,
            });
        }
        Ok(Self {
            name: name.to_string(),
            devices: declaration.devices,
            policies: declaration.policies,
            variants,
        })
    }

    /// Every firmware in this checkout, by name.
    pub fn all(repo: &Repo) -> Result<Vec<Self>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(repo.path(FIRMWARES))? {
            let entry = entry?;
            if entry.path().join("Cargo.toml").is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort();

        names.iter().map(|name| Self::new(repo, name)).collect()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Refused unless the firmware declares `device`.
    pub fn require_device(&self, device: &str) -> Result<()> {
        require(
            self.devices.iter().any(|d| d == device),
            format!(
                "Firmware {} does not build for {device} (it declares {})",
                self.name,
                self.devices.join(", ")
            ),
        )
    }

    /// The declared variant `name`.
    pub fn variant(&self, name: &str) -> Result<Variant> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .cloned()
            .ok_or_else(|| {
                let known: Vec<&str> = self.variants.iter().map(|v| v.name.as_str()).collect();
                Error::Refused(if known.is_empty() {
                    format!("Firmware {} has no image variants", self.name)
                } else {
                    format!(
                        "Firmware {} has no variant {name}; it has {}",
                        self.name,
                        known.join(", ")
                    )
                })
            })
    }

    /// The firmware policies of a first-boot image: the variant's, which
    /// replace the normal image's, or the normal image's.
    pub(crate) fn policies<'a>(&'a self, variant: Option<&'a Variant>) -> &'a Policies {
        variant.map_or(&self.policies, |v| &v.policies)
    }

    /// Its root-relative directory.
    pub(crate) fn dir(&self) -> String {
        format!("{FIRMWARES}/{}", self.name)
    }

    pub(crate) fn package(&self) -> String {
        format!("petlibre-fw-{}", self.name)
    }

    /// The KM4 archive Cargo writes for the package.
    pub(crate) fn archive(&self) -> String {
        format!("lib{}.a", self.package().replace('-', "_"))
    }

    /// What `petlibre-build list` prints for it.
    pub fn entry(&self) -> FirmwareEntry {
        FirmwareEntry {
            name: self.name.clone(),
            devices: self.devices.clone(),
            variants: self
                .variants
                .iter()
                .map(|v| VariantEntry {
                    name: v.name.clone(),
                    summary: v.summary.clone(),
                    motion: v.motion(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[package]
name = "petlibre-fw-demo"

[features]
km4 = []
pulse = []
quiet = []

[package.metadata.petlibre]
devices = ["plaf103"]
policies = { optics = "fw13-optics-v2" }

[package.metadata.petlibre.variants]
pulse = { summary = "Pulses the motor", policies = { motion = "fw16-pulse-v1" } }
quiet = { summary = "No probes" }
"#;

    #[test]
    fn variants_replace_the_normal_policies() {
        let firmware = Firmware::parse("demo", MANIFEST).unwrap();
        assert_eq!(
            firmware.policies(None).optics.as_deref(),
            Some("fw13-optics-v2")
        );

        let pulse = firmware.variant("pulse").unwrap();
        assert!(pulse.motion());
        assert_eq!(firmware.policies(Some(&pulse)).optics, None);
        let quiet = firmware.variant("quiet").unwrap();
        assert!(!quiet.motion());
        assert_eq!(*firmware.policies(Some(&quiet)), Policies::default());
    }

    #[test]
    fn undeclared_names_are_refused() {
        let firmware = Firmware::parse("demo", MANIFEST).unwrap();
        let error = firmware.variant("speaker").unwrap_err().to_string();
        assert!(error.contains("pulse, quiet"), "{error}");
        firmware.require_device("plaf103").unwrap();
        assert!(firmware.require_device("plaf203").is_err());
    }

    #[test]
    fn declarations_are_checked() {
        let renamed = MANIFEST.replace("petlibre-fw-demo", "petlibre-fw-other");
        assert!(Firmware::parse("demo", &renamed).is_err());

        let featureless = MANIFEST.replace("quiet = []\n", "");
        let error = Firmware::parse("demo", &featureless)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no Cargo feature"), "{error}");

        let misspelt = MANIFEST.replace("optics =", "optic =");
        assert!(Firmware::parse("demo", &misspelt).is_err());

        let undeclared = "[package]\nname = \"petlibre-fw-demo\"\n";
        assert!(Firmware::parse("demo", undeclared).is_err());
    }

    #[test]
    fn the_checked_in_firmwares_declare_themselves() {
        let repo = Repo::here().unwrap();
        let firmwares = Firmware::all(&repo).unwrap();
        assert!(firmwares.iter().any(|f| f.name() == "reference"));
        assert!(firmwares.iter().any(|f| f.name() == "blink"));
    }
}
