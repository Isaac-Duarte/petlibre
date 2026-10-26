# Stock server

The `wellfed-stock` executable is the bench's server for stock 1.1.15: an embedded authenticated broker, the stock protocol as far as installing the replacement needs it, the image server stock installs from, and a bearer-authenticated loopback API. It exists only for stock: the replacement firmware speaks no MQTT, and its updates and status go over its [device API](../firmware/device-api.md), which `wellfed-bench` and the owner's `wellfed` CLI use directly.

The [`wellfed-stock`](../../src/tools/wellfed-stock) crate is the library and the binary. The library holds everything stock speaks: its MQTT protocol, the broker with stock's two identities (the stock device and the server) and the CONNECT normalization in front of it, the server's MQTT session with stock, the setup frame, the image server and the install-trial ledger. The binary adds the command line, the loopback API and the `network` redirect. The suite runs it as a process; the suite and the page speak its API through [`wellfed-stock-api`](../../src/tools/wellfed-stock-api), which links none of it, and `cargo xtask check-repository` refuses any other crate that depends on `wellfed-stock`. The server was the `wellfed-local` binary until October 26, 2026, with the same commands, configuration, ledger and API.

Its stock control (manual feeds, the feed ledger, `resolve`) and the C-probe and modified-stock trials were removed on October 24, 2026: the product uses stock only to install from and roll back to. Local dispensing on stock 1.1.15 was demonstrated on the research unit before that, and the code is in Git history.

## Build and try synthetic initialization

```sh
cargo build --locked -p wellfed-stock
./target/debug/wellfed-stock init \
  --credentials src/tools/wellfed-stock/examples/mqtt-credentials.json \
  --capture src/tools/wellfed-stock/examples/mqtt-capture.json \
  --mqtt-listen 127.0.0.1:18883 \
  --hotspot-interface test_wifi \
  --output private/example-server.json
```

The [example inputs](../../src/tools/wellfed-stock/examples/README.md) contain only synthetic identities. This command writes configuration and sends no device commands. A second import to the same path is refused. `--timezone` accepts an IANA timezone and defaults to `UTC`. The generated API token and server password are independent; configuration is mode 0600 on Unix.

## Configure an actual device

Use your own isolated gateway/AP and capture only the device you intend to control. Initial stock pairing normally uses the vendor app; cloud-free provisioning through the setup frame and an installer's server (below) is host-tested only. A laptop merely sharing Wi-Fi may not see the feeder's unicast traffic.

Store the device's single MQTT CONNECT credential record as a JSON array in the shape illustrated by the credential example. Export decoded MQTT packets using Wireshark/TShark JSON, with the `_source.layers.mqtt` and Ethernet source fields illustrated by the capture example. The importer requires one device namespace, its source MAC, and an authoritative captured feeding-plan response (`GET_FEEDING_PLAN_EVENT` or `FEEDING_PLAN_SERVICE` with `plans`). It optionally imports the binding response. Do not invent an empty snapshot for an existing device with schedules.

A typical decoded packet export is `tshark -r private/capture.pcap -Y mqtt -T json > private/mqtt.json`. Inspect the export locally against the documented shape: captures with a different dissector nesting require normalization. Credentials and raw exports must stay private.

Run `init` with your own credential/capture paths, a specific IPv4 broker address reachable from the feeder, your gateway interface, and your timezone. Wildcard and IPv6 broker binds are rejected. `private/server.json` is the default output. The HTTP default is loopback `127.0.0.1:8765`. The configuration format remains version 1; a configuration from before FW51 still carries `max_portions`, which is refused as an unknown field, so delete that line. Edit configuration while the service is stopped and retain the ledger beside it.

```sh
./target/debug/wellfed-stock serve --config private/server.json
./target/debug/wellfed-stock status --config private/server.json
./target/debug/wellfed-stock network preview --config private/server.json
```

Starting the service does not redirect traffic or command feeding. Linux gateway `network apply` is a separate privileged operation: it validates an nft transaction, redirects the configured feeder MAC's TCP/1883 traffic on the configured interface, and blocks its forwarded WAN traffic. Inspect preview and confirm device identity before explicitly invoking it. `network remove` removes Wellfed's dedicated table and its tagged input rules; their names are unchanged since the server's rename, so rules applied before it are removed too. AP setup, host-wide firewall policy, persistent services, and reboot persistence are outside this workflow. Do not assume WAN isolation without checking counters and actual traffic.

## An installer's server: trust on first use

An install needs no capture: `init-installer` writes a config with fresh secrets and the broker address the setup frame names, and no device at all.

```sh
./target/debug/wellfed-stock init-installer --mqtt-listen 192.168.1.20:1883 --output private/installer.json
./target/debug/wellfed-stock serve --config private/installer.json --enroll-minutes 15
```

- **Enrollment:** with no device known, `serve` starts the broker first and waits. Within the window, the first CONNECT that looks like stock's (a client ID stock's topics can carry, not the server's, with a username and password, from a known relayed address) is admitted and recorded beside the config, named for it (`installer.json`'s is `installer.device.json`), mode 0600: its client ID (which is its device ID), username, password and the address it connected from. Then the rest of the server starts with that device.
- **Pinned:** the record is created whole or not at all (written aside, then hard-linked into place, which never replaces), so `serve` polling for it never reads half a record and a killed broker leaves none behind. A second feeder, a wrong password, or anyone after the window is refused as with a captured config, and a later `serve` (no window needed) answers the enrolled feeder alone. To start over, stop the server and remove the record.
- **Contested:** inside the window, any second stock-shaped identity (or a wrong password for the enrolled one) is noted in `installer.contested.json`, without its credentials. The status document's `enrollment` shows the device, its address and the contest, and `POST /api/ota/trial` refuses (409) while it stands: which feeder is the owner's can no longer be told. Only attempts inside the window count: after it closes, a second identity is refused but not flagged, so the installer's owner check of the device ID and address is then the only one.
- **One or the other:** a config that names a captured device and an enrollment beside it is refused.
- **No redirect:** an installer's config has no hotspot interface or feeder MAC, so `network` refuses it; `ota-images` admits the enrolled feeder's address unless `--peer` is given.
- **Host-tested** (`tests/enroll_stack.rs`, simulation): the real `serve` enrolls a simulated feeder with never-seen credentials, hears its version report, refuses a second identity and a guessed password, and after a restart without the window admits only the enrolled feeder. Not yet run against stock.

