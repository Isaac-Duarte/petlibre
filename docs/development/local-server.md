# Stock-firmware local server

The `petlibre-local` executable provides an embedded authenticated broker, stock protocol handling, durable feed-request tracking, and a bearer-authenticated loopback API. It is separate from replacement firmware. Stock 1.1.15 local dispensing was demonstrated on the research unit; unattended reliability and measured portions remain unvalidated.

## Build and try synthetic initialization

```sh
cargo build --locked
./target/debug/petlibre-local init \
  --credentials examples/mqtt-credentials.json \
  --capture examples/mqtt-capture.json \
  --mqtt-listen 127.0.0.1:18883 \
  --hotspot-interface test_wifi \
  --output private/example-server.json
```

The [example inputs](../../examples/README.md) contain only synthetic identities. This command writes configuration and sends no device commands. A second import to the same path is refused. `--timezone` accepts an IANA timezone and defaults to `UTC`. The generated API token and server password are independent; configuration is mode 0600 on Unix.

## Configure an actual device

Use your own isolated gateway/AP and capture only the device you intend to control. Initial stock pairing normally uses the vendor app; cloud-free provisioning has not been established. A laptop merely sharing Wi-Fi may not see the feeder's unicast traffic.

Store the device's single MQTT CONNECT credential record as a JSON array in the shape illustrated by the credential example. Export decoded MQTT packets using Wireshark/TShark JSON, with the `_source.layers.mqtt` and Ethernet source fields illustrated by the capture example. The importer requires one device namespace, its source MAC, and an authoritative captured feeding-plan response (`GET_FEEDING_PLAN_EVENT` or `FEEDING_PLAN_SERVICE` with `plans`). It optionally imports the binding response. Do not invent an empty snapshot for an existing device with schedules.

A typical decoded packet export is `tshark -r private/capture.pcap -Y mqtt -T json > private/mqtt.json`. Inspect the export locally against the documented shape: captures with a different dissector nesting require normalization. Credentials and raw exports must stay private.

Run `init` with your own credential/capture paths, a specific IPv4 broker address reachable from the feeder, your gateway interface, and your timezone. Wildcard and IPv6 broker binds are rejected. `private/server.json` is the default output. The HTTP default is loopback `127.0.0.1:8765`; `max_portions` starts at 1. The configuration format remains version 1. Edit configuration while the service is stopped and retain the ledger beside it.

```sh
./target/debug/petlibre-local serve --config private/server.json
./target/debug/petlibre-local status --config private/server.json
./target/debug/petlibre-local network preview --config private/server.json
```

Starting the service does not redirect traffic or command feeding. Linux gateway `network apply` is a separate privileged operation: it validates an nft transaction, redirects the configured feeder MAC's TCP/1883 traffic on the configured interface, and blocks its forwarded WAN traffic. Inspect preview and confirm device identity before explicitly invoking it. `network remove` removes Petlibre's dedicated table/tagged rule. AP setup, host-wide firewall policy, persistent services, and reboot persistence are outside this workflow. Do not assume WAN isolation without checking counters and actual traffic.

## Replacement-firmware role (FW08)

The broker can also serve the replacement firmware, as a separate role with its own credential and namespace; see the [transport design](../firmware/local-transport.md). With the service stopped:

```sh
./target/debug/petlibre-local firmware-credential --config private/server.json --output private/fw-mqtt.json
```

This adds an optional `firmware` section to the configuration and writes the firmware build's private credential file (mode 0600). The section holds the client ID `petlibre-fw-<device_id>`, the username `petlibre-fw` and a 64-hex-character password. The credential file holds `version`, `broker`, `port`, `device_id`, `client_id`, `username`, `password` and `namespace`. A second run is refused whether or not the output exists, so the credential never rotates silently. The file is written before the configuration: if the configuration update fails, delete the file and rerun. A configuration without the section behaves as before, and the configuration format stays version 1.

| Topic under `petlibre/1/<device_id>/` | Firmware role | Server role |
| --- | --- | --- |
| `health` | publish | subscribe |
| `reply` | publish | subscribe |
| `request` | subscribe | publish |

Every allowed topic is literal: wildcards in this namespace are refused. The firmware role cannot reach the stock `dl/PLAF103/...` topics, and the stock device role cannot reach this namespace. The broker has retained messages disabled, so the firmware publishes health without retain, and its will (`"online": false`) is not retained either. After a server restart, health appears with the next periodic publish.

