//! `/api/health`: whether the bench is ready, from its config, the last relay
//! states, the flash client and the image the feeder last booted.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

use super::{Sources, captures_by_age, modified, printable, read};
use crate::config::{CONFIG, Config};
use crate::status::{self, FlashTool, RELAYS, RelayState};

/// Boot captures are small; skip anything larger when looking for a banner.
const BANNER_SCAN_BYTES: u64 = 4 << 20;
const BANNER_SCAN_CAPTURES: usize = 25;

/// `/api/health`: ports, last relay states, flash client and the image the
/// feeder last booted.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HealthView {
    /// Root-relative `bench.json`.
    config: &'static str,
    config_ok: bool,
    ports: Ports,
    relays: Option<BTreeMap<String, RelayState>>,
    flash_tool: FlashToolView,
    installed: Option<Installed>,
    /// The command a live bench process is running.
    busy: Option<String>,
}

/// Whether each configured path exists, or why the config does not read.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(untagged)]
enum Ports {
    Found {
        relay_port: bool,
        uart_port: bool,
        baseline: bool,
        rom_report: bool,
    },
    Unreadable {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct FlashToolView {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    present: bool,
}

/// The newest replacement-firmware start banner in a capture.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
struct Installed {
    capture: String,
    /// When that capture was last written.
    at: f64,
    build: Option<String>,
    bank: Option<String>,
    role: Role,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
enum Role {
    Normal,
    /// Bank 0's recovery role ran in that capture.
    Recovery,
}

pub(crate) fn health(sources: &Sources, flash: &mut FlashCache) -> HealthView {
    let root = &sources.root;
    let config = Config::load(root);
    let ports = match &config {
        Ok(config) => Ports::Found {
            relay_port: Path::new(&config.relay_port).exists(),
            uart_port: Path::new(&config.uart_port).exists(),
            baseline: root.join(&config.baseline).is_file(),
            rom_report: root.join(&config.rom_report).is_file(),
        },
        Err(error) => Ports::Unreadable {
            error: error.to_string(),
        },
    };

    HealthView {
        config: CONFIG,
        config_ok: config.is_ok(),
        ports,
        relays: read(&sources.status().join(RELAYS)),
        flash_tool: flash.get(sources),
        installed: installed(root),
        busy: status::busy(&sources.status()),
    }
}

/// The newest capture that shows a replacement-firmware start banner.
fn installed(root: &Path) -> Option<Installed> {
    const START: &[u8] = b"PLFW1 start build=";
    const RECOVERY: &[u8] = b"PLOTA1 start role=recovery";

    for (at, dir) in captures_by_age(root).into_iter().take(BANNER_SCAN_CAPTURES) {
        let rx = dir.join("serial-rx.bin");
        if fs::metadata(&rx).map_or(true, |m| m.len() > BANNER_SCAN_BYTES) {
            continue;
        }
        let Ok(data) = fs::read(&rx) else { continue };
        let Some(offset) = data.windows(START.len()).rposition(|w| w == START) else {
            continue;
        };

        let line = printable(&data[offset..]);
        let line = line.lines().next().unwrap_or("");
        let field = |key: &str| {
            line.split(' ')
                .find_map(|word| word.strip_prefix(key))
                .map(str::to_string)
        };
        let recovery = data.windows(RECOVERY.len()).any(|w| w == RECOVERY);

        return Some(Installed {
            capture: status::relative(root, &dir),
            at,
            build: field("build="),
            bank: field("bank="),
            role: if recovery {
                Role::Recovery
            } else {
                Role::Normal
            },
        });
    }
    None
}

/// The flash client's hash, recomputed only when the binary changes.
#[derive(Default)]
pub(crate) struct FlashCache {
    key: Option<(f64, u64)>,
    value: Option<FlashToolView>,
}

impl FlashCache {
    fn get(&mut self, sources: &Sources) -> FlashToolView {
        let missing = || FlashToolView {
            path: sources.flash_bin.display().to_string(),
            sha256: None,
            present: false,
        };
        let Ok(metadata) = fs::metadata(&sources.flash_bin) else {
            return missing();
        };

        let key = (modified(&sources.flash_bin).unwrap_or(0.0), metadata.len());
        let cached = self.value.as_ref().filter(|_| self.key == Some(key));
        if let Some(view) = cached {
            return view.clone();
        }

        let view = match crate::tools::flash_tool(&sources.root, &sources.flash_bin) {
            Some(FlashTool { path, sha256 }) => FlashToolView {
                path,
                sha256: Some(sha256),
                present: true,
            },
            None => missing(),
        };
        self.key = Some(key);
        self.value = Some(view.clone());
        view
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::ui::bindings::decl;
    vec![
        decl::<HealthView>(cfg),
        decl::<Ports>(cfg),
        decl::<FlashToolView>(cfg),
        decl::<Installed>(cfg),
        decl::<Role>(cfg),
    ]
}

#[cfg(test)]
mod tests {
    use petlibre_formats::canonical::sha256;
    use serde_json::json;

    use super::*;
    use crate::config::CAPTURES;
    use crate::status::Status;
    use crate::ui::views::tests::{json, sources};

    #[test]
    fn health_reads_the_last_banner() {
        let (_tmp, sources) = sources();
        let dir = sources.root.join(CAPTURES).join("x-boot");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("serial-rx.bin"),
            b"noise\r\nPLFW1 start build=abc123 bank=1 reset=00000001 mmu=0,0,0\r\n",
        )
        .unwrap();

        let view = json(health(&sources, &mut FlashCache::default()));
        assert_eq!(view["installed"]["build"], "abc123");
        assert_eq!(view["installed"]["bank"], "1");
        assert_eq!(view["installed"]["role"], "normal");
        assert_eq!(view["installed"]["capture"], "private/captures/x-boot");
        assert_eq!(view["config"], CONFIG);
        assert_eq!(view["config_ok"], false);
        assert!(view["ports"]["error"].as_str().unwrap().contains(CONFIG));
        assert!(view["relays"].is_null());
        assert!(view["busy"].is_null());
        assert_eq!(view["flash_tool"]["present"], false);
        assert!(view["flash_tool"].get("sha256").is_none());
    }

    #[test]
    fn health_reports_present_ports_relays_and_the_flash_hash() {
        let (_tmp, sources) = sources();
        fs::create_dir_all(sources.root.join("private")).unwrap();
        fs::write(sources.root.join("baseline.bin"), b"").unwrap();
        fs::write(
            sources.root.join(CONFIG),
            r#"{"relay_port": "/nonexistent/relay", "uart_port": "/", "baseline": "baseline.bin", "rom_report": "rom.json"}"#,
        )
        .unwrap();
        fs::write(&sources.flash_bin, b"tool").unwrap();
        let mut status = Status::new(&sources.root, sources.status());
        status.begin("on", None, &[]).unwrap();
        status.relay(1, true);
        status.finish(0, None);

        let mut cache = FlashCache::default();
        let view = json(health(&sources, &mut cache));
        assert_eq!(view["config_ok"], true);
        assert_eq!(
            view["ports"],
            json!({"relay_port": false, "uart_port": true, "baseline": true, "rom_report": false})
        );
        assert_eq!(view["relays"]["1"]["on"], true);
        assert_eq!(
            view["flash_tool"],
            json!({"path": "petlibre-flash", "sha256": sha256(b"tool"), "present": true})
        );
        assert_eq!(
            json(health(&sources, &mut cache))["flash_tool"],
            view["flash_tool"]
        );
    }
}