`wellfed install` ([consumer installation](../firmware/consumer-installation.md#the-installer)) runs this server as its child.

## The replacement firmware (deleted roles)

The broker once served the replacement firmware too, as a role with its own credential and namespace (`firmware-credential`, health, requests and `ota-offer`; the [transport design](../firmware/local-transport.md) records it). It is deleted, with the firmware's MQTT transport: builds take updates over the [device API](../firmware/ota-design.md#updates-over-the-device-api), which `wellfed-bench ota` pushes, and `wellfed-build` no longer reads `private/fw-mqtt.json`. A configuration written while the role existed keeps its `firmware` section; `wellfed-stock` ignores it unread, so the file loads as it is.

## The install ledger

Install trials are recorded in `stock-ledger.json` beside the configuration, under an exclusive lock. Each trial is persisted before its one `OTA_UPGRADE` goes out and is never resent: a repeated request ID returns the stored record, and a trial in flight when the server stops or loses the broker becomes uncertain. An uncertain or failed trial blocks the next until `ota-trial-resolve` closes it from fresh evidence, which sends no command. A replacement install closes instead on `ota-trial-answered`, once the operator's client saw the served build answer its device API from bank 0 and stock has gone quiet (below).

`serve --enable-ota-trial` holds the [bench lease](../devices/plaf103/bench.md#bench-lease) beside its configuration for as long as it runs, since an install it dispatches runs for minutes; `ota-trial` only asks that server, so it takes none of its own. `setup-send` takes the lease for its one frame. A plain `serve`, `broker` and `ota-images` take none: they only answer, beside the command that drives the feeder.

The ledger before FW51, `feed-ledger.json`, also held feeds and probe queries. `serve` refuses to start while it is present: check that no trial in it is submitted, uncertain or failed, then rename it.

## HTTP API

Write bodies are bounded to 4096 bytes. Use bearer authentication from the private configuration; the CLI handles it without placing the token in shell arguments.

| Route | Interface |
| --- | --- |
| `GET /health` | Public process/build identity (`"service": "wellfed-stock"`) |
| `GET /api/status` | Authenticated stock telemetry, build identity, install trials and stock's OTA reports |
| `POST /api/ota/trial` | `{ "request_id": "UUID", "server_token": "UUID" }`: the stock-to-replacement install; explicit trial mode and a live image lease required |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation; sends no command |
| `POST /api/ota/trial/answered` | `{ "request_id": "UUID", "build": "64 hex", "bank": 0, "note": "…" }`: the latest submitted or uncertain trial's served build answered the device API from bank 0 after it; closes it as `replacement_healthy`, sends no command. 409 while stock still reports, or for another build or bank |

Normal `serve` mode keeps trial dispatch disabled. `ota-images` is a separate HTTP transfer service for one reviewed build; it does not command an update. Trial dispatch requires a clean identifiable source commit. Test-only bypasses are not a deployment procedure; see [fixtures](fixtures.md) and [recovery limits](../devices/plaf103/recovery.md).

**Replacement install (stock-to-replacement).** `ota-images --directory private/verification/<review>` serves that frozen review's `image.bin` as `/ota/<token>/replacement.bin`. It refuses an image that is not a build's, is larger than the stock footprint (1,376,288 bytes), lacks the loader marker, or does not match the review's `ota_sha256`. The review's layout audit is what establishes the container header and byte-sum stock checks. There is no `original.bin` beside it: the way back is the UART stock recovery. `--peer` defaults to the feeder found by its MAC on the hotspot interface. The lease records the served digest and the build's identity (the startup identity of the review's frozen `build-manifest.json`, which must match the review's `manifest_sha256`). A trial needs that lease, announces target version `2.0.0` with the lease's digest, and records both in the ledger.

Stock never reports after installing it, and the build speaks no MQTT, so the trial closes as `replacement_healthy` on the operator's client's report (`POST /api/ota/trial/answered`): the build identity and bank the build's device API answered with after the submission, and a note of what it saw. The server sends nothing and closes only the latest submitted or uncertain trial, and only when:
- **the answer names the served build on bank 0:** its `build` equals the identity recorded at submission, and `bank` is 0 (a record from before the identity was kept never closes this way);
- **stock is not demonstrably running:** its telemetry is not current (none for 90 s), and it has reported no version since the submission. A version report after the submission marks the trial for good (`stock_reported` in the ledger): stock was still running then, so a dropped session that came back on 1.1.15 cannot be closed by an answer, and only `ota-trial-resolve`'s version evidence resolves it. The marking fails safe: if stock also reports its version during a download, an install that did succeed waits for the resolution as before.

The answer is still the client's word: the server checks it against what it served, not the device API itself. `wellfed-itest`'s bank-0 step asks for 120 s, past stock's telemetry window. `wellfed-bench stock-ota` drives the whole sequence ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)).

The server answers stock's schedule queries from the imported snapshot, and a trial needs that snapshot empty. Nonempty-plan operation and stock time-recovery behavior remain unvalidated; see [stock behavior](../devices/plaf103/stock/README.md). Restore prior routing before shutting down the service if the feeder depends on that route.
