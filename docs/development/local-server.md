# Local server

The `wellfed-local` executable is the bench's local server: an embedded authenticated broker, the stock 1.1.15 protocol as far as installing the replacement needs it, the replacement firmware's MQTT transport, and a bearer-authenticated loopback API. It is being split up and retired ([tools roadmap](tools-roadmap.md)).

Since October 25, 2026 it is a binary over the [`wellfed-stock`](../../src/tools/wellfed-stock) library, which holds everything stock 1.1.15 speaks: its MQTT protocol, the broker with stock's two identities (the stock device and the server) and the CONNECT normalization in front of it, the server's MQTT session with stock, the setup frame, the image server stock installs from, and the install-trial ledger. `wellfed-local` itself keeps the command line, the loopback API, the `network` redirect, and the replacement firmware's side of the broker and the session (its role, health and offers), which plug into the library's broker roles and session companion and are deleted with the transport.

Its stock control (manual feeds, the feed ledger, `resolve`) and the C-probe and modified-stock trials were removed on October 24, 2026 (FW51): the product uses stock only to install from and roll back to. Local dispensing on stock 1.1.15 was demonstrated on the research unit before that, and the code is in Git history.

## Build and try synthetic initialization

```sh
cargo build --locked
./target/debug/wellfed-local init \
  --credentials src/tools/wellfed-local/examples/mqtt-credentials.json \
  --capture src/tools/wellfed-local/examples/mqtt-capture.json \
  --mqtt-listen 127.0.0.1:18883 \
  --hotspot-interface test_wifi \
  --output private/example-server.json
```

The [example inputs](../../src/tools/wellfed-local/examples/README.md) contain only synthetic identities. This command writes configuration and sends no device commands. A second import to the same path is refused. `--timezone` accepts an IANA timezone and defaults to `UTC`. The generated API token and server password are independent; configuration is mode 0600 on Unix.

## Configure an actual device

Use your own isolated gateway/AP and capture only the device you intend to control. Initial stock pairing normally uses the vendor app; cloud-free provisioning has not been established. A laptop merely sharing Wi-Fi may not see the feeder's unicast traffic.

Store the device's single MQTT CONNECT credential record as a JSON array in the shape illustrated by the credential example. Export decoded MQTT packets using Wireshark/TShark JSON, with the `_source.layers.mqtt` and Ethernet source fields illustrated by the capture example. The importer requires one device namespace, its source MAC, and an authoritative captured feeding-plan response (`GET_FEEDING_PLAN_EVENT` or `FEEDING_PLAN_SERVICE` with `plans`). It optionally imports the binding response. Do not invent an empty snapshot for an existing device with schedules.

A typical decoded packet export is `tshark -r private/capture.pcap -Y mqtt -T json > private/mqtt.json`. Inspect the export locally against the documented shape: captures with a different dissector nesting require normalization. Credentials and raw exports must stay private.

Run `init` with your own credential/capture paths, a specific IPv4 broker address reachable from the feeder, your gateway interface, and your timezone. Wildcard and IPv6 broker binds are rejected. `private/server.json` is the default output. The HTTP default is loopback `127.0.0.1:8765`. The configuration format remains version 1; a configuration from before FW51 still carries `max_portions`, which is refused as an unknown field, so delete that line. Edit configuration while the service is stopped and retain the ledger beside it.

```sh
./target/debug/wellfed-local serve --config private/server.json
./target/debug/wellfed-local status --config private/server.json
./target/debug/wellfed-local network preview --config private/server.json
```

Starting the service does not redirect traffic or command feeding. Linux gateway `network apply` is a separate privileged operation: it validates an nft transaction, redirects the configured feeder MAC's TCP/1883 traffic on the configured interface, and blocks its forwarded WAN traffic. Inspect preview and confirm device identity before explicitly invoking it. `network remove` removes Wellfed's dedicated table/tagged rule. AP setup, host-wide firewall policy, persistent services, and reboot persistence are outside this workflow. Do not assume WAN isolation without checking counters and actual traffic.

## Replacement-firmware role (FW08)

The broker can also serve the replacement firmware, as a separate role with its own credential and namespace; see the [transport design](../firmware/local-transport.md). With the service stopped:

```sh
./target/debug/wellfed-local firmware-credential --config private/server.json --output private/fw-mqtt.json
```

This adds an optional `firmware` section to the configuration and writes the firmware build's private credential file (mode 0600). The section holds the client ID `wellfed-fw-<device_id>`, the username `wellfed-fw` and a 64-hex-character password. The credential file holds `version`, `broker`, `port`, `device_id`, `client_id`, `username`, `password` and `namespace`. A second run is refused whether or not the output exists, so the credential never rotates silently. The file is written before the configuration: if the configuration update fails, delete the file and rerun. A configuration without the section behaves as before, and the configuration format stays version 1.

| Topic under `wellfed/1/<device_id>/` | Firmware role | Server role |
| --- | --- | --- |
| `health` | publish | subscribe |
| `reply` | publish | subscribe |
| `request` | subscribe | publish |

Every allowed topic is literal: wildcards in this namespace are refused. The firmware role cannot reach the stock `dl/PLAF103/...` topics, and the stock device role cannot reach this namespace. The broker has retained messages disabled, so the firmware publishes health without retain, and its will (`"online": false`) is not retained either. After a server restart, health appears with the next periodic publish.

