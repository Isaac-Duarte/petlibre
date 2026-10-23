# Product architecture

The owner's target (Oct 23, 2026): the Petlibro experience, fully open and usable offline. This page records the decisions behind it and the SDK work it creates. The cards live on [FIRMWARE_BOARD.md](../../FIRMWARE_BOARD.md) (FW44–FW50) and [FEATURE_BOARD.md](../../FEATURE_BOARD.md) (FP16, FP18). Everything here is design; nothing is physical evidence yet.

## Decisions

- **One product firmware.** The consumer image is a single `feeder` firmware that runs the same whether or not a server exists. "Offline" removes a dependency; it is not a second build. Every feeding guarantee, soak and OTA path is proven once. The bring-up firmwares (`qa`, `buttons`, `motor` and the rest) stay development tools.
- **Offline means no server and no cloud, but Wi-Fi.** The feeder may use public NTP. It never depends on the vendor's services.
- **The feeder owns its state:** plans, settings, client keys and a bounded history. Schedules run on the feeder alone once the clock is valid.
- **The feeder hosts the API.** It serves one authenticated LAN API, advertised over mDNS. The Tauri app, the self-hosted server and Home Assistant are all clients of it. The feeder stops being an MQTT client of `wellfed-local`; MQTT stays only where stock needs it during installation ([consumer installation](consumer-installation.md)).
- **Native Home Assistant** means auto-discovery with no add-on, through the ESPHome native API.
- **The server is optional.** It adds what a LAN-only feeder cannot: long history, remote access, push notifications and fetching releases. Remote access exists only through the owner's server (or the owner's own VPN).
- **Users:** both non-technical owners (the app alone) and self-hosters (server, Home Assistant).
- **Hardware:** the PLAF103 first; keep the SDK's device layer generic enough for other AmebaD Petlibro models.

## Components

```text
                 BLE (setup and pairing only)
   Tauri app  ─────────────────────────────┐
      │                                    ▼
      │  LAN API (signed, mDNS)        ┌────────┐   NTP (internet)
      ├───────────────────────────────►│ feeder │◄──────────────
      │                                └────────┘
   server (optional) ── LAN API ───────►  ▲  ▲
      │  history, push, remote access     │  │ ESPHome native API (Noise)
      │  release fetching                 │  │
   Home Assistant ────────────────────────┘  │
                                             │ update offers (signed images)
```

## Trust and pairing

