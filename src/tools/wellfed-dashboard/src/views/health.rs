//! `/api/health`: whether the bench is ready, from its config, the flash
//! client and the image the feeder last booted. The relays and the running
//! job are the server's live state.

use std::fs;
use std::path::Path;

use serde::Serialize;
use wellfed_core::record::{Event, Record, Tag};

use super::{Sources, captures_by_age, modified, printable};
use wellfed_formats::bench::relative;
use wellfed_formats::bench::{CONFIG, Config};
use wellfed_formats::canonical::sha256;

/// Boot captures are small; skip anything larger when looking for a banner.
const BANNER_SCAN_BYTES: u64 = 4 << 20;
const BANNER_SCAN_CAPTURES: usize = 25;

/// `/api/health`: ports, the flash client and the image the feeder last
/// booted.
#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct HealthView {
    /// Root-relative `bench.json`.
    config: &'static str,
    config_ok: bool,
    ports: Ports,
    flash_tool: FlashToolView,
    installed: Option<Installed>,
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
    // Shown as the bench reports it: the path, then why.
    let config = wellfed_formats::load::<Config>(&root.join(CONFIG))
        .map_err(|error| format!("{CONFIG}: {error}"));
    let ports = match &config {
        Ok(config) => Ports::Found {
            relay_port: Path::new(&config.relay_port).exists(),
            uart_port: Path::new(&config.uart_port).exists(),
            baseline: root.join(&config.baseline).is_file(),
            rom_report: root.join(&config.rom_report).is_file(),
        },
        Err(error) => Ports::Unreadable {
            error: error.clone(),
        },
    };

    HealthView {
        config: CONFIG,
        config_ok: config.is_ok(),
        ports,
        flash_tool: flash.get(sources),
        installed: installed(sources),
    }
}

/// The newest capture that shows a replacement-firmware start banner.
/// The scan reads up to 25 captures of up to 4 MiB each, so a write that
/// starts during it stops it at the next capture: the scan never runs
/// beside a write.
fn installed(sources: &Sources) -> Option<Installed> {
    let root = &sources.root;
    let start = format!("{} start build=", Tag::Start);
    let start = start.as_bytes();

    for (at, dir) in captures_by_age(root).into_iter().take(BANNER_SCAN_CAPTURES) {
        if sources.uart_busy() {
            return None;
        }
        let rx = dir.join("serial-rx.bin");
        if fs::metadata(&rx).map_or(true, |m| m.len() > BANNER_SCAN_BYTES) {
            continue;
        }
        let Ok(data) = fs::read(&rx) else { continue };
        let Some(offset) = data.windows(start.len()).rposition(|w| w == start) else {
            continue;
        };

        let line = printable(&data[offset..]);
        let line = line.lines().next().unwrap_or("");
        let (build, bank) = match Record::parse(line).and_then(|record| record.event()) {
            Some(Event::Start { build, bank }) => (Some(build.to_string()), bank),
            _ => (None, None),
        };

        return Some(Installed {
            capture: relative(root, &dir),
            at,
            build,
            bank: bank.map(|bank| bank.to_string()),
            // Bank 0 runs only as the recovery role: after a failed trial,
            // or to install an offer bank 1 gave way for.
            role: if bank == Some(0) {
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

        // Hashed, never run: the page does not start the flash client.
        let view = match fs::read(&sources.flash_bin) {
            Ok(data) => FlashToolView {
                path: relative(&sources.root, &sources.flash_bin),
                sha256: Some(sha256(&data)),
                present: true,
            },
            Err(_) => missing(),
        };
        self.key = Some(key);
        self.value = Some(view.clone());
        view
    }
}

/// This module's page types, for [`super::bindings`].
#[cfg(test)]
pub(super) fn bindings(cfg: &ts_rs::Config) -> Vec<String> {
    use crate::bindings::decl;
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
    use serde_json::json;
    use wellfed_formats::bench::CAPTURES;
    use wellfed_formats::bench::live::Uart;
    use wellfed_formats::canonical::sha256;

    use super::*;
    use crate::views::tests::{json, live_sources, sources};

    #[test]
    fn the_banner_scan_stops_while_the_flash_client_holds_the_uart() {
        let (_tmp, sources, live) = live_sources();
        let dir = sources.root.join(CAPTURES).join("x-boot");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("serial-rx.bin"),
            b"PLFW1 start build=abc123 bank=1\r\n",
        )
        .unwrap();

        live.state.send_modify(|state| state.uart = Uart::Loader);

        assert!(installed(&sources).is_none());
        live.state.send_modify(|state| state.uart = Uart::Idle);
        assert!(installed(&sources).is_some());
    }

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

        fs::write(
            dir.join("serial-rx.bin"),
            b"PLFW1 start build=abc123 bank=0 reset=00000001 mmu=0,0,0\r\n",
        )
        .unwrap();
        let view = json(health(&sources, &mut FlashCache::default()));
        assert_eq!(view["installed"]["role"], "recovery");
        assert_eq!(view["config"], CONFIG);
        assert_eq!(view["config_ok"], false);
        assert!(view["ports"]["error"].as_str().unwrap().contains(CONFIG));
        assert_eq!(view["flash_tool"]["present"], false);
        assert!(view["flash_tool"].get("sha256").is_none());
    }

    #[test]
    fn health_reports_present_ports_and_the_flash_hash() {
        let (_tmp, sources) = sources();
        fs::create_dir_all(sources.root.join("private")).unwrap();
        fs::write(sources.root.join("baseline.bin"), b"").unwrap();
        fs::write(
            sources.root.join(CONFIG),
            r#"{"relay_port": "/nonexistent/relay", "uart_port": "/", "baseline": "baseline.bin", "rom_report": "rom.json"}"#,
        )
        .unwrap();
        fs::write(&sources.flash_bin, b"tool").unwrap();

        let mut cache = FlashCache::default();
        let view = json(health(&sources, &mut cache));
        assert_eq!(view["config_ok"], true);
        assert_eq!(
            view["ports"],
            json!({"relay_port": false, "uart_port": true, "baseline": true, "rom_report": false})
        );
        assert_eq!(
            view["flash_tool"],
            json!({"path": "wellfed-flash", "sha256": sha256(b"tool"), "present": true})
        );
        assert_eq!(
            json(health(&sources, &mut cache))["flash_tool"],
            view["flash_tool"]
        );
    }
}
