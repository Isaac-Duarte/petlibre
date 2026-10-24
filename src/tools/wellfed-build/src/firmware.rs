//! A firmware crate under `src/firmware/wellfed/`, and what its
//! `[package.metadata.wellfed]` table declares: the devices it builds for,
//! whether it links the device's Bluetooth stack, the startup policies of its
//! normal first-boot image and its opt-in image variants.
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
const FIRMWARES: &str = "src/firmware/wellfed";

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
    wellfed: Option<Declaration>,
}

/// `[package.metadata.wellfed]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    devices: Vec<String>,
    #[serde(default)]
    bluetooth: bool,
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
/// Cargo package is `wellfed-fw-<name>`, a KM4 staticlib on `wellfed-sdk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmware {
    name: String,
    devices: Vec<String>,
    bluetooth: bool,
    policies: Policies,
    variants: Vec<Variant>,
}

impl Firmware {
    /// `name`, if this checkout has `src/firmware/wellfed/<name>` with the
    /// matching package and a `[package.metadata.wellfed]` declaration.
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
        let package = format!("wellfed-fw-{name}");
        require(
            manifest.package.name == package,
            format!("Firmware {name} must be the package {package}"),
        )?;
        let Some(declaration) = manifest.package.metadata.wellfed else {
            return Err(Error::Refused(format!(
                "Firmware {name} has no [package.metadata.wellfed] table"
            )));
        };
        require(
            !declaration.devices.is_empty(),
            format!("Firmware {name} declares no devices"),
        )?;
        // Policies reach the manifest of first-boot builds only, and only a
        // variant is limited to those, so a motion image must be a variant:
        // otherwise a non-first-boot build could drive the motor with no
        // motion policy for the bench's attended-motion gate to see.
        require(
            declaration.policies.motion.is_none(),
            format!("Firmware {name}'s normal image names a motion policy; make it a variant"),
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
            bluetooth: declaration.bluetooth,
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

    /// Links the device's Bluetooth stack. Every variant shares it, so
    /// one firmware's images never differ in whether the radio runs BT.
    pub fn bluetooth(&self) -> bool {
        self.bluetooth
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
        format!("wellfed-fw-{}", self.name)
    }

    /// The KM4 archive Cargo writes for the package.
    pub(crate) fn archive(&self) -> String {
        format!("lib{}.a", self.package().replace('-', "_"))
    }

    /// What `wellfed-build list` prints for it.
    pub fn entry(&self) -> FirmwareEntry {
        FirmwareEntry {
            name: self.name.clone(),
            devices: self.devices.clone(),
            bluetooth: self.bluetooth,
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
name = "wellfed-fw-demo"

[features]
km4 = []
pulse = []
quiet = []

[package.metadata.wellfed]
devices = ["plaf103"]
policies = { optics = "fw13-optics-v2" }

[package.metadata.wellfed.variants]
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
        let renamed = MANIFEST.replace("wellfed-fw-demo", "wellfed-fw-other");
        assert!(Firmware::parse("demo", &renamed).is_err());

        let featureless = MANIFEST.replace("quiet = []\n", "");
        let error = Firmware::parse("demo", &featureless)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no Cargo feature"), "{error}");

        let misspelt = MANIFEST.replace("optics =", "optic =");
        assert!(Firmware::parse("demo", &misspelt).is_err());

        let moving = MANIFEST.replace(
            "policies = { optics = \"fw13-optics-v2\" }",
            "policies = { motion = \"fw16-pulse-v1\" }",
        );
        let error = Firmware::parse("demo", &moving).unwrap_err().to_string();
        assert!(error.contains("make it a variant"), "{error}");

        let undeclared = "[package]\nname = \"wellfed-fw-demo\"\n";
        assert!(Firmware::parse("demo", undeclared).is_err());
    }

    #[test]
    fn bluetooth_is_opt_in() {
        assert!(!Firmware::parse("demo", MANIFEST).unwrap().bluetooth());
        let radio = MANIFEST.replace(
            "devices = [\"plaf103\"]\n",
            "devices = [\"plaf103\"]\nbluetooth = true\n",
        );
        assert!(Firmware::parse("demo", &radio).unwrap().bluetooth());
    }

    #[test]
    fn the_checked_in_firmwares_declare_themselves() {
        let repo = Repo::here().unwrap();
        let firmwares = Firmware::all(&repo).unwrap();
        for name in [
            "qa", "optics", "buttons", "adc", "speaker", "motor", "trial", "blink", "feeder",
        ] {
            let firmware = firmwares.iter().find(|f| f.name() == name);
            assert!(firmware.is_some(), "{name}");
            assert_eq!(
                firmware.is_some_and(Firmware::bluetooth),
                name == "feeder",
                "{name}"
            );
        }
    }
}
