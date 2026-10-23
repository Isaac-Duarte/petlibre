# Petlibre

Local control and experimental replacement firmware for the **PETLIBRO Granary PLAF103** pet feeder. Independent community research; no vendor affiliation.

Petlibre contains a Rust local server for stock firmware, a Rust replacement-firmware workspace, and documented hardware/firmware findings. You can build, test, and simulate the code without a feeder, vendor binaries, or private research files.

## Current capabilities

| Component | Established | Remaining work |
| --- | --- | --- |
| Stock-firmware local server | Local MQTT connection and supervised dispensing demonstrated on stock 1.1.15; authenticated API and durable request tracking | Measured portions, sustained offline operation, cold boots, and reconnect reliability |
| Replacement firmware | Feeding, sensor, watchdog, journal, and flash-coordination components tested on host models; Rust diagnostic booted on one PLAF103; build-bound reports, stopped GPIO startup, and bounded LED control added | Functional physical feeding, networking, schedules, panel/audio behavior, real flash persistence, and hardware acceptance |
| UART and recovery | UART access, matching full 8 MiB backups, independent loader entry, bounded write-back, stock restoration/boot, diagnostic installation, and faster development updates demonstrated on the research unit | General device compatibility and power-cut/full-bank recovery acceptance |

The replacement is experimental. A diagnostic PASS does not establish working physical feeding. See [firmware status and architecture](docs/firmware/README.md) and [recovery limits](docs/hardware/recovery.md).

## Start here

- [Resume the current firmware work](RESUME.md): installed image, last bench state, evidence, and next task
- [Hardware identification and signal map](docs/hardware/README.md)
- [UART wiring and loader entry](docs/hardware/uart.md)
- [Stock firmware and replacement architecture](docs/firmware/README.md)
- [Development setup and checks](docs/development/README.md)
- [Local server setup and API](docs/development/local-server.md)
- [SDK firmware builds](docs/development/firmware-builds.md)
- [Firmware feature board](FIRMWARE_BOARD.md) and [startup/indicator diagnostics](docs/firmware/startup-diagnostics.md)
- [UART development updates](docs/development/uart-development.md)

## Build and test

Install Rust through rustup, Python 3.10.12 or newer with venv/pip, and GNU Make. The pinned Rust toolchain includes rustfmt, Clippy, and the ARM targets. On Linux:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r tools/requirements-probe.txt
cargo build --locked
make check PYTHON=.venv/bin/python
```

Initial setup downloads dependencies. Checks use host models and loopback networking; they do not connect to a feeder, open serial ports, or modify firewall rules. SDK downloads and hardware operations are separate, explicit workflows.

## Repository layout

| Location | Contents |
| --- | --- |
| `crates/` | Every Rust crate, one folder per package: `petlibre-local` (stock-firmware local server), `petlibre-flash` (UART flash reads, updates and recovery), `petlibre-bench` (relay bench and its progress page), `petlibre-audit` (offline evidence audits), `petlibre-build` (SDK firmware builds and their link audits), `petlibre-formats` (the file formats those host tools share: build manifests and identities, layout, locks, captures, credentials), `xtask` (`cargo xtask check-repository`), and the replacement-firmware crates, including `petlibre-sim` (the virtual feeder) |
| `firmware/` | SDK hooks and C ports, pinned builds, layout, and simulator build |
| `tools/` | Image review, emulation, and experimental recovery tools/tests |
| `docs/` | Hardware findings, firmware contracts, and contributor workflows |
| `private/` | Ignored contributor-owned secrets, evidence, dependencies, and artifacts |

The existing `petlibre-*` crate names and `petlibre-local` executable are retained for compatibility. [Contributions](CONTRIBUTING.md) are welcome. First-party material is [MIT licensed](LICENSE); see [third-party provenance](THIRD_PARTY.md). This repository contains no vendor firmware or device backups.
