# Architecture board (temporary)

Temporary board for two tracks from the October 24, 2026 architectural review (static analysis): **layering** (make the SDK / product / firmware / client split real) and **retiring `wellfed-local`**. Temporary: when a track settles, fold its surviving cards into [FIRMWARE_BOARD.md](FIRMWARE_BOARD.md) and the [tools roadmap](docs/development/tools-roadmap.md), then delete this file.

Rules for these cards:
- One worktree per card; `make check` for each, plus a fresh image build with its link, RAM-park and stack audits for any card that changes a firmware crate's dependencies.
- No merge to `master` until the owner says FW16's active work has landed.
- Behavior-neutral unless the card says otherwise: wire formats, the `wellfed_*` C exports, pinned recovery checks and the trial ledger's rules move unchanged.
- New crate names are a compatibility change AGENTS.md allows only when the task requires it; each card names the ones it adds.

## Order

1. **Now, in parallel (host only):** AR1, AR2 + AR13 → AR3, AR4, AR19. AR15 changes firmware code but no behavior.
2. **Next (firmware imports change):** AR5, then AR6.
3. **Owner decision:** AR7, before FW48 and FW53 delete anything.
4. **After FW16 lands:** AR8, AR9, then AR10, then AR11.
5. **Last:** AR12.

AR1 goes first: it is the smallest card and closes a live gap (any container can be signed without a review), and it does not depend on anything else here.

## Cards

