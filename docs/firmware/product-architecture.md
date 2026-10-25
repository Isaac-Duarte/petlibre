# Product architecture

The owner's target (Oct 23, 2026): the Petlibro experience, fully open and usable offline. This page records the decisions, why each was chosen over its alternatives, and the SDK work they create. The cards live on [FIRMWARE_BOARD.md](../../FIRMWARE_BOARD.md) (FW44–FW50) and [FEATURE_BOARD.md](../../FEATURE_BOARD.md) (FP16, FP18). Everything here is design; nothing is physical evidence yet.

## Decisions

| Area | Decision | Rejected, and why |
| --- | --- | --- |
| Firmware | One `feeder` firmware, with the server optional | Separate connected and offline builds: every feeding guarantee, soak and OTA path would be proven twice, and "offline" is just the connected firmware without its server |
| State | The feeder owns plans, settings, keys and recent history | Server-owned state would contradict running with no server |
| Offline | No server and no cloud, but on Wi-Fi; public NTP allowed | — |
| Users | Owners with only the app, and self-hosters with a server | — |
| Tasks | The firmware uses two of its four tasks: the button service and one loop that runs the network and feeding | A task per client: each task needs its own stack holding Noise state, and four sessions would take every slot the firmware has ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees), `fw44-simon-01`) |
| Device API | The feeder hosts one message stream: typed requests and responses, plus pushed events and samples | REST plus an event stream: an HTTP parser and several connections cost RAM, and encryption would mean TLS. MQTT client: a phone can't host a broker |
| Encryption | Noise for every client | HMAC signing alone leaves schedules and keys readable on the LAN. TLS costs mbedTLS RAM, and self-signed certificates are hard for owners to trust |
| Setup | BLE, started only in a setup mode the owner asks for with a button hold, and ended by a reboot | Always-on BLE pays heap, tasks and radio time forever. The feeder's own access point makes the phone switch networks. Both paths would double the setup surface |
| More clients | An invite from a paired client, or a button-hold pairing window | Invites alone leave no way back if every client is lost; the button alone means a walk to the feeder every time |
| Concurrent edits | Each write names the generation it was based on, and a stale write is refused | Last writer wins can silently undo someone else's schedule change |
| Updates | The app or server fetches signed releases, and the owner approves each install | Automatic installs would put a late-misbehaving release on every feeder a pet depends on |
| Signing | The project key, plus an owner key enrolled for self-built images | — |
| Time without a source | Schedules hold, the feeder shows that it needs the time, and manual feeds still work | Resuming from a saved time shifts meals late and can repeat them; interval feeding is a new policy at wrong times |
| Clock validity | Valid until its error could exceed a bound | A fixed week is arbitrary; never expiring lets meals drift with no warning |
| On-device history | At least 7 days of events | — |
| Code layout | SDK mechanisms; a `wellfed-feeder` policy crate; thin firmware per model | Policy in the SDK would burden other device types; policy in each firmware gets copied per model |
| Server | A new `wellfed-server` crate | Growing `wellfed-local` would mix bench shortcuts and the stock protocol into the owners' product |
| Stock firmware | Used only to install and roll back | Ongoing stock control means two device protocols forever |
| Several feeders | The app manages each one directly | Requiring a server for a second feeder weakens the no-server promise |
| App | An Android app in Flutter over a sans-IO Rust core. It talks to feeders directly and to a server when one exists | Tauri: one UI, but mobile BLE and Wi-Fi need native plugins and the app feels like a web page. Native Swift/Kotlin: two or three UIs for one maintainer. iOS and a desktop app: not needed now |
| Web | Only the server's Svelte UI; the browser never talks to the feeder | Browsers have no raw TCP, no mDNS and no Web Bluetooth on iOS or Firefox, and Chrome restricts HTTPS pages reaching LAN devices. A UI served by the feeder costs flash and a second listener, and its page arrives over plain HTTP |
| No-server owners | Use the Android app | — |
| Installer | The Android app, and `wellfed-server` or the CLI | — |
| Distribution | APKs and server builds as GitHub releases | App stores and F-Droid later |
| Home Assistant | **Paused** (Oct 23) | See [Home Assistant](#home-assistant) |
| Notifications, remote access | **Deferred**; no notifications for now | See [Deferred](#deferred) |
| Hardware | The PLAF103 first; a device layer generic enough for other AmebaD Petlibro models | — |

## Components

```text
  Android app (Flutter) ── BLE (setup mode only) ──┐
      │   │                                        ▼
      │   │  Noise message stream (mDNS)       ┌────────┐   NTP
      │   └───────────────────────────────────►│ feeder │◄──────
      │ HTTP                                   └────────┘
      ▼                                            ▲
  wellfed-server (optional) ── Noise stream ──────┘
      history store, release fetching, installer
      ▲
      │ HTTP
  browser (server's Svelte UI)
```

In code:

- **SDK (`src/sdk/`):** mechanisms only. The runtime, clock and time sources, journal, feeding owner, buttons, event log, Noise transport, BLE setup mode and updater.
- **`wellfed-feeder` (`src/product/wellfed-feeder`):** a `no_std` library with the product policy. Schedules, child lock, alerts, the history policy, and the request and entity model the API exposes. It is host-tested, and the server's simulator can reuse it.
- **Firmware (`src/firmware/wellfed/feeder`):** the PLAF103's glue between the two. Another model gets its own thin firmware. It exists since FP11 with the panel's owner as its one loop task; the other owners join that loop, the feeding owner since October 25 (FW16).
- **`wellfed-client`:** a sans-IO Rust core. It turns bytes in into bytes out (the Noise handshake, pairing, invites, requests, event-cursor sync and the installer's steps) and never opens a socket or a radio. The server drives it with tokio, the Android app with Dart IO, and WASM can run it later.
- **`wellfed-server`:** the owner's server. Shared pieces (the Noise client, the request types, the stock install path) come out of `wellfed-local` and `wellfed-bench` into single-purpose libraries, signing moves to `wellfed-build`, and `wellfed-local` is then deleted ([tools roadmap](../development/tools-roadmap.md)).

## Frontends

- **Android app (Flutter).** It owns all IO through mature plugins: BLE (`flutter_blue_plus`), mDNS/NSD, sockets and joining a Wi-Fi network for the installer. All protocol logic goes through `wellfed-client` over `flutter_rust_bridge`, which also generates the Dart types. The app talks to feeders on the LAN directly, and to a `wellfed-server` over HTTP for long history and pet profiles when one exists. It can also run the stock installer: it joins stock's setup access point, sends the `DL` frame and serves the image in the foreground for about 3 minutes ([consumer installation](consumer-installation.md)).
- **Server web UI (Svelte, `src/web`).** A SvelteKit single-page app embedded in `wellfed-server`, sharing the bench's tokens and components, with its types generated from the server's `openapi.json` ([web UI design](../web/README.md)). Browsers reach the feeder only through the server.
- **Two UIs.** The app and the web UI are separate codebases. That cost is accepted for a native-feeling mobile app; Rust-generated types keep their data models identical.
- **Spike first.** Before building screens, a throwaway Android app proves the risky parts against a host-simulated feeder: BLE scanning and writes, NSD discovery on a real network, and a Noise session through `wellfed-client`.

## Setup and pairing

- **BLE runs only in setup mode.** A normal boot never starts the BLE stack. Holding Lock, then Mic, for 5 s on an unlocked panel starts it on the running feeder (FW45, [Bluetooth](bluetooth.md)). When setup finishes, or after 15 minutes without a session, the feeder reboots without BLE.
  - **Not automatic:** losing Wi-Fi does not start setup mode (owner, October 24, 2026). A moved feeder needs the button hold.
  - **Not at boot:** every KM0 backup register the SDK leaves free is taken (marker, diagnostics, two boot counts, heartbeat), so there is nowhere to carry "set up on the next boot" across a reset. Starting the stack live and rebooting to end it gives the same costs.
  - This never tears the stack down live. AmebaD's BT deinit is unverified on the bench and could leak heap.
  - **Consumer first boot:** a consumer image with no stored and no build credentials should start in setup mode. That waits for build credentials becoming optional (FW42); every development image has them.
  - A running feeder pays only BLE's flash cost: the linked stack and an 11.5 KB controller patch ([Bluetooth](bluetooth.md)). Heap, the C tasks (outside our stack audit) and shared radio time are spent only in setup mode.
- **First client.** In one BLE session (Noise XX) the app sends its own client key and learns the feeder's identity key. The app shows a one-time code of button presses, which the owner keys in on the feeder; both sides prove the code, bound to the handshake, so a relay in radio range cannot pose as either. Then the app sends the Wi-Fi credentials and its key becomes an owner key ([why a code](bluetooth.md#why-a-code)).
- **More clients:**
  - **Invite:** a paired client creates a short-lived invite (QR or link), which the new client redeems over the LAN. This is how the server and the CLI join.
  - **Button-hold window:** for a short time the feeder accepts one new client over the LAN. This is also recovery when every paired client is lost.
- **Keys.** Each client has its own key, which can be revoked without affecting the others. The FP15 reset clears them all. An owner can enroll an update-signing key through a paired client with a button confirmation.
- **Randomness.** Noise needs unpredictable ephemeral keys, and the carried FW08 item says the current nonce source is not a vetted RNG. FW45 owns the hardware random number generator and its health checks.
- **Removed bench shortcuts:** broker credentials as a build input, captured stock credentials, and `sudo nft` redirects. The installer (FW42) ends by handing the feeder to this pairing flow, not to a server.

## Device API

Built in FW47 (October 24, [device API](device-api.md)): sessions are Noise IK rather than KK, and the MQTT request path is gone.

- **Transport.** A Noise session over TCP, advertised over mDNS. Each client authenticates with its own key. There is no plaintext mode.
- **One message stream per client:**
  - typed requests and responses
  - pushed events behind a cursor
  - live samples for subscribers

  A Rust client library is shared by the app, the server and the CLI.
- **Writes** carry the generation they were based on, and a stale write gets a refusal naming the current generation. The FW15 settings store already counts generations.
- **One loop task.** A firmware gets four tasks, the button service's included ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)), and each needs its own stack, so the API runs in one task as a single loop:
  - it polls the listener and every client socket without blocking;
  - each pass does a bounded amount of work per client, then steps the feeding owner, so a flood or a malformed client costs only its turn and cannot stall feeding or the watchdog;
  - a Noise handshake is one step, and its time on the KM4 bounds the loop's latency (FW44 measures it);
  - the client limit and per-client buffers are fixed at build time (FW44); a client past the limit is refused.
- **One feeding owner.** The FW16/FW17 feeding owner lives only in the loop task. The button task only queues gestures ([buttons](buttons.md)), and the loop reads them, so a press and two clients reach the same owner in the same task and cannot double-feed. It stays in the loop rather than its own higher-priority task because a Wi-Fi association holds every task, a priority-4 one included, for 0.45–0.72 s (physical, [scheduling gaps](scheduling-gaps.md#result-october-25-2026-physical)): the stall is in the vendor driver, below the scheduler, so a priority does not keep a feed's 1 s beat. Instead feeds and joins take turns (`wellfed_hal::interlock`): no feed starts while a join attempt runs or waits (refused `Busy`), and a join waits in the Wi-Fi task until the owner is at rest with its outputs stopped, bounded by the longest feed. Storage writes take the same turn: no flash lease starts while a feed may move the outputs, since a lease checks them and resets the board when they move; a write that comes during a feed waits for the release, and a client's write is refused `Busy` meanwhile ([device API](device-api.md#in-the-firmware)).
- **The event cursor** is described under [data](#data).

## Time

- **Sources:** NTP when reachable, plus syncs from the app and the server over the API.
- **Validity is an error bound.** The SDK learns its drift from successive syncs and tracks how far off the clock could be. It stops counting as valid when that could exceed a bound (on the order of 2 minutes; FW46 sets it). The measured uncorrected drift is about 735 ppm slow ([clock](clock.md)), about 7 minutes a week. With good drift learning the clock should hold for weeks.
- **Warm resets** (watchdog, OTA activation, trial boots) should keep time if an always-on register or RTC survives them. FW46 checks this on hardware.
- **No source.** After a cold boot with no source, or once the bound is exceeded, scheduled meals hold. The feeder shows that it needs the time (FW23 indicator, FP12 sound policy), and the Dispense button and app feeds still work. FW21's missed-meal rule decides what happens once time returns.

## Updates

- The app or server fetches signed releases and shows them. The owner approves each install, which reaches the feeder as an offer over the API.
- The feeder never downloads from the internet, so it needs no TLS client.
- The FW10 download, the FW11 trial and confirmation, and the bank-0 recovery role are unchanged. Images are signed by the project key or the owner's enrolled key.

## Data

What the feeder can measure decides what anyone can store. On the PLAF103 (static and bench findings):

| Signal | Source | Status |
| --- | --- | --- |
| Feed requested, source, result | Feeding owner, journal | Host-tested; needs FW16/FW17 |
| Delivered portions | Outlet beam pulses | Unverified: no real pulse seen yet (FW13 carried to FW16) |
| Food level (ok, low, empty) | Food beam | Polarity known (FW13); levels need FP07 |
| Motor current per feed | ADC channel 4 | Uncalibrated until motion (FW14 carried to FW16) |
| Supply voltage, adapter or battery | ADC channel 5, PA19 | Physical (FW14) |
| Buttons, lock, resets | PB4–PB7 | Physical (FW20) |
| Connectivity, boots, updates, clock syncs | Runtime | Reported in health today |

The feeder has no scale, bowl sensor or camera, so it knows what it dispensed, never what the pet ate.

The goal is that anyone can do anything with their feeder's data: the server keeps everything raw, and features like battery health are queries over it, not firmware changes. The feeder sends two streams:

- **Durable events.** These are stored on the feeder in a ring that holds at least 7 days, each with a sequence number: feeds, faults, food-level changes, button presses and lock changes, setting changes with the client that made them, boots and reset reasons, updates and clock syncs. A feed is a summary: requested and delivered portions, duration, peak and mean motor current, and the result. A client that reconnects reads everything after its cursor. Records already lost to the ring are reported as lost, not skipped.
- **Live samples.** Everything the runtime samples (supply and battery voltage, adapter state, Wi-Fi RSSI, clock offset, food-beam state), at its own rate. They are sent only to subscribers and never stored on the feeder, so a sample missed during a server outage is gone. Raw per-feed traces stay in bring-up firmwares.

**On the server (FP18):**

- **Raw store.** Every event and sample is kept forever by default. The owner can prune, and can export everything as CSV or JSON (and import it to move servers).
- **Open schema.** Event and sample types live in `wellfed-proto`, versioned and documented. The feeder links that crate; `wellfed-formats` holds host-side files only.
- **Owner data the feeder doesn't hold:** pet profiles (name, photo, weight log, daily target) and food (brand, kcal per gram, bag size).
- **Derived views:**
  - Food remaining and a refill estimate: calibrated grams per portion (FW18) × portions since the last refill, against the bag size.
  - Grams and calories against each pet's target.
  - Feeding anomalies: missed, uncertain or short feeds.
  - Hardware health: motor current trends, battery curves, Wi-Fi drops, resets and clock drift.
  - An audit log of who did what.

## Home Assistant

Paused by the owner (Oct 23). The options when it resumes:

- **ESPHome native API on the feeder.** Auto-discovered, with no add-on and no server. But it reimplements an unofficial protocol that changes with HA releases, and it spends firmware RAM. Prove it first with a host stub that a real HA instance adopts.
- **MQTT discovery through `wellfed-server`.** A documented and stable protocol with no HA code in the firmware. But it needs the server and a broker.

The entity model in `wellfed-feeder` should not assume either one.

## Deferred

- **Notifications.** None for now. Push on iOS and Android goes through Apple and Google services, which need credentials tied to the app's publisher, so a self-hosted server can't send them alone. The options are ntfy or UnifiedPush, a small project relay, or both.
- **Remote access.** Only through the owner's server; the owner's VPN is one route.

## SDK work

| Card | Adds |
| --- | --- |
| FW44 Resource budget | RAM, flash, stack and task budget for the product image, including setup-mode BLE and Noise, before the protocol work commits: the two firmware tasks, the handshake time in the loop, the client limit and buffers |
| FW45 Client keys and pairing | Hardware RNG, per-client keys, boot-gated BLE setup mode, invites, the button-hold window, revocation, owner signing-key enrollment |
| FW46 Offline time | NTP, drift learning, the error-bound validity, time across warm resets, the no-source hold |
| FW47 Device-hosted API | The Noise message stream, mDNS, generation-checked writes, the event cursor and samples, several clients served by one loop task |
| FW48 Updates over the device API | Offers from any paired client, the project key and an owner key |
| FW49 ESPHome native API | Paused with Home Assistant |
| FW50 Device layer for other models | Audit the HAL and `device.json` for PLAF103 assumptions |
