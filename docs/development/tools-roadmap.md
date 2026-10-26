# Host tools roadmap

Where each host tool is going as the product platform lands, decided October 24, 2026. The [tools index](../../src/tools/README.md) describes what each tool does today. The direction: single-purpose crates that the owner's tools and the bench link, and no catch-all binary.

## Today

| Tool | Lines | Fate |
| --- | ---: | --- |
| `wellfed-flash` | 8.2k | Stays: the audited UART writer, always run as its own process |
| `wellfed-build` | 7.3k | Stays, and gains `keygen` and `sign` for update signing |
| `wellfed-audit`, `wellfed-emu`, `wellfed-formats`, `xtask` | 10.8k | Stay |
| `wellfed-bench` | 15k | Split into single-purpose crates (`wellfed-relay`, `wellfed-bench`, `wellfed-itest`, `wellfed-dashboard`, Oct 25); stays dev-only |
| `wellfed-local` | 4.1k | Splits up and is deleted: a binary over `wellfed-stock` since October 25 |
| `wellfed-stock` | 4.0k | Stays: the stock 1.1.15 protocol, its broker, the setup frame and the install-trial ledger, moved out of `wellfed-local` unchanged |

## Why `wellfed-local` goes

`wellfed-local` is six tools sharing one config and one MQTT broker, and none of them is a product:

