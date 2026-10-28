# Product architecture

This page records the owner's decisions about what Wellfed is building, and why each one beat its alternatives. It is for anyone who wants to know why the firmware, app and server look the way they do. The goal is the feel of the vendor's app experience, fully open and usable offline.

Most of this is design. What has been built and observed is on the [roadmap](../roadmap.md).

## Decisions

### The firmware

- **One `feeder` firmware, with the server optional.** Separate connected and offline builds would mean proving every feeding guarantee, soak test and update path twice. "Offline" is just the connected firmware without its server.
- **The feeder owns its plans, settings, keys and recent history.** Server-owned state would contradict running with no server.
- **Offline means no server and no cloud, but still on Wi-Fi.** Public NTP is allowed.
- **Two kinds of owner.** Some have only the app; self-hosters also run a server.
- **Two of the firmware's four tasks:** the button service, and one loop that runs the network and feeding. A task per client was rejected: each task needs its own stack holding Noise state, and four sessions would take every slot the firmware has ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)).
- **Code layout:** mechanisms in the SDK, product policy in a `wellfed-feeder` crate, and a thin firmware per model. Policy in the SDK would burden other device types. Policy in each firmware would get copied per model.
- **Hardware:** the PLAF103 first, with a device layer generic enough for other AmebaD models from the same vendor ([device layer](device-layer.md)).

### Talking to the feeder

- **The feeder hosts one message stream per client:** typed requests and responses, plus pushed events and samples.
  - REST with an event stream was rejected: an HTTP parser and several connections cost RAM, and encryption would mean TLS.
  - An MQTT client was rejected: a phone can't host a broker.
- **Noise encryption for every client.** HMAC signing alone leaves schedules and keys readable on the LAN. TLS costs mbedTLS RAM, and owners find self-signed certificates hard to trust.
- **Concurrent edits:** each write names the generation it was based on, and a stale write is refused. Last-writer-wins can silently undo someone else's schedule change.
- **Several feeders:** the app manages each one directly. Requiring a server for a second feeder would weaken the no-server promise.

### Setup and more clients

- **Setup over Bluetooth LE, only in a setup mode** the owner asks for with a button hold, ended by a reboot.
  - Always-on BLE would pay heap, tasks and radio time forever.
  - The feeder's own access point would make the phone switch networks.
  - Offering both would double the setup surface.
- **More clients join by an invite from a paired client, or through a button-hold pairing window.** Invites alone leave no way back if every client is lost. The button alone means a walk to the feeder every time.

### Updates

- **The app or server fetches signed releases, and the owner approves each install.** Automatic installs would put a late-misbehaving release on every feeder a pet depends on.
- **Signing:** the project key, plus an owner key enrolled for self-built images.

### Time

- **With no time source, schedules hold, the feeder shows that it needs the time, and manual feeds still work.** Resuming from a saved time shifts meals late and can repeat them. Falling back to interval feeding is a new policy at the wrong times.
- **The clock counts as valid until its error could exceed a bound.** A fixed week is arbitrary. Never expiring lets meals drift with no warning.

### Data

- **The feeder keeps at least 7 days of events.**

### Apps, server and web

- **A new `wellfed-server` crate.** Growing the bench's server (now `wellfed-stock`'s binary) would mix bench shortcuts and the stock protocol into the owners' product.
- **Stock firmware is used only to install and roll back.** Ongoing stock control would mean two device protocols forever.
- **An Android app in Flutter, over a sans-IO Rust core.** It talks to feeders directly, and to a server when one exists.
  - Tauri was rejected: one UI, but mobile BLE and Wi-Fi need native plugins, and the app feels like a web page.
  - Native Swift and Kotlin would mean two or three UIs for one maintainer.
  - iOS and a desktop app aren't needed now.
