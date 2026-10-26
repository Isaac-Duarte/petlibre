# Local server

The `wellfed-local` executable is the bench's local server: an embedded authenticated broker, the stock 1.1.15 protocol as far as installing the replacement needs it, and a bearer-authenticated loopback API. The replacement firmware speaks no MQTT: its updates and status go over its [device API](../firmware/device-api.md). It is being split up and retired ([tools roadmap](tools-roadmap.md)).

Since October 25, 2026 it is a binary over the [`wellfed-stock`](../../src/tools/wellfed-stock) library, which holds everything stock 1.1.15 speaks: its MQTT protocol, the broker with stock's two identities (the stock device and the server) and the CONNECT normalization in front of it, the server's MQTT session with stock, the setup frame, the image server stock installs from, and the install-trial ledger. `wellfed-local` itself keeps the command line, the loopback API and the `network` redirect. The replacement firmware's side of the broker and the session (its role, health and offers) is deleted, and the broker admits only stock's two identities.

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

## The replacement firmware (deleted roles)

The broker once served the replacement firmware too, as a role with its own credential and namespace (`firmware-credential`, health, requests and `ota-offer`; the [transport design](../firmware/local-transport.md) records it). It is deleted, with the firmware's MQTT transport: builds take updates over the [device API](../firmware/ota-design.md#updates-over-the-device-api), which `wellfed-bench ota` pushes, and `wellfed-build` no longer reads `private/fw-mqtt.json`. A configuration written while the role existed keeps its `firmware` section; `wellfed-local` ignores it unread, so the file loads as it is.

## The install ledger

Install trials are recorded in `stock-ledger.json` beside the configuration, under an exclusive lock. Each trial is persisted before its one `OTA_UPGRADE` goes out and is never resent: a repeated request ID returns the stored record, and a trial in flight when the server stops or loses the broker becomes uncertain. An uncertain or failed trial blocks the next until `ota-trial-resolve` closes it from fresh evidence, which sends no command. A replacement install closes instead on `ota-trial-answered`, once the operator's client saw the new build answer its device API (below).

The ledger before FW51, `feed-ledger.json`, also held feeds and probe queries. `serve` refuses to start while it is present: check that no trial in it is submitted, uncertain or failed, then rename it.

## HTTP API

Write bodies are bounded to 4096 bytes. Use bearer authentication from the private configuration; the CLI handles it without placing the token in shell arguments.

| Route | Interface |
| --- | --- |
| `GET /health` | Public process/build identity |
| `GET /api/status` | Authenticated stock telemetry, build identity, install trials and stock's OTA reports |
| `POST /api/ota/trial` | `{ "request_id": "UUID", "server_token": "UUID" }`: the stock-to-replacement install; explicit trial mode and a live image lease required |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation; sends no command |
| `POST /api/ota/trial/answered` | `{ "request_id": "UUID", "note": "…" }`: the latest submitted or uncertain trial's build answered the device API after it; closes it as `replacement_healthy`, sends no command |

Normal `serve` mode keeps trial dispatch disabled. `ota-images` is a separate HTTP transfer service for one reviewed build; it does not command an update. Trial dispatch requires a clean identifiable source commit. Test-only bypasses are not a deployment procedure; see [fixtures](fixtures.md) and [recovery limits](../devices/plaf103/recovery.md).

**Replacement install (stock-to-replacement).** `ota-images --directory private/verification/<review>` serves that frozen review's `image.bin` as `/ota/<token>/replacement.bin`. It refuses an image that is not a build's, is larger than the stock footprint (1,376,288 bytes), lacks the loader marker, or does not match the review's `ota_sha256`. The review's layout audit is what establishes the container header and byte-sum stock checks. There is no `original.bin` beside it: the way back is the UART stock recovery. `--peer` defaults to the feeder found by its MAC on the hotspot interface, and the lease also records the served digest. A trial needs that lease, and announces target version `2.0.0` with the lease's digest. Stock never reports after installing it, and the build speaks no MQTT, so the trial closes as `replacement_healthy` when the operator's client reports that the build answered its device API from bank 0 after the submission (`POST /api/ota/trial/answered`, with a note of what it saw). The server takes that word as it takes a resolution's: it sends nothing, and only the latest open trial closes this way. `wellfed-bench stock-ota` drives the whole sequence ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)).

The server answers stock's schedule queries from the imported snapshot, and a trial needs that snapshot empty. Nonempty-plan operation and stock time-recovery behavior remain unvalidated; see [stock behavior](../devices/plaf103/stock/README.md). Restore prior routing before shutting down the service if the feeder depends on that route.
