# Python tools

Scripts are grouped by what they do. Run them from the repository root with the project environment, for example `.venv/bin/python src/tools/flash/review-development-image.py --help`. Install `requirements-probe.txt` for checks and emulation, and also `requirements-hardware.txt` for serial access.

What stays in Python, on purpose: stock recovery (the Rust port has not run it on hardware or under emulation), the unicorn emulations of vendor code (factory loader, ROM, flash helper), and the image review (it calls `wellfed-build` for the build's inputs and linked-code audits). Everything else is Rust, including the firmware build (`src/tools/wellfed-build`).

**Device** marks the only scripts that open a serial port. Everything else works offline. Physical operations need an explicit task and a reviewed procedure (see [AGENTS.md](../../AGENTS.md)); a script being here does not authorize running it against the feeder.

## Repository checks

| Script | Purpose |
| --- | --- |
| [check-python.py](check-python.py) | Run the Python tests (`make check`), or `--fixtures` for the private binary-evidence checks |

## Bench: relay control and progress

The relay bench is the Rust binary `wellfed-bench` ([src/tools/wellfed-bench](wellfed-bench), [guide](../../docs/hardware/bench.md)). It switches power and the loader strap, chains `wellfed-flash` for captures, dumps and updates, and serves the read-only progress page with `--ui`. `wellfed-bench demo` clicks the spare relays.

## Flash: reads, updates and recovery

The two `uart-*` clients own the serial port. The [Rust client](../../docs/development/rust-flash.md) `wellfed-flash` ports the reader, development update, give-way and stock recovery with the same evidence formats. It passed on the research unit (FW32), and it is the only development-update and give-way writer; the relay bench drives it. These Python clients, the reader and stock recovery, remain accepted, and the Rust client's tests still check journals against their inspector. The rest are the components they load, plus offline planners and reviewers. See the [development updater](../../docs/development/uart-development.md) and [recovery limits](../../docs/hardware/recovery.md).

| Script | Device | Purpose |
| --- | --- | --- |
| [flash/uart-flash-read.py](flash/uart-flash-read.py) | yes | Restricted reader: observe captures, bounded and full reads; never programs or erases |
| [flash/uart-stock-recovery.py](flash/uart-stock-recovery.py) | yes | Exact original bank-1 stock recovery |
| [flash/review-development-image.py](flash/review-development-image.py) | no | Freeze a reviewed stock OTA or PLAF103 `firstboot` build as an update source/target |
| [flash/stock-bank-wire-policy.py](flash/stock-bank-wire-policy.py) | no | Expected TX transcript for the stock bank-1 acceptance test |
| [flash/plan-stock-recovery.py](flash/plan-stock-recovery.py) | no | Plan sector-scoped stock recovery |
| [flash/recovery-transaction.py](flash/recovery-transaction.py) | no | Recovery plan coordinator with a durable journal |
| [flash/recovery-sector-transport.py](flash/recovery-sector-transport.py) | no | Bounded stock-sector transport component |
| [flash/inspect-recovery-transaction.py](flash/inspect-recovery-transaction.py) | no | Read existing recovery receipts; never resumes a write |
| [flash/marker-activation-test.py](flash/marker-activation-test.py) | no | Original-stock write-back test transactions |
| [flash/flash-status.py](flash/flash-status.py) | no | GD25Q64E status decoding and reads over an injected transport |

## Audit: captures, images and flash

The capture auditors (startup, FW14 ADC, FW30 driver transients) and the full-flash comparison are the Rust binary `wellfed-audit` ([src/tools/wellfed-audit](wellfed-audit)). They check a finalized receive-only capture against the build manifest and image it claims to run, and write evidence only under `private/`. The scripts left here emulate vendor code under unicorn or serve the firmware build.

| Script | Purpose |
| --- | --- |
| [audit/audit-uart-startups.py](audit/audit-uart-startups.py) | Evidence integrity and reset decoding for saved UART captures |
| [audit/audit-ota-layout.py](audit/audit-ota-layout.py) | OTA container and placement profile |
| [audit/audit-factory-mapping.py](audit/audit-factory-mapping.py) | Execute the factory flash mapper offline |
| [audit/audit-recovery-boot.py](audit/audit-recovery-boot.py) | Factory-loader and ROM recovery route |
| [audit/audit-sdk-flash-users.py](audit/audit-sdk-flash-users.py) | Flash/IPC branches in both linked SDK images |

## Emulate: offline execution and simulation

| Script | Purpose |
| --- | --- |
| [emulate/emulate-recovery-session.py](emulate/emulate-recovery-session.py) | Helper erase/program/read transport on synthetic NOR |
| [emulate/emulate-ota-markers.py](emulate/emulate-ota-markers.py) | The FW10/FW11 marker sequence through the actual factory loader's bank selection |

## Layout notes

- The firmware build and its link audits are Rust: `wellfed-build` ([firmware builds](../../docs/development/firmware-builds.md)).
- Tests are in [tests/](tests/), one `test_*.py` per tool or feature. `check-python.py` discovers them.
- Scripts load their siblings by path, because hyphenated names are not importable. A folder-qualified name such as `flash/uart-flash-read.py` is relative to `src/tools/`, and `cargo xtask check-repository` verifies every such reference (documentation links, portability and transport isolation too).
- Reports record their tool hashes under these folder-qualified names. Evidence from before October 21, 2026 names the old flat paths (`src/tools/<script>.py`).
- Removed October 22, 2026, and still named by older evidence (their last versions are in Git history):
  - Ported to Rust, with the same checks and outcomes: `bench/bench.py`, `bench/bench-dashboard.py` and `bench/relay-demo.py` (`wellfed-bench`); `flash/uart-give-way.py`, `flash/uart-development.py`, `flash/development-bank.py` and `flash/development-wire-policy.py` (`wellfed-flash`); `audit/audit-firmware-startup.py`, `audit/audit-firmware-adc.py`, `audit/audit-driver-transients.py` and `audit/audit-flash-backup.py` (`wellfed-audit`); `emulate/virtual-feeder.py` (`wellfed-sim`'s `virtual-feeder`); `check-repository.py` (`cargo xtask check-repository`).
  - Finished, no longer run: the FW05–FW15 capture auditors (`audit-firmware-supervision`, `-park`, `-wifi`, `-wifi-join`, `-transport`, `-optics`, `-journal`, `-settings`); the stock C-probe research (`prepare-c-probe.py`, `emulate-c-probe.py`, `emulate-uart-read.py` and the `wellfed-probe` crate); the standalone diagnostic (`uart-standalone.py`, `standalone-bank.py`, `standalone-wire-policy.py`, `prepare-firstboot.py`); and `audit-probe-memory.py`, `audit-recovery-program.py`, `emulate-development.py`, `emulate-recovery-transaction.py`, `emulate-standalone.py`, `prepare-stock-ota-test.py` and `trace-stock-pad-power.py`.