- **The only web UI is the server's Svelte UI, and the browser never talks to the feeder.** Browsers have no raw TCP, no mDNS, and no Web Bluetooth on iOS or Firefox. Chrome restricts HTTPS pages from reaching LAN devices. A UI served by the feeder would cost flash and a second listener, and its page would arrive over plain HTTP.
- **Owners without a server use the Android app.**
- **The installer** is the Android app, and `wellfed-server` or the CLI.
- **Distribution:** APKs and server builds as GitHub releases. App stores and F-Droid come later.
- **App and server together.** When a server is configured, the app goes through it first. It goes straight to the feeder on the LAN only when the server is down, and an uncertain write never switches path ([two paths, one feed](../server/README.md#two-paths-one-feed)). Going app-direct only was rejected: no remote access, and each person's actions would reach the audit log only through the feeder.
- **Notifications** come later, through the Android app's push. The server decides what is an alert ([deferred](#deferred)).
- **Remote access** goes through a reverse proxy in front of the server, or the owner's VPN ([deferred](#deferred)).
- **Home Assistant is paused** ([below](#home-assistant)).

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

| Part | Where | What it holds |
| --- | --- | --- |
| SDK | `src/sdk/` | Mechanisms only: the runtime, clock and time sources, journal, feeding owner, buttons, Noise transport, the BLE link and the updater |
| Product policy | `src/product/wellfed-feeder` | Schedules, child lock, alerts, history policy, the request and entity model, the product's stores, and the loop itself |
| Product runtime | `src/product/wellfed-feeder-runtime` | The product's stores on the SDK runtime: the settings writer and its mailboxes |
| Firmware | `src/firmware/wellfed/feeder` | The PLAF103's wiring, and the feeding owner stepped in the loop |
| Client core | `wellfed-client` | The sans-IO protocol logic every client shares |
| Server | `wellfed-server` | The owner's server |

Some detail on each:

- **SDK.** No SDK crate depends on a product crate. Where the runtime needs the product's stores (its settings writer, the Wi-Fi join's stored credentials), it calls a hook the product's runtime crate defines.
- **`wellfed-feeder`** is a `no_std` library. Its stores keep the product's records: settings, Wi-Fi credentials, client keys and events. Its loop, `Feeder<P: Platform>::pass`, is the panel's owner, BLE setup mode, the device API's driver and the event recorder, over a `Platform` trait. It is host-tested, and the simulator runs the same pass on its own platform.
- **`wellfed-feeder-runtime`** is linked by every PLAF103 firmware, so a firmware swap keeps the owner's data.
- **The firmware** supplies the `Platform` (pins, drivers, sockets, mailboxes) and steps the feeding owner in one loop task, before the product's pass. Another model gets its own thin firmware.
- **`wellfed-client`** turns bytes in into bytes out: the Noise handshake, pairing, invites, requests, event-cursor sync and the installer's steps. It never opens a socket or a radio. The server drives it with tokio, the Android app with Dart IO, and WASM can run it later.
- **`wellfed-server`** links single-purpose libraries for its shared pieces: the Noise client and request types in `wellfed-client` and `wellfed-model`, the stock protocol in `wellfed-stock`, and signing in `wellfed-signing`. The bench's old catch-all server is deleted ([tools](../../src/tools/README.md)).

## Frontends

- **The Android app** owns all IO through mature plugins: BLE (`flutter_blue_plus`), mDNS/NSD, sockets, and joining a Wi-Fi network for the installer. All protocol logic goes through `wellfed-client` over `flutter_rust_bridge`, which also generates the Dart types.
  - With a server, it talks to the server over HTTP first, and to the feeder directly only when the server is down. Without one, it talks to feeders on the LAN.
  - Setup, pairing and the installer always go to the feeder.
  - It can run the stock installer: it joins stock's setup access point, sends the `DL` frame, and serves the image in the foreground for about 3 minutes ([consumer installation](consumer-installation.md)).
- **The server web UI** is a SvelteKit single-page app embedded in `wellfed-server`. It shares the bench's tokens and components, and its types are generated from the server's `openapi.json` ([web UI design](../web/README.md)). Browsers reach the feeder only through the server.
- **Two UIs** means two codebases. That cost is accepted for a native-feeling mobile app; Rust-generated types keep their data models identical.
- **Spike first.** Before building screens, a throwaway Android app proves the risky parts against a host-simulated feeder: BLE scanning and writes, NSD discovery on a real network, and a Noise session through `wellfed-client`.

## Setup and pairing

**Bluetooth runs only in setup mode.** A normal boot never starts the BLE stack. Holding Lock, then Mic, for 5 s on an unlocked panel starts it on the running feeder ([Bluetooth](bluetooth.md)). When setup finishes, or after 15 minutes without a session, the feeder reboots without BLE.

- **Not automatic.** Losing Wi-Fi does not start setup mode. A moved feeder needs the button hold.
- **Not at boot.** Every KM0 backup register the SDK leaves free is taken: the marker, the silent-reset breadcrumb, two boot counts and the heartbeat. So there is nowhere to carry "set up on the next boot" across a reset. Starting the stack live, and rebooting to end it, gives the same costs.
- **Never torn down live.** AmebaD's BT deinit is unverified on the bench and could leak heap.
- **Consumer first boot.** A consumer image with no stored and no build credentials should start in setup mode. That waits for build credentials to become optional; every development image has them.
- **Cost.** A running feeder pays only BLE's flash cost: the linked stack and an 11.5 KB controller patch. Heap, the C tasks (outside our stack audit) and shared radio time are spent only in setup mode.

**The first client.** In one BLE session (Noise XX) the app sends its own client key and learns the feeder's identity key. The app shows a one-time code of button presses, which the owner keys in on the feeder. Both sides prove the code, bound to the handshake, so a relay in radio range cannot pose as either ([why a code](bluetooth.md#why-a-code)). Then the app sends the Wi-Fi credentials, and its key becomes an owner key.

**More clients.**
- **Invite:** a paired client creates a short-lived invite (QR or link), which the new client redeems over the LAN. This is how the server and the CLI join.
- **Button-hold window:** for a short time the feeder accepts one new client over the LAN. This is also recovery when every paired client is lost.

**Keys.** Each client has its own key, which can be revoked without affecting the others. The panel's reset clears them all. An owner can enroll an update-signing key through a paired client, with a button press to confirm.

**Randomness.** Noise needs unpredictable ephemeral keys. The SDK offers no vetted random generator, so the firmware builds its own from measured ADC noise, with health tests ([pairing](pairing.md#no-hardware-rng)).

**Bench shortcuts that go away:** broker credentials as a build input, captured stock credentials, and `sudo nft` redirects. The installer ends by handing the feeder to this pairing flow, not to a server.

## Device API

The [device API](device-api.md) is built. Its sessions are Noise IK, and no MQTT request path remains.

- **Transport.** A Noise session over TCP, advertised over mDNS. Each client authenticates with its own key. There is no plaintext mode.
- **One message stream per client,** carrying typed requests and responses, pushed events behind a cursor, and live samples for subscribers. The app, the server and the CLI share one Rust client library.
- **Writes** carry the generation they were based on. A stale write gets a refusal naming the current generation. The settings store already counts generations.
- **One loop task.** A firmware gets four tasks, the button service's included ([task slots](../../src/firmware/wellfed/README.md#what-the-sdk-runtime-guarantees)), and each needs its own stack. So the API runs in one task, as a single loop:
  - it polls the listener and every client socket without blocking;
  - each pass does a bounded amount of work per client, then steps the feeding owner, so a flood or a malformed client costs only its turn and cannot stall feeding or the watchdog;
  - a Noise handshake operation is one step, and its time on the KM4 bounds the loop's latency ([resource budget](resource-budget.md#handshakes-and-loop-latency));
  - the client limit and per-client buffers are fixed at build time, and a client past the limit is refused.

**One feeding owner, in the loop.** The button task only queues gestures ([buttons](buttons.md)), and the loop reads them. So a press and two clients reach the same owner, in the same task, and cannot double-feed.

The owner stays in the loop rather than in its own higher-priority task for a measured reason. A Wi-Fi association holds every task, a priority-4 one included, for 0.45–0.72 s ([resource budget](resource-budget.md#scheduling-gaps-across-wi-fi-rejoins)). The stall is in the vendor driver, below the scheduler, so no priority keeps a feed's 1 s beat. Instead, feeds and joins take turns (`wellfed_hal::interlock`):

- no feed starts while a join attempt runs or waits (refused `Busy`);
- a join waits in the Wi-Fi task until the owner is at rest with its outputs stopped, bounded by the longest feed.

Storage writes take the same turn. No flash lease starts while a feed may move the outputs, since a lease checks them and resets the board when they move. A write that comes during a feed waits for the release, and a client's write is refused `Busy` meanwhile ([device API](device-api.md#in-the-firmware)).

The event cursor is described under [data](#data).

## Time

- **Sources:** NTP when reachable (the gateway by default, or an address the owner sets), plus syncs from the app and the server over the API. An owner's sync may step the clock. NTP and every other client may only narrow a valid clock within its error bound ([clock](clock.md)).
- **Time zone:** a feeder setting, the zone's POSIX rule, which the feeder evaluates for any year.
- **Validity is an error bound.** The SDK learns its drift from successive syncs and tracks how far off the clock could be. The clock stops counting as valid when that could exceed 2 minutes: about 6.6 hours after a fix before the drift is learned, about 22 hours after. The measured uncorrected drift is 735 to 2,060 ppm slow ([clock](clock.md)). The learned bound stays wide until the bench shows how much it moves; NTP keeps the clock valid.
- **Warm resets** (watchdog, update activation, trial boots) carry the clock through the always-on RTC. A power cut loses it.
- **No source.** After a cold boot with no source, or once the bound is exceeded, scheduled meals hold. The feeder shows that it needs the time, through its lights and sound policy, and the Dispense button and app feeds still work. The missed-meal rule decides what happens once time returns.

## Updates

- The app or server fetches signed releases and shows them. The owner approves each install, which reaches the feeder as an offer over the API.
- The feeder never downloads from the internet, so it needs no TLS client.
- The trial and confirmation of a new build are unchanged. Updates ride the device API ([OTA design](ota-design.md#updates-over-the-device-api)): bank 1 gives way, and bank 0 (the feeder build, which also feeds) installs into bank 1.
- Images are signed by the project key or the owner's enrolled key. A downgrade needs a statement bound to the feeder.

## Data

What the feeder can measure decides what anyone can store. On the PLAF103:

| Signal | Source | Status |
| --- | --- | --- |
| Feed requested, source, result | Feeding owner, journal | Works |
| Delivered portions | Rotor marks on PA18; the outlet beam for food passing | Partial: one mark per portion is observed; counting food needs calibration |
| Food level (ok, low, empty) | Food beam | Partial: polarity known; levels need the food-level feature |
| Motor current per feed | ADC channel 4 | Partial: read during drives; scaling to milliamps needs a reference |
| Supply voltage, adapter or battery | ADC channel 5, PA19 | Works |
| Buttons, lock, resets | PB4–PB7 | Works |
| Connectivity, boots, updates, clock syncs | Runtime | Works: reported in health and statistics |

The feeder has no scale, bowl sensor or camera. It knows what it dispensed, never what the pet ate.

The goal is that anyone can do anything with their feeder's data. The server keeps everything raw, and features like battery health are queries over it, not firmware changes. The feeder sends:

- **Durable events.** Stored on the feeder in a ring that holds at least 7 days, each with a sequence number: feeds, faults, food-level changes, button presses and lock changes, setting changes with the client that made them, boots and reset reasons, updates and clock syncs. A feed is a summary: requested and delivered portions, duration, peak and mean motor current, and the result. A client that reconnects reads everything after its cursor. Records already lost to the ring are reported as lost, not skipped.
- **Live samples.** Everything the runtime samples (supply and battery voltage, adapter state, Wi-Fi RSSI, clock offset, food-beam state), at its own rate. They go only to subscribers and are never stored on the feeder, so a sample missed during a server outage is gone.
- **Per-feed traces.** Each feed's current, marks, beams, battery and motor commands over the drive. The feeder keeps them for its last 8 to 12 feeds, read on demand ([feed traces](device-api.md#feed-traces)). The server is optional, so the feeder keeps its own history.

**On the server:**

- **Raw store.** Every event and sample is kept forever by default. The owner can prune, and can export everything as CSV or JSON, and import it to move servers.
- **Open schema.** Event and sample types live in `wellfed-proto`, versioned and documented. The feeder links that crate; `wellfed-formats` holds host-side files only.
- **Owner data the feeder doesn't hold:** pet profiles (name, photo, weight log, daily target) and food (brand, kcal per gram, bag size).
- **Derived views:**
  - food remaining and a refill estimate: calibrated grams per portion × portions since the last refill, against the bag size;
  - grams and calories against each pet's target;
  - feeding anomalies: missed, uncertain or short feeds;
  - hardware health: motor current trends, battery curves, Wi-Fi drops, resets and clock drift;
  - an audit log of who did what.

## Home Assistant

Paused by the owner. The options when it resumes:

- **ESPHome native API on the feeder.** Auto-discovered, with no add-on and no server. But it reimplements an unofficial protocol that changes with Home Assistant releases, and it spends firmware RAM. Prove it first with a host stub that a real Home Assistant instance adopts.
- **MQTT discovery through `wellfed-server`.** A documented, stable protocol with no Home Assistant code in the firmware. But it needs the server and a broker.

The entity model in `wellfed-feeder` should not assume either one.

## Deferred

- **Notifications.** Later, through the Android app: the server decides what is an alert, and the app delivers it. Push on iOS and Android goes through Apple and Google services, which need credentials tied to the app's publisher, so a self-hosted server can't send them alone. The options are ntfy or UnifiedPush, a small project relay, or both.
- **Remote access.** Only through the owner's server; the owner's VPN is one route.

## What this asks of the SDK

| Work | Adds | Read |
| --- | --- | --- |
| Resource budget | RAM, flash, stack and task budget for the product image, setup-mode BLE and Noise included: the two firmware tasks, the handshake time in the loop, the client limit and buffers | [Resource budget](resource-budget.md) |
| Client keys and pairing | The random generator, per-client keys, BLE setup mode, invites, the button-hold window, revocation, owner signing-key enrollment | [Pairing](pairing.md) |
| Offline time | NTP, drift learning, the error-bound validity, time across warm resets, the no-source hold | [Clock](clock.md) |
| Device-hosted API | The Noise message stream, mDNS, generation-checked writes, the event cursor and samples, several clients served by one loop task | [Device API](device-api.md) |
| Updates over the device API | Offers from any paired client, the project key and an owner key | [OTA design](ota-design.md) |
| ESPHome native API | Paused with Home Assistant | [Above](#home-assistant) |
| Device layer for other models | An audit of the HAL and `device.json` for PLAF103 assumptions | [Device layer](device-layer.md) |

## Evidence

- **Design:** the decisions above are the owner's, recorded on October 23, 2026, with the app-and-server path and notifications decided on October 26, and the "not automatic" setup rule on October 24.
- **Observed:** a Wi-Fi association held every task, a priority-4 one included, for 0.45–0.72 s on October 25 ([resource budget](resource-budget.md#scheduling-gaps-across-wi-fi-rejoins)).
- **Observed:** PA18 rests HIGH and reads LOW for about 1.17 s at each rotor mark, one mark per portion. During a 3 s empty drive, channel 4 read a scaled maximum of 166 and a mean of 64 ([motor pulse](motor-pulse.md)).
- **Observed:** the task-slot limit was found in another session's captures (`fw44-simon-01`).