| Card | Status | Depends on | Done when |
| --- | --- | --- | --- |
| **AR1 Signing in `wellfed-build`, behind a frozen review** | Ready | — | `wellfed-build keygen` and `sign` exist; `sign` takes a review directory and refuses an `image.bin` whose hash, build id or motion policy differs from its frozen review. The bench links the library instead of running `wellfed-local ota-sign`. `ota-keygen`/`ota-sign` are removed from `wellfed-local`. The bench's OTA staging refuses a directory with no `review.json`. `make check` passes. |
| **AR2 A sans-IO `Connection` in `wellfed-client`** | Ready | — | `wellfed-client` has a `Connection` (`handle_input`, `poll_transmit`, `poll_timeout`/`handle_timeout`, `request`, `poll_event`) that owns keepalive, silence detection, request–answer matching, the event cursor and LAN recovery. `wellfed-connect` is a thin tokio loop over it; `wellfed-service` and the CLI keep no session logic (the CLI's hand-built recovery frame is gone). Wire bytes unchanged: the v1 protocol fixtures pass untouched. |
| **AR3 `wellfed-model`: owned types shared by client and server** | Ready | AR2 | A new `wellfed-model` crate holds owned `Status`, `Settings`, `Event` and friends plus the proto→model conversion (moved from `wellfed-service/src/convert.rs`). `wellfed-client`'s `Connection` emits model types; `wellfed-api` uses them. No borrowed type crosses the client's public surface. `openapi.json` regenerated with no unintended diff. |
| **AR4 `wellfed-stock` library** | Ready | — | The stock 1.1.15 protocol, its broker, the setup frame and the install-trial ledger live in a `wellfed-stock` library, moved unchanged with their tests (no-resend rule, image binding, evidence gates). `wellfed-local` is a thin binary over it with identical commands and flags. `wellfed-bench` host tests for `ota`, `itest` and `stock-ota` pass unchanged. |
| **AR5 `wellfed-session` out of `wellfed-core`** | Ready | — | `noise`, `mac`, `setup_code`, `frame`, `hex` and `base64` live in `wellfed-session`. `wellfed-api` no longer depends on `wellfed-core`. `wellfed-formats` uses the session crate's hex; the bench uses `wellfed_core::ota` instead of its second `PLOTA1` parser. Fresh `feeder` and `qa` image builds pass their audits. |
| **AR6 The replacement's MQTT transport fenced off** | Ready after AR5 | AR5 | `wellfed_core::transport` and the broker side it talks to share one `wellfed-ota-mqtt` crate that the HAL's offer watch and bank-0 updater depend on. `wellfed-core` no longer contains MQTT. The crate's doc says what bank 0 requires of it and that its wire format is frozen. Fresh image builds pass their audits. |
| **AR7 Decide what bank 0 is** | Needs owner | — | The owner has chosen, and the docs record, one of: (a) bank 0 speaks the device API and runs the feeder role (needs a reviewed UART reinstall of bank 0), or (b) a minimal offer broker survives in `wellfed-server` for recovery. FW48 and FW53 are rewritten to match. Recommended: (a), which also makes a failed update land on a build that still feeds. |
| **AR8 Product schema out of the SDK** | Blocked (FW16 uncommitted in these files) | AR5 | `wellfed-proto` lives under `src/product/` (name unchanged). The settings, client, event and Wi-Fi stores are in `wellfed-feeder::store`; the journal, slot and seed stores stay in core. A `wellfed-board-plaf103` crate holds the pins and storage sector map; `wellfed-amebad-sys` is bindings and the fake only. No storage layout changes. Fresh image builds pass their audits. |
| **AR9 The feeding owner in its own task** | Blocked (FW16; bench measurement) | — | A bench measurement (no motion) shows the gap a priority-4 task sees during WPA3 rejoins. If priority helps: the owner runs in `wellfed-feed` at priority 4 through a cross-task `Shared<T>` container, the loop keeps submit/follow/events, the runtime checks the loop's liveness, and the `owner-stall` variant is split into loop and owner stalls. If it doesn't: rejoins are held during a feed and a feed refuses to start during a join. The stack audit lists the new task with its bound. The task docs are updated. |
| **AR10 Thin firmware: one loop pass shared with the simulator** | Planned | AR8, AR9 | `wellfed-feeder` has `Feeder<P: Platform>::pass(now)` covering the API socket driver, the setup-mode driver and the event recorder. The PLAF103 firmware is pin and capability wiring. `wellfed-feeder-sim` calls the same pass; its copied loop and second `Device` are deleted. A `Device` conformance suite runs against both. |
| **AR11 Simulator fidelity** | Planned | AR10 | The plant is parameterized from FW16 captures (PA18 marks, missed and double edges, jitter) instead of an edge per 10 ms. Liveness invariants exist ("after any cut a new feed id is eventually accepted", "a feed completes under background flash writes"), each landing with the fix that makes it pass. `python_dumps` is gone. |
| **AR12 Delete `wellfed-local`** | Planned | AR1, AR4, AR6, AR7, FW43, FW48 | The crate is deleted; `stock-roundtrip` and the standard tier pass on the research unit through the new crates; no doc names `wellfed-local` outside history (closes FW53). |
| **AR13 Protocol evolution rules, while v1 has no outside clients** | Ready | — (pairs with AR2) | Written in the device API doc and enforced by tests: (1) the IK prologue `wellfed-api-v1` never changes; versions are negotiated in `Hello.major`, and a test shows a major-2 hello gets the typed `Version` refusal; (2) every new op, event kind or message field is gated by a `Capabilities` bit both sides claim, as `FEED` already is, and a test shows a `V1`-only session never receives a non-`V1` event body; (3) existing message structs never gain fields (postcard is positional), so additions are new variants behind a bit; (4) the event history carries a random epoch (in `Welcome` or `Status`, and in the server's event key), so a re-created ring is detected instead of silently freezing the mirror; (5) the compatibility policy is clients adapting to the feeder's capabilities indefinitely, not "N and N−1", because owners can hold an update forever. (4) changes v1 bytes: do it now, regenerating fixtures, since every client is in this repository. |
| **AR15 Sound safe APIs at the FFI and ISR edge** | Ready | — | `audio_page()` hands out a `Page<'a>` guard borrowed from the audio handle, with `submit(self)` consuming it, instead of an aliasing `&'static mut`. `wellfed_wifi_outcome` is no longer a safe `pub extern` dereferencing a static mutably. `Board::with` refuses reentry. Firmwares reach motor pins, the watchdog and storage erase only through the HAL's owners, not as safe raw `sdk::` functions. Fresh image builds pass their audits. |
| **AR16 Bring-up firmwares as variants of one** | Planned | AR10 | `optics`, `buttons`, `adc`, `speaker`, `motor` and `trial` (40–79 lines each) become variants of one bring-up firmware through the existing variant mechanism; policy names, markers and record formats unchanged; the bench's `--firmware` names still resolve. |
| **AR17 One dual-slot store with one torn-write policy** | Planned | AR8 | The journal, settings, client, seed, Wi-Fi and event stores share one slot abstraction and one documented torn-write rule, each keeping its on-flash layout byte for byte; the power-cut-at-every-mutation tests run once against the abstraction and per store. |
| **AR18 Link liveness without internet DNS** | Planned | AR7 | The C Wi-Fi task no longer resolves a hard-coded `example.com` every 5 s and drops the link after two failures. Liveness comes from association, DHCP and the gateway, decided in host-tested Rust, and trial confirmation stops depending on internet DNS. A behavior change: needs a bench run with the hotspot's forwarding blocked. |
| **AR19 Fuzz and property-test the network parsers** | Ready | — | `cargo-fuzz` targets (or `proptest` where fuzzing can't run in `make check`) for the device API's frame and request decoding, the Noise handshake reader, the setup protocol, the OTA manifest parser and the MQTT transport codec, with a short bounded run in `make check` and longer runs on demand. |
| **AR14 Minimal CI** | Deferred (owner, Oct 25) | — | A workflow on every push runs `cargo fmt --check`, the workspace Clippy passes including the firmware target, nextest, `check-deps` and `check-web`, and the well-fed.dev deploy waits on it. Fast enough to keep (target under 15 min with caching). Until then, each refactor card runs `make check` (and `make check-web` where it applies) in its worktree before it merges. |

## Summary

| Card | Status |
| --- | --- |
| AR1 Signing behind a frozen review | 🟢 Ready |
| AR2 Sans-IO `Connection` | 🟢 Ready |
| AR3 `wellfed-model` | 🟢 Ready after AR2 |
| AR4 `wellfed-stock` | 🟢 Ready |
| AR5 `wellfed-session` | 🟢 Ready |
| AR6 MQTT transport fenced off | 🟢 Ready after AR5 |
| AR7 Bank 0 decision | 🟡 Needs owner |
| AR8 Product schema out of the SDK | 🔴 Blocked |
| AR9 Owner task | 🔴 Blocked |
| AR10 Thin firmware | ⬜ Planned |
| AR11 Simulator fidelity | ⬜ Planned |
| AR12 Delete `wellfed-local` | ⬜ Planned |
| AR13 Protocol evolution rules | 🟢 Ready |
| AR15 Sound safe FFI APIs | 🟢 Ready |
| AR16 Bring-up firmwares as variants | ⬜ Planned |
| AR17 One dual-slot store | ⬜ Planned |
| AR18 Link liveness without internet DNS | ⬜ Planned |
| AR19 Fuzz the network parsers | 🟢 Ready |
| AR14 Minimal CI | ⏸ Deferred |
