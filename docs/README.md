# Documentation

Start here to find the right page. The docs are grouped by what you want to do: use Wellfed, understand how it works, learn the hardware, or help build it.

Every finding carries an [evidence level](evidence.md) that says how we know it. Outside sources are listed in [references](references.md). A documented finding is not a request to repeat an experiment. Historical bench diaries and raw evidence live outside this repository.

## Using Wellfed

| Page | What it covers |
| --- | --- |
| [Roadmap](roadmap.md) | What works today and what comes next |
| [Glossary](glossary.md) | The words the rest of the docs use without explaining |
| [Supported devices](devices/README.md) | Which feeders Wellfed runs on |
| [Consumer installation](firmware/consumer-installation.md) | Moving a stock feeder to Wellfed over the air, and going back |

## How it works

The firmware docs describe Wellfed's own replacement, which isn't tied to one device.

| Page | What it covers |
| --- | --- |
| [Product architecture](firmware/product-architecture.md) | The product we are building toward, and why each choice was made |
| [Firmware architecture](firmware/README.md) | How the replacement firmware is put together |
| [Device API](firmware/device-api.md) | The feeder's encrypted local API |
| [Pairing](firmware/pairing.md) | Keys, randomness and how clients pair |
| [Bluetooth](firmware/bluetooth.md) | Bluetooth LE and setup mode |
| [OTA design](firmware/ota-design.md) | Over-the-air updates |
| [Bank policy](firmware/bank-policy.md) | Which bank boots, and how a trial image is confirmed |
| [Startup diagnostics](firmware/startup-diagnostics.md) | What every boot checks before firmware code runs |
| [QA firmware](firmware/qa.md) | The guided self-test image |
| [Resource budget](firmware/resource-budget.md) | Flash, RAM and task limits of the product image |
| [Device layer](firmware/device-layer.md) | Where the board's assumptions live, so a second model can be added |
| [The server](server/README.md) | What the optional server adds, [running it](server/running.md), and [how it's built](server/design.md) |
| [Web UI design](web/README.md) | The server's browser UI |
| [well-fed.dev](web/site.md) | The project's landing page |
| [docs.well-fed.dev](web/docs-site.md) | This documentation as a website |

Feeder components:

| Page | What it covers |
| --- | --- |
| [Motor pulse](firmware/motor-pulse.md) | The supervised motor drive |
| [Portions and calibration](firmware/calibration.md) | Counting portions and calibrating them |
| [Jams and sensor faults](firmware/jams-and-sensor-faults.md) | What happens when food or feedback goes missing |
| [Optical inputs](firmware/optical-inputs.md) | The food, outlet and feedback sensors |
| [Buttons](firmware/buttons.md) | Panel buttons and gestures |
| [Indicators](firmware/indicators.md) | The panel lights |
| [Audio](firmware/audio.md) | The speaker and built-in tunes |
| [Clock](firmware/clock.md) | Wall clock, offline time and missed time |
| [Power](firmware/power.md) | The supply, the battery floor, brownout and sleep |

## Hardware

Facts about a **chip** hold for every board built on it. Facts about a **device** hold for one feeder model and its stock firmware. Adding a model adds a folder under `devices/` and reuses the rest.

| Page | What it covers |
| --- | --- |
| [PLAF103](devices/plaf103/README.md) | The feeder: identification, chips and feature support |
| [Board and signal map](devices/plaf103/board.md) | What each chip pin connects to |
| [Motor driver](devices/plaf103/motor-driver.md) | The motor driver and its electrical checks |
| [UART](devices/plaf103/uart.md) | Console wiring and loader entry |
| [Flash layout](devices/plaf103/flash-layout.md) | Boot flow and what lives where in flash |
| [Recovery](devices/plaf103/recovery.md) | The experimental UART recovery writers |
| [Relay bench](devices/plaf103/bench.md) | Powering and flashing the feeder from the bench |
| [Stock behavior and protocol](devices/plaf103/stock/README.md) | What stock firmware 1.1.15 does |
| [Stock OTA requirements](devices/plaf103/stock/ota-requirements.md) | What stock's updater accepts |
| [RTL8721DM (AmebaD)](chips/rtl8721dm/README.md) | The SoC and its SDK |
| [RTL8721DM pins](chips/rtl8721dm/pins.md) | Each pin's job on each device |
| [Device template](devices/_template/README.md) | The starting point for a new device folder |

## Contributing

| Page | What it covers |
| --- | --- |
| [Development setup](development/README.md) | Toolchain, check targets and fuzzing |
| [Writing docs](development/writing.md) | How these docs are written |
| [Rust standard](development/rust.md) | Rules for every Rust crate |
| [TypeScript standard](development/typescript.md) | Rules for the web frontends |
| [SDK builds](development/firmware-builds.md) | Full firmware images with the vendor SDK |
| [Stock server](development/stock-server.md) | The bench's server for stock 1.1.15 |
| [Rust flash client](development/rust-flash.md) | Reading and writing flash over UART |
| [UART development updates](development/uart-development.md) | Faster region-scoped updates during development |
| [Optional fixtures](development/fixtures.md) | Checks that need private binary inputs |
| [Import provenance](development/provenance.md) | Where the first code came from |
| [Host tools](../src/tools/README.md) | Every host tool and what it does |
| [Firmwares](../src/firmware/wellfed/README.md) | Every firmware crate and how to build one |
