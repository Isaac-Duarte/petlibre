# Host tools roadmap

Where each host tool is going as the product platform lands, decided October 24, 2026. The [tools index](../../src/tools/README.md) describes what each tool does today. The direction: single-purpose crates that the owner's tools and the bench link, and no catch-all binary.

## Today

| Tool | Lines | Fate |
| --- | ---: | --- |
| `wellfed-flash` | 8.2k | Stays: the audited UART writer, always run as its own process |
| `wellfed-build` | 7.3k | Stays, and gains `keygen` and `sign` for update signing |
| `wellfed-audit`, `wellfed-emu`, `wellfed-formats`, `xtask` | 10.8k | Stay |
| `wellfed-bench` | 15k | Splits into single-purpose crates; stays dev-only |
| `wellfed-local` | 7.3k | Splits up and is deleted |

## Why `wellfed-local` goes

`wellfed-local` is six tools sharing one config and one MQTT broker, and none of them is a product:

| Job | Commands | Destination |
| --- | --- | --- |
| Stock control (research era) | `feed`, `resolve`, `new-request-id`, `/api/feed`, the feed records in the ledger | **Deleted** (stage 1). The product uses stock only to install and roll back ([product architecture](../firmware/product-architecture.md)) |
| C-probe and modified-stock research | `probe-query`, the `1.1.16` and C-probe trial images | **Deleted** (stage 1). The research closed on October 22 |
| Direct-to-feeder bring-up | `speak`, `ble-wifi` | **Deleted** (stage 1). Their firmwares are bring-up only, and FW45 replaces BLE setup |
| Stock installer (FW40–FW43) | `ota-images`, `ota-trial`, `ota-trial-resolve`, `setup-send`, the stock broker; `init` and `network` (captured credentials, sudo redirect) | **`wellfed-stock`** (the protocol) and **`wellfed-client`** (the installer's steps). `init` and `network` go once the FW42 setup frame works |
| Replacement MQTT transport (FW08) | `firmware-credential`, `firmware-request-key`, the firmware broker role, `/api/replacement/request` | **Deleted** once the device API (FW47) carries every request |
| Replacement OTA (FW10) | `ota-keygen`, `ota-sign`, `ota-offer`, the fault-server example | Signing to **`wellfed-build`**; offers to the device API (FW48), with the fault server rebuilt on `wellfed-client` |

The bench drives `wellfed-local` as a child process today (`serve`, `ota-sign`, `ota-offer`, `ota-images`, `status`, and four HTTP routes). Linking libraries replaces that.

## Where it ends up

```text
wellfed-client   lib, sans-IO   device API sessions and pairing; the installer's steps
wellfed-stock    lib            the stock 1.1.15 protocol, only what install and rollback need:
                                 the legacy MQTT 3.1 broker, OTA_UPGRADE and its trial ledger,
                                 the setup frame, reading stock's plan records for FP04
wellfed          CLI            the owner's command line: pair, feed, plans, settings, history,
                                 offer an update, install from stock, roll back (FP02)
wellfed-server   binary         the owner's server, its web UI, and installs
wellfed-build    CLI            builds, reviews, and now update signing (keygen, sign)
wellfed-flash    CLI            unchanged; every bench write still goes through it as a process
```

## The bench, split

The bench's modules already fall into separate jobs with one-way dependencies. Each becomes its own crate:

| Crate | Holds | Depends on |
| --- | --- | --- |
| `wellfed-relay` | The SH-UR04A relay board driver (`relay.rs`): power and the loader strap | Nothing first-party |
| `wellfed-bench` | The binary and the operator sequences: power, loader, boot, capture, dump, the UART update and recovery chains, transients, the OTA loop | `wellfed-relay`, `wellfed-formats`, `wellfed-client`, `wellfed-build` (library), `wellfed-flash` (as a process) |
| `wellfed-itest` | The integration suite: scenarios, the scripted test rig, run records, `stock-ota` | `wellfed-bench`'s rig, `wellfed-stock`, `wellfed-client` |
| `wellfed-dashboard` | The read-only progress page and its embedded web app | `wellfed-formats` only |

- **Shared files move to `wellfed-formats`:** the progress document (`current.json`), the run record (`NAME-itest.json`) and the bench config, since one crate writes each and another reads it ([conventions](../../AGENTS.md#rust-conventions)).
- **The page's read-only rule becomes structural:** `wellfed-dashboard` cannot link the relay, the flash client or the server.
- **`wellfed-flash` stays a child process.** It is the audited writer with its own journals, reports and no-retry stops, and a fault in the bench must never take a write down with it.
- The `wellfed-bench` binary still offers `itest` and `--ui` by linking the other two crates.

## Stages

Every hardware test runs through `wellfed-local` today, so it shrinks in step with the cards that replace each job.

| Stage | Waits on | Change |
| --- | --- | --- |
| 1 | Nothing | Delete stock control, the C-probe and modified-stock trials, `speak` and `ble-wifi`. The ledger keeps only install trials |
| 2 | Nothing | Split the bench into `wellfed-relay`, `wellfed-bench`, `wellfed-itest` and `wellfed-dashboard`, with their shared files in `wellfed-formats` |
| 3 | FW42 | The setup frame provisions stock: the broker trusts a new feeder on first use during an install, and `init` and `network` are deleted |
| 4 | FW45, FW47 | `wellfed-client` and the `wellfed` CLI (begun Oct 24 with FW45: the BLE setup session and `wellfed setup`); the bench and suite talk to the feeder over the device API; the MQTT firmware role and its credentials are deleted |
| 5 | FW48 | Offers over the device API; signing moves to `wellfed-build`; the fault server is rebuilt on `wellfed-client` |
| 6 | FW43 | What is left of the stock path moves into `wellfed-stock` and `wellfed-client`'s installer steps; `wellfed-local` is deleted |

The cards are FW51 (stage 1), FW52 (stage 2) and FW53 (stages 3–6) on the [firmware board](../../FIRMWARE_BOARD.md). FP02 retargets to the `wellfed` CLI, and FP04 reads stock's own plan records.

## Rules for the new crates

- **Single purpose.** A crate does one job, and a second consumer is a reason to split, not to grow.
- **Libraries do no IO they can leave to their caller.** `wellfed-client` is sans-IO; `wellfed-stock` takes its sockets from the caller where it can, so the server, the CLI and the bench drive it the same way.
- **Every pinned check survives the move.** The trial ledger's no-resend rule, the image hash binding and the evidence gates move unchanged, with their tests.
