# Host tools roadmap

Where each host tool is going as the product platform lands, decided October 24, 2026. The [tools index](../../src/tools/README.md) describes what each tool does today. The direction: single-purpose crates that the owner's tools and the bench link, and no catch-all binary.

## Today

| Tool | Lines | Fate |
| --- | ---: | --- |
| `wellfed-flash` | 8.2k | Stays: the audited UART writer, always run as its own process. What other crates checked through it moved to `wellfed-image` (Oct 26), so no crate links it |
| `wellfed-build` | 7.3k | Stays, with `keygen` and `sign` for update signing over `wellfed-signing`, which the bench links instead of the builder (Oct 26) |
| `wellfed-audit`, `wellfed-emu`, `wellfed-formats`, `xtask` | 10.8k | Stay |
| `wellfed-bench` | 15k | Split into single-purpose crates (`wellfed-relay`, `wellfed-bench`, `wellfed-itest`, `wellfed-dashboard`, Oct 25); stays dev-only |
| `wellfed-stock` | 6.4k | Stays: the stock 1.1.15 protocol, its broker, the setup frame, the image server and the install-trial ledger, and the bench's server binary over them (`serve`, `ota-images`, the trial commands, `init`, `setup-send`, `network`) ([stock server](stock-server.md)) |
| `wellfed-stock-api` | 0.2k | Stays: the loopback client of that server's API, for the suite and the page; it links nothing of the server |

## Where `wellfed-local` went (deleted October 26, 2026)

`wellfed-local` was six tools sharing one config and one MQTT broker, and none of them was a product:

| Job | Commands | Destination |
| --- | --- | --- |
| Stock control (research era) | `feed`, `resolve`, `new-request-id`, `/api/feed`, the feed records in the ledger | **Deleted** (stage 1). The product uses stock only to install and roll back ([product architecture](../firmware/product-architecture.md)) |
| C-probe and modified-stock research | `probe-query`, the `1.1.16` and C-probe trial images | **Deleted** (stage 1). The research closed on October 22 |
| Direct-to-feeder bring-up | `speak`, `ble-wifi` | **Deleted** (stage 1). Their firmwares are bring-up only, and FW45 replaces BLE setup |
| Stock installer (FW40–FW43) | `ota-images`, `ota-trial`, `ota-trial-resolve`, `ota-trial-answered`, `setup-send`, the stock broker; `init` and `network` (captured credentials, sudo redirect) | **`wellfed-stock`**: the protocol **moved** Oct 25, and the commands, the loopback API and the `network` redirect **moved** Oct 26 into its binary, unchanged. The installer's steps go to **`wellfed-client`** with FW42, and `init` and `network` go once the FW42 setup frame works |
| Replacement MQTT transport (FW08) | `firmware-credential`, `firmware-request-key`, the firmware broker role, `/api/replacement/request` | **Deleted**: requests with FW47, and `firmware-credential`, the broker role and the replacement's health in the status with the transport (Oct 25). A config's leftover `firmware` section is ignored |
| Replacement OTA (FW10) | `ota-offer`, the fault-server example | Signing **moved** to `wellfed-build keygen` and `sign` (version 2 manifests, `downgrade`, `revoke`, `add-key`), behind a frozen review (Oct 25). Offers **moved** to the device API: `wellfed_client::update`, which the bench drives with its own faults. `ota-offer`, the fault server and the version 1 manifest are **deleted** (Oct 25) |

The suite runs `wellfed-stock` as a child process (`serve`, `serve --enable-ota-trial` and `ota-images`) and speaks its HTTP routes (status and the trial: request, resolve and answered) through `wellfed-stock-api`; the bench signs by linking `wellfed-build` and offers over the device API, with no server.

## Where it ends up

```text
wellfed-client   lib, sans-IO   device API sessions and pairing; the installer's steps
wellfed-model    lib            the device API's values, owned, as the CLI, the app and the server share them
wellfed-stock    lib + binary   the stock 1.1.15 protocol, only what install and rollback need:
                                 the legacy MQTT 3.1 broker, OTA_UPGRADE and its trial ledger,
                                 the setup frame, reading stock's plan records for FP04; the
                                 binary is the bench's stock server
wellfed          CLI            the owner's command line: pair, feed, plans, settings, history,
                                 offer an update, install from stock, roll back (FP02)
wellfed-server   binary         the owner's server, its web UI, and installs
wellfed-build    CLI            builds, reviews, and update signing (keygen, sign)
wellfed-signing  lib            update signing behind a frozen review, for the builder's
                                 commands, the bench and, later, the server's installs
wellfed-image    lib            the OTA image, the banks and frozen reviews, as every tool
                                 checks them
wellfed-flash    CLI            unchanged; every bench write still goes through it as a process
```

## The bench, split

The bench's modules fell into separate jobs with one-way dependencies, and each is now its own crate (October 25, 2026; the rig came out of the bench on October 26):

