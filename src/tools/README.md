# Host tools

Every host tool is a Rust crate in the workspace. This page lists them all, says what each does, and says which ones touch hardware. To pick a tool by task, see [Which tool do I use?](../../docs/development/README.md#which-tool-do-i-use).

Build them with `cargo build --locked --release`, and run the binaries from `target/release/` at the repository root. Some tools take `--root` to name another checkout. `make check` runs their tests. Evidence tests that need ignored private inputs run under `make check-fixtures` ([fixtures](../../docs/development/fixtures.md)).

## Inventory

**Touches hardware** marks the tools that open a serial port, switch relays or use Bluetooth. Everything else works offline or on the network.

> [!NOTE]
> A tool being listed here does not authorize running it against a feeder. Physical operations need an explicit task and a reviewed procedure ([AGENTS.md](../../AGENTS.md)).

| Tool | What it does | Touches hardware? |
| --- | --- | --- |
| [wellfed](wellfed) | The owner's command line, through `wellfed-client` and `wellfed-connect`. `setup` pairs this computer with a feeder in BLE setup mode and gives it Wi-Fi ([Bluetooth](../../docs/firmware/bluetooth.md#setup-exchange)). While a bench server runs for this user (found by its pointer in `$XDG_RUNTIME_DIR` or `~/.local/state/wellfed`), the device commands go through its session and every direct session is refused ([bench](../../docs/devices/plaf103/bench.md#the-feeder-session)). On the [device API](../../docs/firmware/device-api.md): `status`, `settings`, `plans`, `call`, `desiccant`, `lock`, `wifi`, `time`, `events`, `feeds`, `logs`, `invite`, `join`, `clients`, `revoke`, `feed`, `cancel-feed`, `reboot`, `stats`, `trace`, `calibrate` and `recover`. Installing and leaving: `install`, `backup` and `restore-stock`. `server` talks to the owner's server over its HTTP API | Yes (Bluetooth, for `setup`) |
| [wellfed-feeder-sim](wellfed-feeder-sim) | A simulated feeder serving the real device API over TCP. It runs the feeder's own loop pass (API server and driver, panel, pairing, setup mode) on the product's stores, with the panel's buttons and the supply (the adapter and the pack) on standard input. The server, the app and the CLI run against it without hardware | No (network) |
| [wellfed-flash](wellfed-flash) | The UART flash client: restricted reads, region-scoped development updates, give-ways and original-stock recovery, each as inspect, prepare, execute ([guide](../../docs/development/rust-flash.md)). No crate links it: every read and write runs it as a process | Yes (serial) |
| [wellfed-bench](wellfed-bench) | The relay bench's binary. `serve` runs the bench server in the foreground, drawn as a full-screen console on a terminal, and `note` records the operator's steps, timers and evidence on it. Every hardware command (power and the loader strap, chained `wellfed-flash` sessions, the over-the-air loop, the suite's `itest` and `stock-ota`) is its client. With `--features ui`, `serve --ui` runs the read-only progress page beside it ([guide](../../docs/devices/plaf103/bench.md)) | Yes (relays, serial) |
| [wellfed-bench-server](wellfed-bench-server) | The bench server: it holds the bench lease, the relay board, the owner's one device API session and a receive-only UART capture, runs one job at a time on the rig and the suite, keeps the bank ledger and the operator's notes (settling their timers as what they expect arrives), and answers on `private/bench.sock`. Also the clients' side of that socket | Yes (relays, serial, network) |
| [wellfed-rig](wellfed-rig) | The bench's operator sequences, and the power, capture, server and device API steps the suite builds on. The bench's command line runs them | Yes (relays, serial) |
| [wellfed-itest](wellfed-itest) | The real-device integration suite (`wellfed-bench itest`) and the stock-to-replacement install (`stock-ota`), run on the rig ([integration suite](../../docs/devices/plaf103/bench.md#integration-suite)) | Yes (relays, serial) |
| [wellfed-relay](wellfed-relay) | The SH-UR04A relay board driver, behind the `Board` trait another board can implement: feeder power, the loader strap and the UART lines | Yes (relays) |
| [wellfed-dashboard](wellfed-dashboard) | The bench's read-only progress page, which `wellfed-bench serve --ui` serves on the server's live state and its jobs' output. Links no relay or flash code, nor the server | No |
| [wellfed-stock](wellfed-stock) | What stock 1.1.15 speaks, as far as installing from it and rolling back need: its MQTT protocol and broker, the setup frame, the image server and the install-trial ledger. Its binary is the bench's stock server: `serve`, `ota-images`, the trial commands and the feeder-only `network` redirect ([guide](../../docs/development/stock-server.md)) | No (network) |
| [wellfed-stock-api](wellfed-stock-api) | The blocking loopback client of `wellfed-stock serve`'s HTTP API, shared by the suite and the page. Links nothing of the server | No (network) |
| [wellfed-build](wellfed-build) | SDK firmware builds and their linked-image audits. `review` freezes a reviewed build or the stock OTA for `wellfed-flash`. `keygen` and `sign` sign a frozen review as an update ([firmware builds](../../docs/development/firmware-builds.md), [development updates](../../docs/development/uart-development.md)) | No |
| [wellfed-audit](wellfed-audit) | Offline audits of finalized receive-only captures (startup, ADC samples, driver transients), and full-flash backup comparison | No |
| [wellfed-emu](wellfed-emu) | The vendor factory loader (mapping and bank selection) and UART flash helper, executed under Unicorn on supplied state. Backs the review's mapping check, the OTA marker fixtures and `wellfed-flash`'s emulated sessions | No |
| [wellfed-signing](wellfed-signing) | Update signing, only ever of a frozen review: the bench key, version 2 manifests for the device API, and the downgrade and key statements. `wellfed-build keygen`/`sign`, the bench's offers and `wellfed restore-stock` use it | No |
| [wellfed-image](wellfed-image) | The OTA image as every tool checks it: the container and core images, the two banks' layouts, the `private/` evidence root and frozen image reviews. The builder, the signer and the audits share these with `wellfed-flash` without linking it | No |
| [wellfed-formats](wellfed-formats) | The files these tools pass to each other, typed once: build manifest, frozen review, stock layout, locks, captures, credentials and keys, flash plans, and the bench's config, progress record, live state, bank ledger, notes, run records and suite catalog. Also the private-file writer | No |
| [xtask](xtask) | `cargo xtask check-repository`: documentation links, portability, no Python under `src/`, transport isolation and the server's crate layering. `cargo xtask fuzz <target>`: one bounded cargo-fuzz run ([fuzzing](../../docs/development/README.md#fuzzing)) | No |

Outside `src/tools/`, the simulator `wellfed-sim` (in `src/sdk/`) has the `virtual-feeder` binary ([the simulator](../../docs/development/README.md#the-simulator)), and the owner's server is `wellfed-server` (in `src/server/`).

## The `wellfed` command line

`wellfed` is the owner's client until the app exists. Every command below speaks the [device API](../../docs/firmware/device-api.md) through `wellfed-client`, as the app will, except `setup` (Bluetooth), `install` and `server`. Each takes `--keys` (default `private/wellfed-owner.json`), `--feeder` when the key file knows more than one feeder, and `--address` to skip mDNS. `wellfed <command> --help` has the rest.

| Command | What it does |
| --- | --- |
| `setup` | Pair with a feeder in BLE setup mode, give it Wi-Fi and enroll this computer as its owner |
| `status` | The clock, whether a feed can start and why not, the food level, the last failed or uncertain feed, the supply and charge, the desiccant reminder, the panel lock and the link |
| `settings [--lights on\|off] [--sound on\|off] [--volume N] [--lights-window W] [--sound-window W] [--desiccant-days N] [--meal-reminder off\|N]` | Read the settings, or change them against the generation just read |
| `plans list` / `add --at HH:MM --portions N [--days D] [--call 0-5] [--disabled]` / `edit SLOT` / `enable SLOT` / `disable SLOT` / `delete SLOT` | The feeding plans, up to ten, each with how often the meal call plays before it ([schedules](../../docs/firmware/schedules.md)) |
| `call show` / `upload FILE.wav` / `delete` | The meal call: one clip of your own, converted here and uploaded, played before each meal whose plan asks for it ([meal call](../../docs/firmware/schedules.md#meal-call)) |
| `feed [--portions N]` | Feed 1–48 portions and wait for the result; never retried |
| `cancel-feed FEED` | Stop a running feed |
| `feeds [--after N]` | The feeding history, one line a feed; an uncertain feed reads as a range |
| `events [--after N] [--follow [--samples]]` | Every recorded event, and with `--follow` new ones and live samples |
| `desiccant [--reset]` | Days until the desiccant is due; `--reset` records a replacement today |
| `lock on\|off` | The panel's child lock |
| `time [--zone Z\|--clear-zone] [--ntp S] [--show]` | Set the clock from this computer, or change and read the time settings |
| `wifi --ssid S \| --clear` | Store or forget the network the feeder joins next |
| `invite`, `join`, `clients`, `revoke`, `recover` | Who may use the feeder, and the owner's way back in through the recovery window |
| `calibrate`, `stats`, `trace FEED`, `logs` | Portion weight, statistics, one feed's drive trace and the debug log |
| `reboot` | Restart the feeder |
| `install`, `backup`, `restore-stock` | Move from stock 1.1.15 to Wellfed (importing stock's plans), keep stock's image, and go back |
| `server …` | The same reads and writes through the owner's server, with its history |

`status`, `settings`, `plans list`, `events`, `feeds` and `desiccant` print text for a person, and `--json` prints the feeder's values as the wire spells them, for scripts. The other commands print the feeder's answer as JSON.

## How the tools fit together

Each crate does one job, with one-way dependencies:

```text
wellfed-client   lib, sans-IO   device API sessions and pairing; the installer's steps
wellfed-model    lib            the device API's values, owned, as the CLI, the app and the server share them
wellfed-stock    lib + binary   the stock 1.1.15 protocol, only what install and rollback need:
                                 the legacy MQTT 3.1 broker, OTA_UPGRADE and its trial ledger,
                                 the setup frame; the binary is the bench's stock server
wellfed          CLI            the owner's command line
wellfed-server   binary         the owner's server, its web UI, and installs
wellfed-build    CLI            builds, reviews, and update signing (keygen, sign)
wellfed-signing  lib            update signing behind a frozen review, for the builder's
                                 commands, the bench, the CLI and, later, the server's installs
wellfed-image    lib            the OTA image, the banks and frozen reviews, as every tool
                                 checks them
wellfed-flash    CLI            every bench write goes through it as a process
```

### The bench

| Crate | Holds | Depends on |
| --- | --- | --- |
| `wellfed-relay` | The relay board driver: power, the loader strap and the UART lines | Nothing first-party |
| `wellfed-rig` | The operator sequences: power, loader, boot, capture, dump, the UART update and recovery chains, transients, the OTA loop, and the device API and server steps the suite builds on | `wellfed-relay`, `wellfed-formats`, `wellfed-client`, `wellfed-signing`, `wellfed-build`, and `wellfed-flash` as a process for every build, read and write |
| `wellfed-itest` | The integration suite: scenarios, the live and scripted rigs, `stock-ota`, and their arguments | `wellfed-rig`, `wellfed-relay`, `wellfed-formats` |
| `wellfed-bench-server` | The bench server: the lease, the relay board, the socket, the feeder session and the UART capture for its life, the rig's sequences and the suite as its jobs, one at a time, and the bank ledger; the client of its socket | `wellfed-rig`, `wellfed-itest`, `wellfed-relay`, `wellfed-formats`, `wellfed-connect` |
| `wellfed-bench` | The binary: `serve`, and the command line as the server's client | `wellfed-bench-server`, `wellfed-rig`, `wellfed-dashboard` (with `ui`) |
| `wellfed-dashboard` | The read-only progress page and its embedded web app, fed the server's live state through its `Live` trait | `wellfed-formats`, `wellfed-stock-api` |

- **The relay board has a successor in design.** The [Wellfed Bench board](../hardware/bench/README.md) answers the same `AT+CHn` lines on one USB cable, with the UART bridge beside them. It has not been built, and nothing here knows it yet.
- **Shared files live in `wellfed-formats`**, since one crate writes each and another reads it ([conventions](../../AGENTS.md#rust-conventions)). Under `bench`: a command's progress record and its writer, the server's live state and job stream, the bank ledger, the operator's notes, the run record (`NAME-itest.json`), the bench config, the session evidence layout, and the suite's scenario catalog. Under `stock`: the stock server's config, status document and image lease.
- **One server holds the bench.** `wellfed-bench serve` takes the lease and the relay board for its life, so a command is a job it runs, never a second process racing for the port. The relay board sits behind `wellfed-relay`'s `Board` trait, so another board can take its place.
- **The page is read-only by structure.** `wellfed-dashboard` links neither the relay driver, the flash client nor the server, and reports the flash client by hashing its file. `make check-deps` enforces the first two ([Rust standard](../../docs/development/rust.md#dependencies)). `cargo xtask check-repository` enforces the rest, by refusing any manifest under `src/` that names `wellfed-flash` or `wellfed-stock`.
- **`wellfed-flash` stays a child process** for every read and write. It is the audited writer, with its own journals, reports and no-retry stops, and a fault in the bench must never take a write down with it.
- **The suite links the rig, not the bench.** So the server links the suite and runs `itest` and `stock-ota` in its own process. `serve --ui` links the dashboard.
- **The suite runs `wellfed-stock` as a child process** (`serve`, `serve --enable-ota-trial` and `ota-images`), and speaks its HTTP routes through `wellfed-stock-api`. The bench signs by linking `wellfed-signing` and offers over the device API, with no server.

### Rules for new crates

- **Single purpose.** A crate does one job. A second consumer is a reason to split, not to grow.
- **Libraries leave IO to their caller where they can.** `wellfed-client` is sans-IO. `wellfed-stock` takes its sockets from the caller where it can, so the server, the CLI and the bench drive it the same way.
- **Every pinned check survives a move.** The trial ledger's no-resend rule, the image hash binding and the evidence gates move unchanged, with their tests.

## Older evidence names Python tools

The tools here were Python until October 2026, and older evidence still names them, as `src/tools/<folder>/<script>.py` (or `src/tools/<script>.py` before October 21, 2026). Their last versions are in Git history. Plans, reports and journals they wrote still read in `wellfed-flash`.

| Python tool | Now |
| --- | --- |
| `flash/uart-flash-read.py`, `flash-status.py`, `plan-stock-recovery.py`, `recovery-transaction.py`, `recovery-sector-transport.py`, `inspect-recovery-transaction.py`, `uart-stock-recovery.py`, `flash/uart-give-way.py`, `flash/uart-development.py`, `flash/development-bank.py`, `flash/development-wire-policy.py` | `wellfed-flash` |
| `flash/review-development-image.py`, `audit/audit-ota-layout.py` | `wellfed-build review` and its layout audit |
| `audit/audit-factory-mapping.py` (loader execution), `emulate/emulate-ota-markers.py`, `emulate/emulate-recovery-session.py` (helper model) | `wellfed-emu` |
| `audit/audit-firmware-startup.py`, `audit/audit-firmware-adc.py`, `audit/audit-driver-transients.py`, `audit/audit-flash-backup.py`, `audit/audit-uart-startups.py` (capture validation) | `wellfed-audit` |
| `bench/bench.py`, `bench/bench-dashboard.py` | `wellfed-bench` |
| `emulate/virtual-feeder.py` | `wellfed-sim`'s `virtual-feeder` |
| `check-repository.py`, `check-python.py` | `cargo xtask check-repository` and Rust tests |

The other Python tools finished their job and were removed without a port: the stock write-back acceptance tests, the research audits, the early bring-up capture auditors, the stock C-probe research and the standalone diagnostic.
