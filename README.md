# Wellfed

Wellfed frees the **PETLIBRO Granary PLAF103** pet feeder from the cloud. It gives the feeder replacement firmware, a command-line client and an optional self-hosted server, all running on your own network.

Wellfed is not affiliated with PETLIBRO, and it is not ready for everyday use yet. It is independent community research, tested on one feeder on our bench.

## Status

The replacement firmware feeds, joins Wi-Fi and talks to its clients on a real feeder. Here is what works today:

- **Feeding.** A supervised motor owner stops each feed on time and survives a power cut without feeding twice. The dispense button and the CLI both feed.
- **Connecting.** The feeder joins Wi-Fi, pairs over Bluetooth and serves an encrypted local API to the clients you pair.
- **Installing.** Stock firmware 1.1.15 installs Wellfed through its own updater, with no soldering. You can go back to stock over the air the same way.
- **Updating.** Over-the-air updates fall back to the previous image if the new one fails to start.

Schedules, the server's web UI and the Android app are not built yet. The [roadmap](docs/roadmap.md) has the full table and what comes next.

## Is Wellfed for you?

Wellfed may suit you if:

- you own a PLAF103 and want it to work without the vendor's cloud;
- you are comfortable with a command line and a self-built Rust toolchain;
- you want to help test, document or build a local-first feeder.

Don't use it yet if:

- a pet depends on the feeder for its meals. There are no schedules yet, and the firmware has not finished its long unattended run;
- you want a phone app. The Android app is not started;
- you have a different PETLIBRO model. Only the PLAF103 is supported, and a finding on one model never carries over to another;
- you can't afford to lose the feeder. Recovery from a bad flash needs UART access, and it is proven on one unit only.

## Documentation

The [documentation index](docs/README.md) lists every page. Good places to start:

| You want to… | Read |
| --- | --- |
| See what works and what's next | [Roadmap](docs/roadmap.md) |
| Install Wellfed over stock's updater | [Consumer installation](docs/firmware/consumer-installation.md) |
| Look up a word | [Glossary](docs/glossary.md) |
| Understand the product | [Product architecture](docs/firmware/product-architecture.md), [device API](docs/firmware/device-api.md), [the server](docs/server/README.md) |
| Understand the firmware | [Replacement firmware](docs/firmware/README.md), [OTA design](docs/firmware/ota-design.md), [bank policy](docs/firmware/bank-policy.md) |
| Learn the hardware | [PLAF103 device page](docs/devices/plaf103/README.md), [UART wiring](docs/devices/plaf103/uart.md), [flash layout](docs/devices/plaf103/flash-layout.md), [recovery](docs/devices/plaf103/recovery.md) |
| Contribute | [Contributing](CONTRIBUTING.md), [development setup](docs/development/README.md), [SDK builds](docs/development/firmware-builds.md), [writing docs](docs/development/writing.md) |

## Build and test

You don't need a feeder, vendor binaries or private research files to build, test and simulate the code.

Install Rust through rustup, GNU Make, CMake and a C compiler (for the Unicorn emulator the host tools link). The pinned Rust toolchain includes rustfmt, Clippy, and the ARM targets. On Linux:

```sh
cargo build --locked
make check
```

The first run downloads dependencies. Checks use host models and loopback networking. They never connect to a feeder, open serial ports or change firewall rules. SDK downloads and hardware operations are separate, explicit workflows.

## Repository layout

| Folder | Contents |
| --- | --- |
| `src/sdk/` | The SDK every firmware builds on: runtime, feeding core, hardware ports, board map and simulator ([firmware architecture](docs/firmware/README.md)) |
| `src/product/` | Product policy shared by the firmware and the server: the device protocol and the feeder's stores |
| `src/client/` | The clients the CLI, the server and the Android app share ([server design](docs/server/design.md#crates)) |
| `src/server/` | The owner's optional server ([the server](docs/server/README.md)) |
| `src/tools/` | Host tools, including the `wellfed` CLI ([tool list](src/tools/README.md)) |
| `src/firmware/wellfed/` | One crate per firmware: `feeder`, `qa`, `bringup` and `blink` ([firmware guide](src/firmware/wellfed/README.md)) |
| `src/firmware/vendor/` | The vendor SDK side: C hooks and ports, pinned SDK and toolchain locks, the stock layout, simulator scenarios and the image build (`make -C src/firmware/vendor`) |
| `src/firmware/devices/` | Per-device descriptions and layouts |
| `src/firmware/bench/` | The bench board's RP2040 firmware and its relay protocol crate ([bench firmware](src/firmware/bench/README.md)) |
| `src/hardware/bench/` | The Wellfed Bench board: KiCad project, fab outputs and enclosure ([bench board](src/hardware/bench/README.md)) |
| `src/web/` | Web frontends (a pnpm workspace): the bench page, the [well-fed.dev](docs/web/site.md) site, shared tokens and components, and API types generated from Rust. `make ui` builds them |
| `src/fuzz/` | Fuzz targets ([fuzzing](docs/development/README.md#fuzzing)) |
| `assets/` | Brand marks, design tokens, the feeder's 3D model and its renders ([assets](assets/README.md)) |
| `docs/` | Hardware findings, firmware design and contributor guides |
| `private/` | Ignored, contributor-owned secrets, evidence, dependencies and artifacts |

The existing `wellfed-*` crate names are kept for compatibility.

## License and provenance

First-party material is [MIT licensed](LICENSE), except the bench board's hardware design in `src/hardware/bench/`, which is [CERN-OHL-P](src/hardware/bench/LICENSE). Dependencies and vendor components keep their own licenses; see [third-party provenance](THIRD_PARTY.md).

This repository contains no vendor firmware or device backups. [Contributions](CONTRIBUTING.md) are welcome.
