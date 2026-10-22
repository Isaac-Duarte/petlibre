# Python tools

Scripts are grouped by what they do. Run them from the repository root with the project environment, for example `.venv/bin/python tools/bench/bench.py status`. Install `requirements-probe.txt` for checks and emulation, and also `requirements-hardware.txt` for serial access.

**Device** marks the only scripts that open a serial port. Everything else works offline. Physical operations need an explicit task and a reviewed procedure (see [AGENTS.md](../AGENTS.md)); a script being here does not authorize running it against the feeder.

## Repository checks

| Script | Purpose |
| --- | --- |
| [check-python.py](check-python.py) | Run the Python tests (`make check`), or `--fixtures` for the private binary-evidence checks |
| [check-repository.py](check-repository.py) | Documentation links, tool dependencies, portable inputs, transport isolation |

## Bench: relay control and progress

| Script | Device | Purpose |
| --- | --- | --- |
| [bench/bench.py](bench/bench.py) | yes | Relay power and loader strap; chains captures, dumps and updates through the flash tools ([guide](../docs/hardware/bench.md)) |
| [bench/bench-dashboard.py](bench/bench-dashboard.py) | no | Read-only localhost page: progress, ETAs, latest RX log, timed operator steps |

## Flash: reads, updates and recovery

The five `uart-*` clients own the serial port. The rest are the components they load, plus offline planners and reviewers. See the [development updater](../docs/development/uart-development.md) and [recovery limits](../docs/hardware/recovery.md).

| Script | Device | Purpose |
| --- | --- | --- |
| [flash/uart-flash-read.py](flash/uart-flash-read.py) | yes | Restricted reader: observe captures, bounded and full reads; never programs or erases |
| [flash/uart-development.py](flash/uart-development.py) | yes | Inspect, prepare and execute reviewed application updates of bank 1, or bank 0 with `--bank 0` (`--interrupted` finishes a stopped one, `--restore-marker` undoes a give-way) |
| [flash/uart-standalone.py](flash/uart-standalone.py) | yes | Install the pinned UART diagnostic, or restore the original |
| [flash/uart-stock-recovery.py](flash/uart-stock-recovery.py) | yes | Exact original bank-1 stock recovery |
| [flash/uart-give-way.py](flash/uart-give-way.py) | yes | Clear bank 1's first marker byte so the loader falls back to an exactly reviewed bank 0 |
| [flash/review-development-image.py](flash/review-development-image.py) | no | Freeze a reviewed stock OTA or `plaf103-firstboot` build as an update source/target |
| [flash/development-bank.py](flash/development-bank.py) | no | Region-scoped bank-1 update transactions (injected transport) |
| [flash/development-wire-policy.py](flash/development-wire-policy.py) | no | Independent expected TX for development updates |
| [flash/standalone-bank.py](flash/standalone-bank.py) | no | Pinned-diagnostic install and restore transactions |
| [flash/standalone-wire-policy.py](flash/standalone-wire-policy.py) | no | Independent expected TX for the pinned diagnostic |
| [flash/stock-bank-wire-policy.py](flash/stock-bank-wire-policy.py) | no | Expected TX transcript for the stock bank-1 acceptance test |
| [flash/plan-stock-recovery.py](flash/plan-stock-recovery.py) | no | Plan sector-scoped stock recovery |
| [flash/recovery-transaction.py](flash/recovery-transaction.py) | no | Recovery plan coordinator with a durable journal |
| [flash/recovery-sector-transport.py](flash/recovery-sector-transport.py) | no | Bounded stock-sector transport component |
| [flash/inspect-recovery-transaction.py](flash/inspect-recovery-transaction.py) | no | Read existing recovery receipts; never resumes a write |
| [flash/marker-activation-test.py](flash/marker-activation-test.py) | no | Original-stock write-back test transactions |
| [flash/flash-status.py](flash/flash-status.py) | no | GD25Q64E status decoding and reads over an injected transport |
| [flash/prepare-firstboot.py](flash/prepare-firstboot.py) | no | Freeze and review the UART-only standalone diagnostic |
| [flash/prepare-stock-ota-test.py](flash/prepare-stock-ota-test.py) | no | Prepare the fixed stock-version experiment |

## Audit: captures, images and flash

Capture auditors check a finalized receive-only UART capture against the build manifest and image it claims to run; evidence output must stay under `private/`.