The server accepts health of schema `petlibre.health` version 1 with a boolean `online`, up to 512 bytes, and replies up to 256 bytes with a UUID `id`. It counts and ignores anything else. Health older than 75 s is stale. A request is sent with QoS 0 only while health is fresh and online; otherwise the API refuses it instead of losing it unseen.

The firmware channel is plaintext MQTT on the isolated hotspot. Anyone with the Wi-Fi password could read it or impersonate either end, which is accepted only because the channel carries read-only health. Feeding or update commands must not use it until it is authenticated or encrypted.

## Replacement-firmware update signing (FW10)

The replacement firmware will accept only updates signed by an owner-held key ([design](../firmware/ota-design.md)). These commands run offline and never contact a device:

```sh
./target/debug/petlibre-local ota-keygen --output private/ota-key.json
./target/debug/petlibre-local ota-sign --key private/ota-key.json \
  --image private/verification/<review>/image.bin --version 2 \
  --build-id <startup identity, 64 hex> --output private/<name>.manifest
```

- **`ota-keygen`:** writes the Ed25519 seed (mode 600) and prints the public key that firmware builds embed. It refuses to replace an existing key.
- **`ota-sign`:** checks that the image's bank contents start with the loader marker and fit the reviewed bank-1 bound, then writes the manifest in the exact format `petlibre-core::ota` verifies.

Offer a signed update to the running server, or clear it:

```sh
./target/debug/petlibre-local ota-offer --manifest private/<name>.manifest --image private/verification/<review>/image.bin
./target/debug/petlibre-local ota-offer --clear
```

The offer lives only in the server's memory. The server answers the firmware's `ota/get` requests on `ota/data` with the manifest or a chunk of at most 512 bytes, and never pushes anything. On load it checks that the image matches the manifest's length and hash; authenticity is the firmware's signature check. Paths must be relative and under `private/`. The broker lets the firmware role publish only `ota/get` and subscribe only `ota/data`, and the server role the reverse.

## Manual requests and uncertainty

The `feed` command requires a caller-generated UUID and portion count. `new-request-id` prints a UUID without contacting a device. Reuse the same request ID after a lost response; never create a new ID merely to retry. Offline/stale devices are refused, commands are not retained, and the controller does not replay a persisted request.

When telemetry cannot establish the outcome, inspect the physical result before `resolve --request-id UUID --note "observed outcome"`. Resolution changes the ledger without sending a command. Device-reported completion does not establish measured food weight. Configuration, feed ledger, and exclusive lock are stored together.

## HTTP API

Write bodies are bounded to 4096 bytes. Use bearer authentication from the private configuration; the CLI handles it without placing the token in shell arguments.

| Route | Interface |
| --- | --- |
| `GET /health` | Public process/build identity |
| `GET /api/status` | Authenticated telemetry, build identity, request history, trial/probe observations, and `replacement` (latest firmware health, `age_ms`, `online`, `stale`, rejected count and recent replies; null without a firmware role) |
| `POST /api/feed` | `{ "request_id": "UUID", "portions": 1 }` |
| `POST /api/feed/resolve` | `{ "request_id": "UUID", "note": "physical observation" }` |
| `POST /api/ota/trial` | Experimental fixed-image install/restore; explicit trial mode and image lease required |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation; sends no command |
| `POST /api/probe/query` | Explicit diagnostic request for a confirmed pinned probe installation |
| `POST /api/replacement/request` | `{ "op": "health" }` or `{ "op": "ping" }` to the replacement firmware; returns the generated request ID. Other bodies get 400, no firmware role 404, and stale or offline health 503 |

Normal `serve` mode keeps OTA trial/probe dispatch disabled. `ota-images` is a separate fixed-image HTTP transfer service; it does not command an update. Production trial dispatch requires a clean identifiable source commit. Test-only bypasses are not a deployment procedure. Retained OTA/probe helpers accept only their reviewed inputs; see [fixtures](fixtures.md) and [recovery limits](../hardware/recovery.md).

The server answers schedule queries from the imported snapshot and does not edit schedules. Nonempty-plan operation and stock time-recovery behavior remain unvalidated; see [stock behavior](../firmware/stock-behavior.md). Restore prior routing before shutting down the service if the feeder depends on that route.