| Job | Commands | Destination |
| --- | --- | --- |
| Stock control (research era) | `feed`, `resolve`, `new-request-id`, `/api/feed`, the feed records in the ledger | **Deleted** (stage 1). The product uses stock only to install and roll back ([product architecture](../firmware/product-architecture.md)) |
| C-probe and modified-stock research | `probe-query`, the `1.1.16` and C-probe trial images | **Deleted** (stage 1). The research closed on October 22 |
| Direct-to-feeder bring-up | `speak`, `ble-wifi` | **Deleted** (stage 1). Their firmwares are bring-up only, and FW45 replaces BLE setup |
| Stock installer (FW40–FW43) | `ota-images`, `ota-trial`, `ota-trial-resolve`, `setup-send`, the stock broker; `init` and `network` (captured credentials, sudo redirect) | **`wellfed-stock`** (the protocol; **moved** Oct 25, the commands still in `wellfed-local`) and **`wellfed-client`** (the installer's steps). `init` and `network` go once the FW42 setup frame works |
| Replacement MQTT transport (FW08) | `firmware-credential`, `firmware-request-key`, the firmware broker role, `/api/replacement/request` | **Deleted**: requests with FW47, and `firmware-credential`, the broker role and the replacement's health in the status with the transport (Oct 25). A config's leftover `firmware` section is ignored |
| Replacement OTA (FW10) | `ota-offer`, the fault-server example | Signing **moved** to `wellfed-build keygen` and `sign` (version 2 manifests, `downgrade`, `revoke`, `add-key`), behind a frozen review (Oct 25). Offers **moved** to the device API: `wellfed_client::update`, which the bench drives with its own faults. `ota-offer`, the fault server and the version 1 manifest are **deleted** (Oct 25) |

The bench drives `wellfed-local` as a child process today (`serve` and `ota-images`, and the HTTP routes for status and the trial: request, resolve and answered); it already signs by linking `wellfed-build`. Linking libraries replaces the rest.

## Where it ends up

```text
wellfed-client   lib, sans-IO   device API sessions and pairing; the installer's steps
wellfed-model    lib            the device API's values, owned, as the CLI, the app and the server share them
wellfed-stock    lib            the stock 1.1.15 protocol, only what install and rollback need:
                                 the legacy MQTT 3.1 broker, OTA_UPGRADE and its trial ledger,
                                 the setup frame, reading stock's plan records for FP04
wellfed          CLI            the owner's command line: pair, feed, plans, settings, history,
                                 offer an update, install from stock, roll back (FP02)
wellfed-server   binary         the owner's server, its web UI, and installs
wellfed-build    CLI            builds, reviews, and update signing (keygen, sign)
wellfed-flash    CLI            unchanged; every bench write still goes through it as a process
```

## The bench, split

The bench's modules fell into separate jobs with one-way dependencies, and each is now its own crate (October 25, 2026):

| Crate | Holds | Depends on |
| --- | --- | --- |
| `wellfed-relay` | The SH-UR04A relay board driver: power, the loader strap and the UART lines | Nothing first-party |
| `wellfed-bench` | The binary and the operator sequences: power, loader, boot, capture, dump, the UART update and recovery chains, transients, the OTA loop; the command line, including the suite's arguments | `wellfed-relay`, `wellfed-formats`, `wellfed-client`, `wellfed-build` (library), `wellfed-flash` (as a process for every read and write; as a library for the review verification signing does), `wellfed-dashboard` (with `ui`) |
| `wellfed-itest` | The integration suite: scenarios, the live and scripted rigs, `stock-ota`; its binary runs the suite's commands | `wellfed-bench`'s rig, `wellfed-relay`, `wellfed-formats` |
| `wellfed-dashboard` | The read-only progress page and its embedded web app; its binary serves the page alone | `wellfed-formats`, `wellfed-local-api` (the loopback client of `wellfed-local`'s API, which links no relay or flash code) |

- **Shared files live in `wellfed-formats`** (`bench`): the progress document (`current.json`) and its writer, the run record (`NAME-itest.json`), the bench config, the session evidence layout, and the suite's scenario catalog, since one crate writes each and another reads it ([conventions](../../AGENTS.md#rust-conventions)). So does `wellfed-local`'s server config (`local`). The one loopback exchange the suite and the page both make with that server is its own crate, `wellfed-local-api`, which links no relay or flash code.
- **The page's read-only rule is structural:** `wellfed-dashboard` links neither the relay driver, the flash client nor the server; it reports the flash client by hashing its file. `make check-deps` enforces it (cargo-deny bans with wrappers, [Rust standard](rust.md#dependencies)).
- **`wellfed-flash` stays a child process** for every read and write. It is the audited writer with its own journals, reports and no-retry stops, and a fault in the bench must never take a write down with it.
- **The `wellfed-bench` binary keeps its commands and flags.** `--ui` links the dashboard. `itest` and `stock-ota` are parsed by the bench, then handed with the same arguments to the `wellfed-itest` binary (an `exec`, so the run, its interrupt and its exit status are the command's own): the suite links the bench's library, so the bench's binary cannot link the suite without a dependency cycle. Both binaries carry one build fingerprint, a hash over the sources of every first-party crate they link; the bench passes its own at the hand-off and the suite refuses a different one, so a stale `wellfed-itest` never runs old relay or cleanup code.

## Stages

Every hardware test runs through `wellfed-local` today, so it shrinks in step with the cards that replace each job.

| Stage | Waits on | Change |
| --- | --- | --- |
| 1 | Nothing | Delete stock control, the C-probe and modified-stock trials, `speak` and `ble-wifi`. The ledger keeps only install trials |
| 2 | Nothing | Split the bench into `wellfed-relay`, `wellfed-bench`, `wellfed-itest` and `wellfed-dashboard`, with their shared files in `wellfed-formats` (done October 25, 2026) |
| 3 | FW42 | The setup frame provisions stock: the broker trusts a new feeder on first use during an install, and `init` and `network` are deleted |
| 4 | FW45, FW47 | `wellfed-client` and the `wellfed` CLI (FW45: the BLE setup session and `wellfed setup`; FW47, Oct 24: the device API session, invites, recovery and the commands on it); the bench and suite talk to the feeder over the device API; the firmware's MQTT request path is deleted. The broker credentials go with the offers in stage 5 |
| 5 | FW48 | Offers over the device API (signing already moved to `wellfed-build`); the bench's own faults replace the fault server. Then the replacement's MQTT transport, the broker credentials in the build and the replacement's side of the broker are deleted (done October 25, 2026) |
| 6 | FW43 | What is left of the stock path moves into `wellfed-stock` and `wellfed-client`'s installer steps; `wellfed-local` is deleted |

The cards are FW51 (stage 1), FW52 (stage 2) and FW53 (stages 3–6) on the [firmware board](../../FIRMWARE_BOARD.md). FP02 retargets to the `wellfed` CLI, and FP04 reads stock's own plan records.

## Rules for the new crates

- **Single purpose.** A crate does one job, and a second consumer is a reason to split, not to grow.
- **Libraries do no IO they can leave to their caller.** `wellfed-client` is sans-IO; `wellfed-stock` takes its sockets from the caller where it can, so the server, the CLI and the bench drive it the same way.
- **Every pinned check survives the move.** The trial ledger's no-resend rule, the image hash binding and the evidence gates move unchanged, with their tests.
