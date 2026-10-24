# Host tools

Every host tool is a Rust crate in the workspace. Build them with `cargo build --locked --release` and run the binaries from `target/release/` at the repository root (or pass `--root` where a tool takes one). `make check` runs their tests; the evidence tests that need ignored private inputs run under `make check-fixtures` ([fixtures](../../docs/development/fixtures.md)).

Where each tool is going (`petlibre-local` split up and retired, the bench split into single-purpose crates) is in the [tools roadmap](../../docs/development/tools-roadmap.md).

**Device** marks the tools that open a serial port or switch relays. Everything else works offline. Physical operations need an explicit task and a reviewed procedure (see [AGENTS.md](../../AGENTS.md)); a tool being here does not authorize running it against the feeder.

| Crate | Device | Purpose |
| --- | --- | --- |
| [petlibre-flash](petlibre-flash) | yes | The UART flash client: restricted reads, region-scoped development updates, give-ways and original-stock recovery, each inspect → prepare → execute ([guide](../../docs/development/rust-flash.md)) |
| [petlibre-bench](petlibre-bench) | yes | The relay bench: power and loader strap, chained `petlibre-flash` sessions, the over-the-air loop, and the read-only progress page with `--features ui` ([guide](../../docs/devices/plaf103/bench.md)) |
| [petlibre](petlibre) | yes (Bluetooth) | The owner's command line: `petlibre setup` pairs this computer with a feeder in BLE setup mode and gives it Wi-Fi ([Bluetooth](../../docs/firmware/bluetooth.md#setup-exchange)); `status`, `settings`, `lock`, `wifi`, `time`, `events`, `invite`, `join`, `clients`, `revoke`, `reboot` and `recover` run on the [device API](../../docs/firmware/device-api.md), all through `petlibre-client` |
| [petlibre-local](petlibre-local) | no (network) | The bench's local server: the stock-to-replacement install, the replacement firmware's MQTT transport, signed OTA offers and status ([guide](../../docs/development/local-server.md)); being retired |
| [petlibre-build](petlibre-build) | no | SDK firmware builds, their linked-image audits, and `review`, which freezes a reviewed build or the stock OTA for `petlibre-flash` ([firmware builds](../../docs/development/firmware-builds.md), [development updates](../../docs/development/uart-development.md)) |
| [petlibre-audit](petlibre-audit) | no | Offline audits of finalized receive-only captures (startup, ADC samples, driver transients) and full-flash backup comparison |
| [petlibre-emu](petlibre-emu) | no | The vendor factory loader (mapping and bank selection) and UART flash helper, executed under Unicorn on supplied state: the review's mapping check, the OTA marker fixtures, and `petlibre-flash`'s emulated sessions |
| [petlibre-formats](petlibre-formats) | no | The files these tools pass to each other (build manifest, frozen review, stock layout, locks, captures, credentials and keys, flash plans), typed once, and the private-file writer |
| [xtask](xtask) | no | `cargo xtask check-repository`: documentation links, portability, no Python under `src/`, transport isolation |

## Removed Python tools

The tools here were Python until October 2026. Older evidence still names them, as `src/tools/<folder>/<script>.py` (or `src/tools/<script>.py` before October 21, 2026); their last versions are in Git history.

- Removed October 23, 2026:
  - Ported to Rust: `flash/review-development-image.py` (`petlibre-build review`; both froze the same build to identical reviews apart from the reviewer hash); `audit/audit-factory-mapping.py`'s loader execution and `emulate/emulate-ota-markers.py` (`petlibre-emu` and its fixture tests); `emulate/emulate-recovery-session.py`'s helper model (`petlibre-emu`'s helper, which `petlibre-flash`'s emulated tests serve); `check-python.py` and `tests/` (Rust tests).
  - Superseded by earlier ports: `flash/uart-flash-read.py`, `flash-status.py`, `plan-stock-recovery.py`, `recovery-transaction.py`, `recovery-sector-transport.py`, `inspect-recovery-transaction.py` and `uart-stock-recovery.py` (`petlibre-flash`); `audit/audit-ota-layout.py` (`petlibre-build`'s layout audit); `audit/audit-uart-startups.py`'s capture validation (`petlibre-audit`).
  - Finished, no longer run: the stock write-back acceptance tests (`flash/marker-activation-test.py`, `flash/stock-bank-wire-policy.py`, `uart-stock-recovery.py prepare-activation-test` and `bank-writeback-test`); the research reports of `audit/audit-recovery-boot.py` (ROM download-route selector), `audit-factory-mapping.py` (probe-slot mapping), `audit-uart-startups.py` (reset-reason decoder), `audit/audit-sdk-flash-users.py` (call-site inventory; its packaged-code check lives on in `petlibre-build`) and `emulate-recovery-session.py`.
- Removed October 22, 2026:
  - Ported to Rust, with the same checks and outcomes: `bench/bench.py`, `bench/bench-dashboard.py` and `bench/relay-demo.py` (`petlibre-bench`); `flash/uart-give-way.py`, `flash/uart-development.py`, `flash/development-bank.py` and `flash/development-wire-policy.py` (`petlibre-flash`); `audit/audit-firmware-startup.py`, `audit/audit-firmware-adc.py`, `audit/audit-driver-transients.py` and `audit/audit-flash-backup.py` (`petlibre-audit`); `emulate/virtual-feeder.py` (`petlibre-sim`'s `virtual-feeder`); `check-repository.py` (`cargo xtask check-repository`).
  - Finished, no longer run: the early bring-up capture auditors (`audit-firmware-supervision`, `-park`, `-wifi`, `-wifi-join`, `-transport`, `-optics`, `-journal`, `-settings`); the stock C-probe research (`prepare-c-probe.py`, `emulate-c-probe.py`, `emulate-uart-read.py` and the `petlibre-probe` crate); the standalone diagnostic (`uart-standalone.py`, `standalone-bank.py`, `standalone-wire-policy.py`, `prepare-firstboot.py`); and `audit-probe-memory.py`, `audit-recovery-program.py`, `emulate-development.py`, `emulate-recovery-transaction.py`, `emulate-standalone.py`, `prepare-stock-ota-test.py` and `trace-stock-pad-power.py`.