- **Per-client keys.** Each client (a phone, the server, Home Assistant) holds its own key, which the feeder stores and can revoke on its own. Requests stay HMAC-signed with a nonce and sequence, as in the [local transport](local-transport.md#authenticated-requests). Requests and history record which client made each change.
- **Enrollment over BLE during setup.** The app sends Wi-Fi credentials and enrolls its key in one BLE session, as the `ble` firmware already does for Wi-Fi. A physical button press confirms. More clients are added later from an already enrolled client.
- **Home Assistant's key** is the 32-byte Noise key HA asks for when it adds an ESPHome device. The app shows it.
- **Release signing.** Official releases are signed by the project key. An owner can enroll their own signing key through the same paired, button-confirmed path and run self-built images. The bank-0 recovery role and the UART path stay unchanged.
- **Removed bench shortcuts:** broker credentials as a build input, captured stock credentials, and `sudo nft` redirects.

## Time

- **Sources, all equal:** NTP when the LAN has internet, the app, the server and Home Assistant (the ESPHome API's time request). Every source except NTP is signed.
- **Holdover.** A valid clock stays valid across a week without a sync. The SDK learns its own drift from successive syncs. The measured uncorrected rate is about 735 ppm slow ([clock](clock.md)), about 7 minutes a week, so correction is required to keep meals on time.
- **Warm resets.** A watchdog reset, an OTA activation or a trial boot should keep time if a register or RTC in the always-on domain survives the reset. This is unverified (FW46).
- **Cold boot.** Removing the adapter and batteries loses time. Schedules wait for any one source, and FW21's missed-meal rule covers the gap. With NTP reachable that is seconds; with no source at all the feeder holds schedules and shows that it needs time.

## Device API

- **One envelope for every client:** the existing signed request format, carried over a TCP listener (HTTP or WebSocket, settled by FW44's budget) instead of the broker.
- **Discovery:** mDNS for the API, and `_esphomelib._tcp` for Home Assistant.
- **Two streams:** durable events behind a cursor, and live samples to subscribers ([data](#data)).
- **Event cursor.** Every history record and alert carries a monotonic sequence number. A client that reconnects reads everything after its last cursor, so the server mirrors the feeder with no gaps across its own outages. Records older than the ring's oldest entry are reported as lost, not silently skipped.
- **Concurrency.** Several clients at once, bounded by FW44's budget. Every feed still goes through the FW16/FW17 feeding owner, so two clients, an HA automation and a button press cannot double-feed.

## Home Assistant

- **ESPHome native API** (protobuf over TCP, Noise encryption), with a pinned `api.proto` version. Upgrading HA can change it, so compatibility is tested against a real HA instance.
- **Entities:** a feed button with a portions number, food level, faults, battery and power, child lock, lights, sound and volume, and an event entity per feed result. Each plan appears as an enable switch and a time entity. Editing whole plans stays in the app.
- **One feeding owner.** HA automations call the same feed request as everything else, and feeding never depends on HA.

## Updates

- **The app or server fetches** signed releases (for example from the repository's releases) and offers them to the feeder over the LAN. The feeder never downloads from the internet itself, so it needs no TLS client.
- **Same updater:** the FW10 download, the FW11 trial and confirmation, and the bank-0 recovery role, with offers arriving over the device API instead of the broker.

## Data

What the feeder can measure decides what anyone can store. On the PLAF103 (static and bench findings):

| Signal | Source | Status |
| --- | --- | --- |
| Feed requested, source, result | Feeding owner, journal | Host-tested; needs FW16/FW17 |
| Delivered portions | Outlet beam pulses | Unverified: no real pulse seen yet (FW13 carried to FW16) |
| Food level (ok, low, empty) | Food beam | Polarity known (FW13); levels need FP07 |
| Motor current per feed | ADC channel 4 | Uncalibrated until motion (FW14 carried to FW16) |
| Supply voltage, adapter or battery | ADC channel 5, PA19 | Physical (FW14) |
| Buttons, lock, resets | PB4–PB7 | Mapped (FW20) |
| Connectivity, boots, updates, clock syncs | Runtime | Reported in health today |

The feeder has no scale, bowl sensor or camera, so it knows what it dispensed, never what the pet ate.

The goal is that anyone can do anything with their feeder's data: the server keeps everything raw, and features like battery health are queries over it, not firmware changes. The feeder sends two streams:

- **Durable events.** These are stored on the feeder in a bounded ring with sequence numbers: feeds, faults, food-level changes, button presses and lock changes, setting changes with the client that made them, boots and reset reasons, updates and clock syncs. A feed is a summary: requested and delivered portions, duration, peak and mean motor current, and the result. FW44 sizes the ring to hold weeks of meals, not years. The cursor means the server never misses an event, as long as it reconnects before the ring wraps.
- **Live samples.** Everything the runtime samples (supply and battery voltage, adapter state, Wi-Fi RSSI, clock offset, food-beam state), at the rate it samples it. They are sent only while a client subscribes and are never stored on the feeder, so they cost no flash. A sample missed during a server outage is gone; the durable events are not. Raw per-feed traces (current and beam during a feed) are not part of the product stream. A bring-up firmware can capture them, and the live stream can carry them later if FW44 leaves room.

**On the server ([FP18](../../FEATURE_BOARD.md)):**

- **Raw store.** Every event and sample is kept forever by default. The owner can prune, and can export everything as CSV or JSON (and import it to move servers).
- **Open schema.** Event and sample types live in `wellfed-formats` and are versioned and documented, so third-party tools can read the stream or the export.
- **Owner data the feeder doesn't hold:** pet profiles (name, photo, weight log, daily target) and food (brand, kcal per gram, bag size).
- **Derived views:**
  - Food remaining and a refill estimate: calibrated grams per portion (FW18) × portions since the last refill (food beam low → ok), against the bag size.
  - Grams and calories dispensed against each pet's target.
  - Feeding anomalies: missed, uncertain or short feeds, and departures from the usual days.
  - Hardware health: motor current per feed as a jam predictor, battery curves, Wi-Fi drops, resets and clock drift.
  - An audit log of who did what.

## SDK work

| Card | Adds |
| --- | --- |
| FW44 Resource budget | RAM, flash, stack and task budget for the full product image, before committing to the protocols below |
| FW45 Client keys and BLE pairing | Per-client keys in the settings store, BLE enrollment with a button confirmation, revocation, owner signing-key enrollment |
| FW46 Offline time | NTP, drift learning, a week of holdover, time across warm resets, the extra sources |
| FW47 Device-hosted API | The listener, mDNS, the signed envelope off MQTT, the event cursor, several clients |
| FW48 Updates over the device API | Offers from any enrolled client, the project key and an owner key |
| FW49 ESPHome native API | Noise, discovery, the entity mapping, HA time |
| FW50 Device layer for other models | Audit the HAL and `device.json` for PLAF103 assumptions |
