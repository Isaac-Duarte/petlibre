# Wellfed

Local control and experimental replacement firmware for the **PETLIBRO Granary PLAF103** pet feeder. Independent community research; no vendor affiliation.

Wellfed contains a Rust local server for stock firmware, a Rust replacement-firmware workspace, and documented hardware/firmware findings. You can build, test, and simulate the code without a feeder, vendor binaries, or private research files.

## Current capabilities

| Component | Established | Remaining work |
| --- | --- | --- |
| Stock install path | Stock 1.1.15 installs the replacement over its own updater and the round trip back runs over the air, with no UART write (FW40, FW41); local dispensing on stock was demonstrated, then retired with stock control (FW51) | The setup-frame provisioning (FW42) and rollback from the owner's own stock (FW43) |
| Replacement firmware | Feeding, sensor, watchdog, journal, and flash-coordination components tested on host models; Rust diagnostic booted on one PLAF103; build-bound reports, stopped GPIO startup, and bounded LED control added | Functional physical feeding, networking, schedules, panel/audio behavior, real flash persistence, and hardware acceptance |
| UART and recovery | UART access, matching full 8 MiB backups, independent loader entry, bounded write-back, stock restoration/boot, diagnostic installation, and faster development updates demonstrated on the research unit | General device compatibility and power-cut/full-bank recovery acceptance |

The replacement is experimental. A diagnostic PASS does not establish working physical feeding. See [firmware status and architecture](docs/firmware/README.md) and [recovery limits](docs/devices/plaf103/recovery.md).

## Start here

- [Resume the current firmware work](RESUME.md): installed image, last bench state, evidence, and next task
- [Supported devices](docs/devices/README.md) and the [PLAF103 device page](docs/devices/plaf103/README.md): identification, signal map, flash layout and stock firmware
- [UART wiring and loader entry](docs/devices/plaf103/uart.md)
- [Stock firmware and replacement architecture](docs/firmware/README.md)
- [Development setup and checks](docs/development/README.md)
- [Local server setup and API](docs/development/local-server.md)
- [SDK firmware builds](docs/development/firmware-builds.md)
- [Firmware board](FIRMWARE_BOARD.md), [feature parity board](FEATURE_BOARD.md) and [startup/indicator diagnostics](docs/firmware/startup-diagnostics.md)
- [UART development updates](docs/development/uart-development.md)

## Build and test

Install Rust through rustup, GNU Make, CMake and a C compiler (for the Unicorn emulator the host tools link). The pinned Rust toolchain includes rustfmt, Clippy, and the ARM targets. On Linux:

```sh
cargo build --locked
make check
```

Initial setup downloads dependencies. Checks use host models and loopback networking; they do not connect to a feeder, open serial ports, or modify firewall rules. SDK downloads and hardware operations are separate, explicit workflows.

## Repository layout

| Location | Contents |
| --- | --- |
| `src/sdk/` | The Wellfed SDK that firmwares build on: `wellfed-sdk` (the runtime every firmware runs inside, and the firmware API), `wellfed-core` (hardware-independent feeding, journal and flash logic), `wellfed-hal` and `wellfed-hal-common` (AmebaD ports), `wellfed-amebad-sys` (pinned vendor SDK bindings and the host fake), `wellfed-sdk-c` (the KM0 archive and the panic handler), `wellfed-proto` (the device protocol's messages), `wellfed-probes` (the bench probes the firmwares share), `wellfed-testkit` and `wellfed-sim` (the virtual feeder) |
| `src/tools/` | Host tools: `wellfed` (the owner's command line: BLE setup and the device API), `wellfed-feeder-sim` (a simulated feeder serving the device API), `wellfed-local` (the bench's local server: stock installs and the replacement's MQTT transport, being retired), `wellfed-flash` (UART flash reads, updates and recovery), `wellfed-bench` (relay bench and its progress page, built in with `--features ui`), `wellfed-audit` (offline evidence audits), `wellfed-build` (SDK firmware builds, their link audits and the frozen image review), `wellfed-emu` (the vendor factory loader and flash helper, executed under Unicorn), `wellfed-formats` (the file formats those host tools share), `xtask` (`cargo xtask check-repository`) |
| `src/product/` | Product policy shared by the firmware and the server: `wellfed-feeder` (`no_std`; the child lock, pairing and the BLE setup session today, schedules and alerts later) |
| `src/client/` | `wellfed-client`: the sans-IO client of the feeder's protocols that the owner's CLI and the Android app share (BLE setup and the device API); `wellfed-connect`: its tokio driver for the CLI and the server |
| `src/server/` | The owner's optional server ([API design](docs/api/README.md)): `wellfed-server` (the binary: the HTTP API and its event stream), `wellfed-service` (users and one connection task per feeder), `wellfed-store` (SQLite through sqlx) and `wellfed-api` (the HTTP types and OpenAPI schemas) |
| `src/firmware/wellfed/` | One crate per firmware built on the SDK ([guide](src/firmware/wellfed/README.md)): `feeder` (the product firmware: the panel's owner in its loop task), `qa` (the default: a guided self-test of every bring-up check), one firmware per bring-up area (`optics`, `buttons`, `adc`, `speaker`, `motor`, `trial`), and `blink` (the minimal template) |
| `src/firmware/vendor/` | The vendor SDK side: C entry hooks and ports, the pinned SDK and toolchain locks, the stock layout, simulator scenarios, and the image build (`make -C src/firmware/vendor`) |
| `src/web/` | Web frontends (pnpm workspace): the bench page, the [well-fed.dev](docs/web/site.md) landing page, shared design tokens and components, and API types generated from Rust. `make ui` builds them |
| `src/scripts/` | Developer scripts: `dev-dashboard` serves the bench page for this checkout |
| `docs/` | Hardware findings, firmware contracts, and contributor workflows |
| `private/` | Ignored contributor-owned secrets, evidence, dependencies, and artifacts |

The existing `wellfed-*` crate names and `wellfed-local` executable are retained for compatibility. [Contributions](CONTRIBUTING.md) are welcome. First-party material is [MIT licensed](LICENSE); see [third-party provenance](THIRD_PARTY.md). This repository contains no vendor firmware or device backups.