| Script | Purpose |
| --- | --- |
| [audit/audit-firmware-startup.py](audit/audit-firmware-startup.py) | Startup, stopped outputs and LED probe; shared by every capture auditor below |
| [audit/audit-firmware-supervision.py](audit/audit-firmware-supervision.py) | FW05 supervision timing, stall and watchdog |
| [audit/audit-firmware-park.py](audit/audit-firmware-park.py) | FW06 park round trip |
| [audit/audit-firmware-wifi.py](audit/audit-firmware-wifi.py) | FW07 radio on and scans |
| [audit/audit-firmware-wifi-join.py](audit/audit-firmware-wifi-join.py) | FW07 join, DHCP, DNS and reconnect |
| [audit/audit-firmware-transport.py](audit/audit-firmware-transport.py) | FW08 health transport, stages b and c |
| [audit/audit-firmware-optics.py](audit/audit-firmware-optics.py) | FW13 emitter windows, inputs and owner stall, stages b and c |
| [audit/audit-firmware-adc.py](audit/audit-firmware-adc.py) | FW14 power-input samples |
| [audit/audit-firmware-journal.py](audit/audit-firmware-journal.py) | FW15 journal on NOR |
| [audit/audit-firmware-settings.py](audit/audit-firmware-settings.py) | FW15 settings power-cut test |
| [audit/audit-driver-transients.py](audit/audit-driver-transients.py) | FW30 logic-analyzer captures: any motor-driver drive across power-ons or loader entries |
| [audit/audit-uart-startups.py](audit/audit-uart-startups.py) | Evidence integrity and reset decoding for saved UART captures |
| [audit/audit-flash-backup.py](audit/audit-flash-backup.py) | Compare two full-flash reads |
| [audit/audit-ota-layout.py](audit/audit-ota-layout.py) | OTA container and placement profile |
| [audit/audit-factory-mapping.py](audit/audit-factory-mapping.py) | Execute the factory flash mapper offline |
| [audit/audit-recovery-boot.py](audit/audit-recovery-boot.py) | Factory-loader and ROM recovery route |
| [audit/audit-recovery-program.py](audit/audit-recovery-program.py) | Vendor-helper sector-program semantics |
| [audit/audit-probe-memory.py](audit/audit-probe-memory.py) | Stock startup MPU configuration |
| [audit/audit-sdk-flash-users.py](audit/audit-sdk-flash-users.py) | Flash/IPC branches in both linked SDK images |

## Emulate: offline execution and simulation

| Script | Purpose |
| --- | --- |
| [emulate/virtual-feeder.py](emulate/virtual-feeder.py) | Replacement Rust feeder scenarios on synthetic hardware ([scenarios](emulate/scenarios/)) |
| [emulate/emulate-uart-read.py](emulate/emulate-uart-read.py) | Pinned vendor UART helper on synthetic flash |
| [emulate/emulate-development.py](emulate/emulate-development.py) | Development-update round trip through the actual helper |
| [emulate/emulate-standalone.py](emulate/emulate-standalone.py) | Pinned-diagnostic install and restore receipts |
| [emulate/emulate-recovery-session.py](emulate/emulate-recovery-session.py) | Helper erase/program/read transport on synthetic NOR |
| [emulate/emulate-recovery-transaction.py](emulate/emulate-recovery-transaction.py) | A complete planned restore in memory |
| [emulate/prepare-c-probe.py](emulate/prepare-c-probe.py) | Build the non-actuating stock execution probe |
| [emulate/emulate-c-probe.py](emulate/emulate-c-probe.py) | Run stock GET_CONFIG and the compiled probe on real ARM code |
| [emulate/emulate-ota-markers.py](emulate/emulate-ota-markers.py) | The FW10/FW11 marker sequence through the actual factory loader's bank selection |
| [emulate/trace-stock-pad-power.py](emulate/trace-stock-pad-power.py) | Execute the stock KM0 shared-pad supply initializer |

## Layout notes

- Firmware build helpers live separately in [firmware/tools/](../firmware/tools/) (`build.py` and its link audits).
- Tests are in [tests/](tests/), one `test_*.py` per tool or feature. `check-python.py` discovers them.
- Scripts load their siblings by path, because hyphenated names are not importable. A folder-qualified name such as `flash/uart-flash-read.py` is relative to `tools/`, and `check-repository.py` verifies every such reference.
- Reports record their tool hashes under these folder-qualified names. Evidence from before October 21, 2026 names the old flat paths (`tools/<script>.py`).
