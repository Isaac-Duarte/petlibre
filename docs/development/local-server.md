# Stock-firmware local server

The `petlibro-local` executable provides an embedded authenticated broker, stock protocol handling, durable feed-request tracking, and a bearer-authenticated loopback API. It is separate from replacement firmware. Stock 1.1.15 local dispensing was demonstrated on the research unit; unattended reliability and measured portions remain unvalidated.

## Build and try synthetic initialization

```sh
cargo build --locked
./target/debug/petlibro-local init \
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
./target/debug/petlibro-local serve --config private/server.json
./target/debug/petlibro-local status --config private/server.json
./target/debug/petlibro-local network preview --config private/server.json
```

Starting the service does not redirect traffic or command feeding. Linux gateway `network apply` is a separate privileged operation: it validates an nft transaction, redirects the configured feeder MAC's TCP/1883 traffic on the configured interface, and blocks its forwarded WAN traffic. Inspect preview and confirm device identity before explicitly invoking it. `network remove` removes Petlibre's dedicated table/tagged rule. AP setup, host-wide firewall policy, persistent services, and reboot persistence are outside this workflow. Do not assume WAN isolation without checking counters and actual traffic.

## Manual requests and uncertainty

The `feed` command requires a caller-generated UUID and portion count. `new-request-id` prints a UUID without contacting a device. Reuse the same request ID after a lost response; never create a new ID merely to retry. Offline/stale devices are refused, commands are not retained, and the controller does not replay a persisted request.

When telemetry cannot establish the outcome, inspect the physical result before `resolve --request-id UUID --note "observed outcome"`. Resolution changes the ledger without sending a command. Device-reported completion does not establish measured food weight. Configuration, feed ledger, and exclusive lock are stored together.

## HTTP API

Write bodies are bounded to 4096 bytes. Use bearer authentication from the private configuration; the CLI handles it without placing the token in shell arguments.

| Route | Interface |
| --- | --- |
| `GET /health` | Public process/build identity |
| `GET /api/status` | Authenticated telemetry, build identity, request history, trial/probe observations |
| `POST /api/feed` | `{ "request_id": "UUID", "portions": 1 }` |
| `POST /api/feed/resolve` | `{ "request_id": "UUID", "note": "physical observation" }` |
| `POST /api/ota/trial` | Experimental fixed-image install/restore; explicit trial mode and image lease required |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation; sends no command |
| `POST /api/probe/query` | Explicit diagnostic request for a confirmed pinned probe installation |

Normal `serve` mode keeps OTA trial/probe dispatch disabled. `ota-images` is a separate fixed-image HTTP transfer service; it does not command an update. Production trial dispatch requires a clean identifiable source commit. Test-only bypasses are not a deployment procedure. Retained OTA/probe helpers accept only their reviewed inputs; see [fixtures](fixtures.md) and [recovery limits](../hardware/recovery.md).

The server answers schedule queries from the imported snapshot and does not edit schedules. Nonempty-plan operation and stock time-recovery behavior remain unvalidated; see [stock behavior](../firmware/stock-behavior.md). Restore prior routing before shutting down the service if the feeder depends on that route.
