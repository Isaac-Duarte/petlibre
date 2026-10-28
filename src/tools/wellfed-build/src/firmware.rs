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

use serde::Deserialize;
use wellfed_formats::require;

use crate::catalog::{FirmwareEntry, VariantEntry};
use crate::{Repo, Result, checked_name};

/// The firmware directory every firmware crate lives in.
const FIRMWARES: &str = "src/firmware/wellfed";

/// Why a firmware, its declaration or a requested variant was refused.
#[derive(Debug, thiserror::Error)]
pub enum FirmwareError {
    #[error("No firmware named {name} under {FIRMWARES}/")]
    Unknown {
        name: String,
        #[source]
        source: std::io::Error,
    },
    // The message names the parse error, so it is not also a source: a
    // chain printer would show it twice.
    #[error("Firmware {name}'s Cargo.toml: {error}")]
    Manifest {
        name: String,
        error: toml::de::Error,
    },
    #[error("Firmware {name} must be the package {package}")]
    Package { name: String, package: String },
    #[error("Firmware {name} has no [package.metadata.wellfed] table")]
    Undeclared { name: String },
    #[error("Firmware {name} declares no devices")]
    NoDevices { name: String },
    /// Only a variant's policies are limited to first-boot builds, so only
    /// a variant may name a motion policy.
    #[error("Firmware {name}'s normal image names a motion policy; make it a variant")]
    NormalMotion { name: String },
    #[error("Firmware {name}'s variant {variant} has no Cargo feature of that name")]
    NoFeature { name: String, variant: String },
    #[error("Firmware {name} does not build for {device} (it declares {})", declared.join(", "))]
    Device {
        name: String,
        device: String,
        declared: Vec<String>,
    },
    #[error("Firmware {name} has no image variants")]
    NoVariants { name: String },
    #[error("Firmware {name} has no variant {variant}; it has {}", known.join(", "))]
    UnknownVariant {
        name: String,
        variant: String,
        known: Vec<String>,
    },
}

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
        let text = fs::read_to_string(&path).map_err(|source| FirmwareError::Unknown {
            name: name.into(),
            source,
        })?;

        Self::parse(name, &text)
    }

    fn parse(name: &str, text: &str) -> Result<Self> {
        let manifest: Manifest = toml::from_str(text).map_err(|error| FirmwareError::Manifest {
            name: name.into(),
            error,
        })?;
        let package = format!("wellfed-fw-{name}");
        require(manifest.package.name == package, || {
            FirmwareError::Package {
                name: name.into(),
                package,
            }
        })?;
        let Some(declaration) = manifest.package.metadata.wellfed else {
            return Err(FirmwareError::Undeclared { name: name.into() }.into());
        };
        require(!declaration.devices.is_empty(), || {
            FirmwareError::NoDevices { name: name.into() }
        })?;
        // Policies reach the manifest of first-boot builds only, and only a
        // variant is limited to those, so a motion image must be a variant:
        // otherwise a non-first-boot build could drive the motor with no
        // motion policy for the bench's attended-motion gate to see.
        require(declaration.policies.motion.is_none(), || {
            FirmwareError::NormalMotion { name: name.into() }
        })?;

        let mut variants = Vec::new();
        for (variant, declared) in declaration.variants {
            require(manifest.features.contains_key(&variant), || {
                FirmwareError::NoFeature {
                    name: name.into(),
                    variant: variant.clone(),
                }
            })?;
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
    pub fn check_device(&self, device: &str) -> Result<()> {
        require(self.devices.iter().any(|d| d == device), || {
            FirmwareError::Device {
                name: self.name.clone(),
                device: device.into(),
                declared: self.devices.clone(),
            }
        })?;
        Ok(())
    }

    /// The declared variant `name`.
    pub fn variant(&self, name: &str) -> Result<Variant> {
        self.variants
            .iter()
            .find(|v| v.name == name)
            .cloned()
            .ok_or_else(|| {
                let known: Vec<String> = self.variants.iter().map(|v| v.name.clone()).collect();
                let firmware = self.name.clone();
                if known.is_empty() {
                    FirmwareError::NoVariants { name: firmware }.into()
                } else {
                    FirmwareError::UnknownVariant {
                        name: firmware,
                        variant: name.into(),
                        known,
                    }
                    .into()
                }
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
    use crate::Error;

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
        let error = firmware.variant("speaker").unwrap_err();
        assert!(
            matches!(
                &error,
                Error::Firmware(FirmwareError::UnknownVariant { variant, known, .. })
                    if variant == "speaker" && known == &["pulse", "quiet"]
            ),
            "{error}"
        );
        firmware.check_device("plaf103").unwrap();
        assert!(matches!(
            firmware.check_device("plaf203"),
            Err(Error::Firmware(FirmwareError::Device { device, .. })) if device == "plaf203"
        ));
    }

    #[test]
    fn declarations_are_checked() {
        let firmware_error = |text: &str| match Firmware::parse("demo", text) {
            Err(Error::Firmware(error)) => error,
            other => panic!("{other:?}"),
        };

        let renamed = MANIFEST.replace("wellfed-fw-demo", "wellfed-fw-other");
        assert!(matches!(
            firmware_error(&renamed),
            FirmwareError::Package { package, .. } if package == "wellfed-fw-demo"
        ));

        let featureless = MANIFEST.replace("quiet = []\n", "");
        assert!(matches!(
            firmware_error(&featureless),
            FirmwareError::NoFeature { variant, .. } if variant == "quiet"
        ));

        let misspelt = MANIFEST.replace("optics =", "optic =");
        assert!(matches!(
            firmware_error(&misspelt),
            FirmwareError::Manifest { .. }
        ));

        let moving = MANIFEST.replace(
            "policies = { optics = \"fw13-optics-v2\" }",
            "policies = { motion = \"fw16-pulse-v1\" }",
        );
        assert!(matches!(
            firmware_error(&moving),
            FirmwareError::NormalMotion { .. }
        ));

        let undeclared = "[package]\nname = \"wellfed-fw-demo\"\n";
        assert!(matches!(
            firmware_error(undeclared),
            FirmwareError::Undeclared { .. }
        ));
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
        for name in ["qa", "bringup", "blink", "feeder"] {
            let firmware = firmwares.iter().find(|f| f.name() == name);
            assert!(firmware.is_some(), "{name}");
            assert_eq!(
                firmware.is_some_and(Firmware::bluetooth),
                name == "feeder",
                "{name}"
            );
        }
    }

    /// The bring-up areas are variants of one firmware, each with the
    /// policies the area's image has always carried.
    #[test]
    fn the_bringup_variants_keep_their_policies() {
        let repo = Repo::here().unwrap();
        let bringup = Firmware::new(&repo, "bringup").unwrap();
        assert_eq!(*bringup.policies(None), Policies::default());

        let policy = |variant: &str| {
            let variant = bringup.variant(variant).unwrap();
            bringup.policies(Some(&variant)).clone()
        };
        let some = |value: &str| Some(value.to_string());
        for (variant, expected) in [
            (
                "optics",
                Policies {
                    optics: some("fw13-optics-v2"),
                    ..Policies::default()
                },
            ),
            ("buttons", Policies::default()),
            (
                "adc",
                Policies {
                    adc: some("fw14-adc-v2"),
                    ..Policies::default()
                },
            ),
            ("speaker", Policies::default()),
            (
                "pulse",
                Policies {
                    motion: some("fw16-pulse-v1"),
                    ..Policies::default()
                },
            ),
            (
                "stall",
                Policies {
                    trial: some("fw11-reset-v1"),
                    ..Policies::default()
                },
            ),
            (
                "fail",
                Policies {
                    trial: some("fw11-fail-v1"),
                    ..Policies::default()
                },
            ),
        ] {
            assert_eq!(policy(variant), expected, "{variant}");
        }

        let motion: Vec<&str> = bringup
            .variants
            .iter()
            .filter(|v| v.motion())
            .map(Variant::name)
            .collect();
        assert_eq!(motion, ["pulse"]);
    }

    /// Every declared variant is one the crate's start selection knows: the
    /// one-variant check counts it, and only `fail` (whose failure is the
    /// runtime's) keeps the normal image's start hook.
    #[test]
    fn the_bringup_start_selection_knows_every_variant() {
        let repo = Repo::here().unwrap();
        let bringup = Firmware::new(&repo, "bringup").unwrap();
        let app = fs::read_to_string(repo.path("src/firmware/wellfed/bringup/src/app.rs")).unwrap();
        let block = |start: &str, end: &str| {
            let from = app.find(start).unwrap();
            let to = from + app[from..].find(end).unwrap();
            app[from..to].to_string()
        };
        let counted = block("let variants = [", "];");
        let normal = block("#[cfg(not(any(", ")))]");

        assert!(!bringup.variants.is_empty());
        for variant in &bringup.variants {
            let feature = format!("feature = \"{}\"", variant.name());
            assert!(
                counted.contains(&feature),
                "{} is not counted",
                variant.name()
            );
            assert_eq!(
                normal.contains(&feature),
                variant.name() != "fail",
                "{} and the normal start hook",
                variant.name()
            );
        }
    }
}
