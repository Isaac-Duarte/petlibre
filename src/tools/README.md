# Host tools

Every host tool is a Rust crate in the workspace. Build them with `cargo build --locked --release` and run the binaries from `target/release/` at the repository root (or pass `--root` where a tool takes one). `make check` runs their tests; the evidence tests that need ignored private inputs run under `make check-fixtures` ([fixtures](../../docs/development/fixtures.md)).

**Device** marks the tools that open a serial port or switch relays. Everything else works offline. Physical operations need an explicit task and a reviewed procedure (see [AGENTS.md](../../AGENTS.md)); a tool being here does not authorize running it against the feeder.

| Crate | Device | Purpose |
| --- | --- | --- |
| [wellfed-flash](wellfed-flash) | yes | The UART flash client: restricted reads, region-scoped development updates, give-ways and original-stock recovery, each inspect → prepare → execute ([guide](../../docs/development/rust-flash.md)) |
| [wellfed-bench](wellfed-bench) | yes | The relay bench: power and loader strap, chained `wellfed-flash` sessions, the over-the-air loop, and the read-only progress page with `--features ui` ([guide](../../docs/devices/plaf103/bench.md)) |
| [wellfed-local](wellfed-local) | no (network) | The local server for stock and replacement firmware: MQTT broker, signed OTA offers and status |
| [wellfed-build](wellfed-build) | no | SDK firmware builds, their linked-image audits, and `review`, which freezes a reviewed build or the stock OTA for `wellfed-flash` ([firmware builds](../../docs/development/firmware-builds.md), [development updates](../../docs/development/uart-development.md)) |
| [wellfed-audit](wellfed-audit) | no | Offline audits of finalized receive-only captures (startup, FW14 ADC, FW30 driver transients) and full-flash backup comparison |
| [wellfed-emu](wellfed-emu) | no | The vendor factory loader (mapping and bank selection) and UART flash helper, executed under Unicorn on supplied state: the review's mapping check, the OTA marker fixtures, and `wellfed-flash`'s emulated sessions |
| [wellfed-formats](wellfed-formats) | no | The files these tools pass to each other (build manifest, frozen review, stock layout, locks, captures, credentials, flash plans), typed once |
| [xtask](xtask) | no | `cargo xtask check-repository`: documentation links, portability, no Python under `src/`, transport isolation |

## Removed Python tools

The tools here were Python until October 2026. Older evidence still names them, as `src/tools/<folder>/<script>.py` (or `src/tools/<script>.py` before October 21, 2026); their last versions are in Git history.

- Removed October 23, 2026:
  - Ported to Rust: `flash/review-development-image.py` (`wellfed-build review`; both froze the same build to identical reviews apart from the reviewer hash); `audit/audit-factory-mapping.py`'s loader execution and `emulate/emulate-ota-markers.py` (`wellfed-emu` and its fixture tests); `emulate/emulate-recovery-session.py`'s helper model (`wellfed-emu`'s helper, which `wellfed-flash`'s emulated tests serve); `check-python.py` and `tests/` (Rust tests).
  - Superseded by earlier ports: `flash/uart-flash-read.py`, `flash-status.py`, `plan-stock-recovery.py`, `recovery-transaction.py`, `recovery-sector-transport.py`, `inspect-recovery-transaction.py` and `uart-stock-recovery.py` (`wellfed-flash`); `audit/audit-ota-layout.py` (`wellfed-build`'s layout audit); `audit/audit-uart-startups.py`'s capture validation (`wellfed-audit`).
  - Finished, no longer run: the stock write-back acceptance tests (`flash/marker-activation-test.py`, `flash/stock-bank-wire-policy.py`, `uart-stock-recovery.py prepare-activation-test` and `bank-writeback-test`); the research reports of `audit/audit-recovery-boot.py` (ROM download-route selector), `audit-factory-mapping.py` (probe-slot mapping), `audit-uart-startups.py` (reset-reason decoder), `audit/audit-sdk-flash-users.py` (call-site inventory; its packaged-code check lives on in `wellfed-build`) and `emulate-recovery-session.py`.
- Removed October 22, 2026:
  - Ported to Rust, with the same checks and outcomes: `bench/bench.py`, `bench/bench-dashboard.py` and `bench/relay-demo.py` (`wellfed-bench`); `flash/uart-give-way.py`, `flash/uart-development.py`, `flash/development-bank.py` and `flash/development-wire-policy.py` (`wellfed-flash`); `audit/audit-firmware-startup.py`, `audit/audit-firmware-adc.py`, `audit/audit-driver-transients.py` and `audit/audit-flash-backup.py` (`wellfed-audit`); `emulate/virtual-feeder.py` (`wellfed-sim`'s `virtual-feeder`); `check-repository.py` (`cargo xtask check-repository`).
  - Finished, no longer run: the FW05–FW15 capture auditors (`audit-firmware-supervision`, `-park`, `-wifi`, `-wifi-join`, `-transport`, `-optics`, `-journal`, `-settings`); the stock C-probe research (`prepare-c-probe.py`, `emulate-c-probe.py`, `emulate-uart-read.py` and the `wellfed-probe` crate); the standalone diagnostic (`uart-standalone.py`, `standalone-bank.py`, `standalone-wire-policy.py`, `prepare-firstboot.py`); and `audit-probe-memory.py`, `audit-recovery-program.py`, `emulate-development.py`, `emulate-recovery-transaction.py`, `emulate-standalone.py`, `prepare-stock-ota-test.py` and `trace-stock-pad-power.py`.