| Crate | Holds | Depends on |
| --- | --- | --- |
| `wellfed-relay` | The SH-UR04A relay board driver: power, the loader strap and the UART lines | Nothing first-party |
| `wellfed-rig` | The operator sequences: power, loader, boot, capture, dump, the UART update and recovery chains, transients, the OTA loop, and the device API and server steps the suite builds on | `wellfed-relay`, `wellfed-formats`, `wellfed-client`, `wellfed-signing`, `wellfed-build` and `wellfed-flash` (each as a process: every build, every read and write) |
| `wellfed-itest` | The integration suite: scenarios, the live and scripted rigs, `stock-ota`, and their arguments | `wellfed-rig`, `wellfed-relay`, `wellfed-formats` |
| `wellfed-bench` | The binary: the command line over the rig's sequences and the suite, one process | `wellfed-rig`, `wellfed-itest`, `wellfed-dashboard` (with `ui`) |
| `wellfed-dashboard` | The read-only progress page and its embedded web app; its binary serves the page alone | `wellfed-formats`, `wellfed-stock-api` (the loopback client of the stock server's API, which links no relay or flash code) |

- **Shared files live in `wellfed-formats`** (`bench`): the progress document (`current.json`) and its writer, the run record (`NAME-itest.json`), the bench config, the session evidence layout, and the suite's scenario catalog, since one crate writes each and another reads it ([conventions](../../AGENTS.md#rust-conventions)). So do the stock server's config, status document and image lease (`stock`). The one loopback exchange the suite and the page both make with that server is its own crate, `wellfed-stock-api`, which links no relay or flash code.
- **The page's read-only rule is structural:** `wellfed-dashboard` links neither the relay driver, the flash client nor the server; it reports the flash client by hashing its file. `make check-deps` enforces the first two (cargo-deny bans with wrappers, [Rust standard](rust.md#dependencies)). The server holds because no crate depends on `wellfed-stock`: cargo-deny cannot ban every dependent without banning the crate itself, so `cargo xtask check-repository` refuses a manifest under `src/` that names it.
- **`wellfed-flash` stays a child process** for every read and write. It is the audited writer with its own journals, reports and no-retry stops, and a fault in the bench must never take a write down with it.
- **The `wellfed-bench` binary keeps its commands and flags.** `--ui` links the dashboard. The suite links the rig, not the bench, so the bench links the suite and runs `itest` and `stock-ota` in its own process. Until October 26 the suite linked the bench's library, so the bench `exec`ed a second binary, `wellfed-itest`, guarded by a build fingerprint against a stale one; one binary needs neither.

## Stages

`wellfed-local` shrank in step with the cards that replaced each job, and was deleted at stage 6.

| Stage | Waits on | Change |
| --- | --- | --- |
| 1 | Nothing | Delete stock control, the C-probe and modified-stock trials, `speak` and `ble-wifi`. The ledger keeps only install trials |
| 2 | Nothing | Split the bench into `wellfed-relay`, `wellfed-bench`, `wellfed-itest` and `wellfed-dashboard`, with their shared files in `wellfed-formats` (done October 25, 2026) |
| 3 | FW42 | The setup frame provisions stock: the broker trusts a new feeder on first use during an install, and `init` and `network` are deleted from `wellfed-stock` (host-tested October 26: `init-installer`, `serve --enroll-minutes` and `wellfed install`, with the steps in `wellfed_client::install`; `init` and `network` stay until the frame provisions stock on hardware) |
| 4 | FW45, FW47 | `wellfed-client` and the `wellfed` CLI (FW45: the BLE setup session and `wellfed setup`; FW47, Oct 24: the device API session, invites, recovery and the commands on it); the bench and suite talk to the feeder over the device API; the firmware's MQTT request path is deleted. The broker credentials go with the offers in stage 5 |
| 5 | FW48 | Offers over the device API (signing already moved to `wellfed-build`); the bench's own faults replace the fault server. Then the replacement's MQTT transport, the broker credentials in the build and the replacement's side of the broker are deleted (done October 25, 2026) |
| 6 | FW43 | What is left of the stock path moves into `wellfed-stock`, and `wellfed-local` is deleted (done October 26, 2026, ahead of FW42 and FW43 by the owner's decision: the commands moved unchanged into `wellfed-stock`'s binary). `wellfed-client`'s installer steps and the rollback come with FW42 and FW43 |

The cards were FW51 (stage 1), FW52 (stage 2) and FW53 (stages 3–6) on the [firmware board](../../FIRMWARE_BOARD.md); FW53 was done as architecture card AR12 (October 26; the architecture board is retired, `git show 4af1b56:ARCHITECTURE_BOARD.md`). FP02 retargets to the `wellfed` CLI, and FP04 reads stock's own plan records.

## Rules for the new crates

- **Single purpose.** A crate does one job, and a second consumer is a reason to split, not to grow.
- **Libraries do no IO they can leave to their caller.** `wellfed-client` is sans-IO; `wellfed-stock` takes its sockets from the caller where it can, so the server, the CLI and the bench drive it the same way.
- **Every pinned check survives the move.** The trial ledger's no-resend rule, the image hash binding and the evidence gates move unchanged, with their tests.