The server accepts health of schema `wellfed.health` version 1 with a boolean `online` and, if present, a `nonce` of 16 lowercase hex characters, up to 704 bytes, and replies up to 256 bytes with a UUID `id`. It counts and ignores anything else. Health older than 75 s is stale. A request is sent with QoS 0 only while health is fresh and online; otherwise the API refuses it instead of losing it unseen.

The firmware channel is plaintext MQTT on the isolated hotspot. Anyone with the Wi-Fi password could read it or impersonate either end, which is accepted only because the channel carries read-only health. Feeding or update commands must not use it until it is authenticated or encrypted.

### Request signing (retired)

The firmware takes no requests over MQTT since FW47; they go over its [device API](../firmware/device-api.md). `firmware-request-key` is deleted. A `request_key` already in the configuration or the credential file still parses and is unused, and goes with the broker credentials in FW48.

With a key configured, the server signs every request it sends to the firmware: API `health` and `ping` requests and the FW21 `time` sync. It signs under the `nonce` of the latest health, with a `seq` that starts at 1 for each new nonce and rises by one per request; it refuses (503) rather than wrap once `seq` would pass 4294967295. Firmware whose health has no `nonce` (built without a key) gets unsigned requests, as before. Without a configured key the server never signs. A refused request comes back as a reply with `"error":"auth"`, which the status shows like any other reply. The key is validated on load and never appears in the API, status or logs; the nonce is not secret and shows in the health.

## Replacement-firmware updates (FW10)

The replacement firmware accepts only updates signed by the key its build embeds ([design](../firmware/ota-design.md)). Signing is `wellfed-build keygen` and `wellfed-build sign`, offline, and `sign` takes only a frozen review ([firmware builds](firmware-builds.md#update-signing)).

Offer a signed update to the running server, or clear it:

```sh
./target/debug/wellfed-local ota-offer --manifest private/<name>.manifest --image private/verification/<review>/image.bin
./target/debug/wellfed-local ota-offer --clear
```

The offer lives only in the server's memory. Each offer is served to completion once. After its last chunk has gone out, a new manifest request means the candidate fell back to bank 0, so the server drops the offer and answers that it has none. Offer it again to retry. Re-asks of chunks, such as a lost last answer, are still served. The server answers the firmware's `ota/get` requests on `ota/data` with the manifest or a chunk of at most 512 bytes, and never pushes anything. On load it checks that the image matches the manifest's length and hash; authenticity is the firmware's signature check. Paths must be relative and under `private/`. The broker lets the firmware role publish only `ota/get` and subscribe only `ota/data`, and the server role the reverse. A confirmed normal image checks for an offer as `<client_id>-ota`, with the same credentials. That identity may only publish `ota/check` and subscribe `ota/offered`. The server answers with the manifest or no offer, and a check never counts as a delivery.

## The install ledger

Install trials are recorded in `stock-ledger.json` beside the configuration, under an exclusive lock. Each trial is persisted before its one `OTA_UPGRADE` goes out and is never resent: a repeated request ID returns the stored record, and a trial in flight when the server stops or loses the broker becomes uncertain. An uncertain or failed trial blocks the next until `ota-trial-resolve` closes it from fresh evidence, which sends no command.

The ledger before FW51, `feed-ledger.json`, also held feeds and probe queries. `serve` refuses to start while it is present: check that no trial in it is submitted, uncertain or failed, then rename it.

## HTTP API

Write bodies are bounded to 4096 bytes. Use bearer authentication from the private configuration; the CLI handles it without placing the token in shell arguments.

| Route | Interface |
| --- | --- |
| `GET /health` | Public process/build identity |
| `GET /api/status` | Authenticated telemetry, build identity, install trials and OTA reports, and `replacement` (the latest firmware health, which since FW47 only bank 0's recovery role sends: `age_ms`, `online`, `stale` and the rejected count; null without a firmware role) |
| `POST /api/ota/trial` | `{ "request_id": "UUID", "server_token": "UUID" }`: the stock-to-replacement install; explicit trial mode and a live image lease required |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation; sends no command |

Normal `serve` mode keeps trial dispatch disabled. `ota-images` is a separate HTTP transfer service for one reviewed build; it does not command an update. Trial dispatch requires a clean identifiable source commit. Test-only bypasses are not a deployment procedure; see [fixtures](fixtures.md) and [recovery limits](../devices/plaf103/recovery.md).

**Replacement install (stock-to-replacement).** `ota-images --directory private/verification/<review>` serves that frozen review's `image.bin` as `/ota/<token>/replacement.bin`. It refuses an image that is not a build's, is larger than the stock footprint (1,376,288 bytes), lacks the loader marker, or does not match the review's `ota_sha256`. The review's layout audit is what establishes the container header and byte-sum stock checks. There is no `original.bin` beside it: the way back is the UART stock recovery. `--peer` defaults to the feeder found by its MAC on the hotspot interface, and the lease also records the served digest. A trial needs that lease, and announces target version `2.0.0` with the lease's digest. Stock never reports after installing it, so the trial closes as `replacement_healthy` when firmware-role health first arrives after the submission. `wellfed-bench stock-ota` drives the whole sequence ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)).

The server answers stock's schedule queries from the imported snapshot, and a trial needs that snapshot empty. Nonempty-plan operation and stock time-recovery behavior remain unvalidated; see [stock behavior](../devices/plaf103/stock/README.md). Restore prior routing before shutting down the service if the feeder depends on that route.
