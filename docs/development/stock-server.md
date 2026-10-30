# Stock server

`wellfed-stock` is the server a feeder running stock 1.1.15 installs the replacement from. This guide sets it up, first with synthetic inputs and then for a real device or an install. It is for developers at the bench; the owner's installer, `wellfed install`, runs it for you.

The server exists only for stock. It holds an embedded authenticated broker, the stock protocol as far as an install needs it, the image server stock downloads from, and a bearer-authenticated loopback API. The replacement firmware speaks no MQTT. Its updates and status go over its [device API](../firmware/device-api.md), which `wellfed-bench` and the owner's `wellfed` CLI use directly.

## How the crate is laid out

The [`wellfed-stock`](../../src/tools/wellfed-stock) crate is both the library and the binary.

- **The library** holds everything stock speaks: its MQTT protocol, and the broker with stock's two identities (the stock device and the server) and the CONNECT normalization in front of it. It also holds the server's MQTT session with stock, the loopback API, the setup frame, the image server and the install-trial ledger. `server::Server` runs the broker, the session and (optionally) the API as tasks in the caller's process, and `ota::image::start` the image server; the caller owns and stops them.
- **The binary** adds the command line and the `network` redirect. Its `serve` and `ota-images` are those same entry points. The binary, `network` and the neighbour lookup sit behind the default `bench` feature.

The integration suite runs the binary as a process. The suite and the bench page speak its API through [`wellfed-stock-api`](../../src/tools/wellfed-stock-api), which links none of the server. The one crate that links the library is the install driver, [`wellfed-install`](../../src/client/wellfed-install), without the `bench` feature: `wellfed install` and the Android app run stock's side in their own process. `cargo xtask check-repository` refuses any other crate that depends on `wellfed-stock`.

The server does not control feeding on stock. The product uses stock only to install from and roll back to.

## Try it with synthetic inputs

No feeder needed.

1. Build it:

   ```sh
   cargo build --locked -p wellfed-stock
   ```

2. Write a configuration from the example inputs:

   ```sh
   ./target/debug/wellfed-stock init \
     --credentials src/tools/wellfed-stock/examples/mqtt-credentials.json \
     --capture src/tools/wellfed-stock/examples/mqtt-capture.json \
     --mqtt-listen 127.0.0.1:18883 \
     --hotspot-interface test_wifi \
     --output private/example-server.json
   ```

   You should see `private/example-server.json` written, mode 0600 on Unix. This sends no device commands.

The [example inputs](../../src/tools/wellfed-stock/examples/README.md) contain only synthetic identities. A second import to the same path is refused. `--timezone` takes an IANA time zone and defaults to `UTC`. The generated API token and the server password are independent.

## Configure a real device

Use your own isolated gateway or access point, and capture only the device you mean to control. A laptop that merely shares the Wi-Fi may not see the feeder's unicast traffic. Initial stock pairing normally uses the vendor app. Cloud-free provisioning through the setup frame and an [installer's server](#an-installers-server-trust-on-first-use) is host-tested only.

1. Store the device's single MQTT CONNECT credential record as a JSON array, in the shape the credential example shows.

2. Export the decoded MQTT packets as Wireshark/TShark JSON:

   ```sh
   tshark -r private/capture.pcap -Y mqtt -T json > private/mqtt.json
   ```

   You should see `_source.layers.mqtt` and the Ethernet source fields, as in the capture example. A capture with different dissector nesting needs normalizing first. Keep credentials and raw exports private.

3. Check the export holds what the importer needs: one device namespace, its source MAC, and an authoritative captured feeding-plan response (`GET_FEEDING_PLAN_EVENT` or `FEEDING_PLAN_SERVICE` with `plans`). The binding response is optional. Never invent an empty snapshot for a device that has schedules.

4. Run `init` with your own credential and capture paths, a specific IPv4 broker address the feeder can reach, your gateway interface and your time zone.

   You should see `private/server.json`, the default output. Wildcard and IPv6 broker binds are refused. The HTTP API defaults to loopback `127.0.0.1:8765`.

5. Start the server and check it:

   ```sh
   ./target/debug/wellfed-stock serve --config private/server.json
   ./target/debug/wellfed-stock status --config private/server.json
   ./target/debug/wellfed-stock network preview --config private/server.json
   ```

   Starting the service does not redirect traffic or command feeding.

The configuration format is version 1. An older configuration may still carry `max_portions`, which is refused as an unknown field: delete that line. Edit the configuration while the service is stopped, and keep the ledger beside it.

### Redirect the feeder's broker traffic

`network apply` is a separate, privileged Linux gateway operation. It validates an nft transaction, redirects the configured feeder MAC's TCP/1883 traffic on the configured interface, and blocks that feeder's forwarded WAN traffic.

1. Read the `network preview` output, and confirm the device identity it names.
2. Run `network apply`.
3. Check the counters and the actual traffic. Don't assume WAN isolation without checking.

`--redirect off` (on `apply` and `preview`) leaves out only the 1883 redirect. The WAN block and both input accepts stay, so the feeder reaches the broker only by the name it resolves, as on the DNS route.

`network remove` removes Wellfed's dedicated table and its tagged input rules. Their names are unchanged since the server was renamed, so rules applied before the rename are removed too.

Setting up the access point, host-wide firewall policy, persistent services and reboot persistence are outside this workflow. If the feeder depends on the redirected route, restore the prior routing before you shut the service down.

## An installer's server: trust on first use

An install needs no capture. `init-installer` writes a config with fresh secrets and the broker address the setup frame names. It needs no device at all.

```sh
./target/debug/wellfed-stock init-installer --mqtt-listen 192.168.1.20:1883 --output private/installer.json
./target/debug/wellfed-stock serve --config private/installer.json --enroll-minutes 15
```

- **Enrollment.** With no device known, `serve` starts the broker first and waits. Within the window, it admits the first CONNECT that looks like stock's: a client ID stock's topics can carry (not the server's), with a username and password, from a known relayed address. It records that device beside the config, mode 0600, named for the config (`installer.json`'s is `installer.device.json`). The record holds the client ID (which is the device ID), username, password and the address it connected from. Then the rest of the server starts with that device.
- **Stock's first reports.** Stock sends its clock request, its start event and its version report as soon as it is admitted, and only heartbeats after. The broker keeps no message for a later subscriber, so it tells the server of the enrollment at once and holds the feeder's CONNACK until the server's session has subscribed (at most 3 s). Before, the server subscribed 0.7 s too late and the version report was lost (physical, October 30).
- **Pinned.** The record is created whole or not at all: written aside, then hard-linked into place, which never replaces. So a reader never sees half a record, and a killed broker leaves none behind. A second feeder, a wrong password, or anyone after the window is refused, as with a captured config. A later `serve` needs no window and answers the enrolled feeder alone. To start over, stop the server and remove the record.
- **Contested.** Inside the window, any second stock-shaped identity, or a wrong password for the enrolled one, is noted in `installer.contested.json`, without its credentials. The status document's `enrollment` shows the device, its address and the contest. `POST /api/ota/trial` refuses (409) while the contest stands, because which feeder is the owner's can no longer be told. Only attempts inside the window count. After it closes, a second identity is refused but not flagged, so the installer's owner check of the device ID and address is then the only one.
- **One or the other.** A config that names a captured device and has an enrollment beside it is refused.
- **No redirect.** An installer's config has no hotspot interface or feeder MAC, so `network` refuses it. `ota-images` admits the enrolled feeder's address unless `--peer` is given.
- **Stock's plans are left alone.** An installer's config holds no plan snapshot, so `GET_FEEDING_PLAN_EVENT` goes unanswered. Stock erases and rewrites its plan table on any reply that has a `plans` key, even an empty one. It does not check the code or the message ID, and it takes the message unasked. With no reply it keeps its table; it asks once and does not retry. An empty answer would erase the owner's plans before the installer can import them. The server never sends a plan message on its own: the commands it sends unasked are a closed set (`protocol::Sent`). (Static analysis of 1.1.15's KM4 image, with one UART capture of a stock install.)
- **Stock's clock is set only when it is close.** Stock may hold the owner's plans, and we have not checked whether it runs meals that came due while its clock jumps forward. So with no snapshot, the `NTP` reply sets stock's clock (`calibrationTag`) only when stock's own clock is within 10 minutes of the server's, either way. That is no bigger a step than its vendor cloud's sync would make. Stock's `NTP` request carries its own clock in `ts`, and that alone decides. A request with no `ts`, a zero or an implausible one counts as unknown and is held; an earlier message's `ts` does not count, since it may predate a restart.
  - **When held,** stock's clock stays where it was. Stock 1.1.15 refused an install with error 1214 while its clock was two days behind, likely for that reason. `wellfed install` then names the flag below.
  - **`serve --calibrate-clock`** (and `wellfed install --calibrate-clock`, which passes it on) sets the clock whatever it reads. If stock's clock is far behind, stock may then run meals that came due in the jump, with real food.
  - **The log** says, for every `NTP` request, stock's `ts` (or `unknown`), the skew in seconds, and whether the clock was set and why. A held clock adds a warning that names the flag.
  - **A captured snapshot** keeps its own rule: an empty table always sets the clock, and a nonempty one (a seeding config) holds it unless the flag is passed. With the flag, those captured plans may run any meals that came due in the jump.

### A set-up stock, moved by DNS

The consumer install moves a stock that is already set up: the owner's resolver answers stock's broker name with this server's address, and stock restarts ([moving stock by DNS](../firmware/consumer-installation.md#moving-stock-by-dns)). The server needs nothing new for it:

- **Its firewall on the bench.** `network apply --redirect off` keeps the feeder's WAN block and both input accepts but redirects nothing, so stock reaches the broker only by the name it resolves, and a failed override fails closed.
- **Its port.** Stock keeps its stored port, so `--mqtt-listen` must use port 1883 on an address the owner's resolver can name. `wellfed install --broker-name` refuses any other port.
- **Its credentials.** Stock presents its own, as after a setup frame, so enrollment trusts it on first use the same way.
- **No binding.** A set-up stock sends no `BINDING` when it connects (observed). An installer's config has no captured binding reply, which only a stock fresh from the setup frame would ask for.
- **Its settings stay.** The server never sends `DEVICE_CONFIG_SYNC`, `WIFI_CHANGE_SERVICE` or the MQTT `RESTORE` (`protocol::Sent` is a closed set). So stock's stored broker name, Wi-Fi and plans are untouched, and removing the override returns stock to the vendor's cloud.
- **Its clock.** A stock that was on its vendor's cloud has a clock close to the right time, so the `NTP` reply sets it ([above](#an-installers-server-trust-on-first-use)). A stock whose clock is more than 10 minutes off needs `--calibrate-clock`.

Modeled: the installer's DNS step and enrollment of a never-seen identity, on the host. Not yet run on stock: a set-up stock reaching this server through a resolver override, and an install with its clock set within the bound or by `--calibrate-clock`.

`wellfed-stock broker-name <file>` prints the broker name stock's settings store, from a whole flash read or the settings sector alone. The name is the vendor's, so no tracked file carries it; the bench reads it from its own flash reads.

`wellfed install` ([consumer installation](../firmware/consumer-installation.md#the-installer)) runs this server as its child.

## Seed stock's plans on the bench

The import is only proven against a stock that holds plans. Stock stores whatever table a `GET_FEEDING_PLAN_EVENT` reply carries, so the bench seeds it through a captured config with a test table. This needs the bench with stock 1.1.15 on bank 1, reaching this server. No motion: every seeded plan carries a far-future `skipEndTime`, so stock stores it but never runs it.

1. Write the test table to a file, here `private/seed-plans.json`. Give every plan `repeatDay` and a `skipEndTime` far ahead, and at most 3 portions:

   ```json
   [
     {"planId": 9001, "executionTime": "07:30", "grainNum": 2, "repeatDay": [1, 2, 3, 4, 5, 6, 7],
      "enableAudio": false, "audioTimes": 0, "syncTime": 1792271861000, "skipEndTime": 4102444800000},
     {"planId": 9002, "executionTime": "18:05", "grainNum": 3, "repeatDay": [6, 7],
      "enableAudio": true, "audioTimes": 2, "syncTime": 1792271861000, "skipEndTime": 4102444800000}
   ]
   ```

   `4102444800000` is 2100-01-01. The table's fields are in [how stock receives plans](../devices/plaf103/stock/README.md#how-stock-receives-plans).

2. Write a seeding config from the bench's captured one, keeping its mode:

   ```sh
   (umask 077; jq --slurpfile p private/seed-plans.json '.plans = $p[0]' private/server.json > private/seed.json)
   ```

3. Run `./target/debug/wellfed-stock serve --config private/seed.json`, then restart stock, or return the feeder to stock. Stock asks for its plans once, within about 30 s of its boot, and stores the table.

   You should see stock online with version 1.1.15 in `wellfed-stock status --config private/seed.json`. A full flash read then decodes with `wellfed-stock plans <flash read>`: each plan disabled, with a note that its skip is still ahead.

4. Stop `serve`, and delete `private/seed.json`. Before any install trial, the server that stock reaches must hold no snapshot (`plans` absent), so the request goes unanswered and stock keeps the seeded table. A trial refuses while a nonempty snapshot is configured, and `wellfed install` refuses any config with a `plans` key.

The seeded plans reach the replacement disabled. Delete them with `wellfed plans delete` after the run.

## The install ledger

Install trials are recorded in `stock-ledger.json` beside the configuration, under an exclusive lock.

- Each trial is persisted before its one `OTA_UPGRADE` goes out, and is never resent. A repeated request ID returns the stored record.
- A trial in flight when the server stops or loses the broker becomes uncertain.
- An uncertain or failed trial blocks the next until `ota-trial-resolve` closes it from fresh evidence. Resolving sends no command.
- A replacement install closes instead on `ota-trial-answered`, once the operator's client saw the served build answer its device API from bank 0 and stock has gone quiet ([below](#install-the-replacement)).

`serve --enable-ota-trial` holds the [bench lease](../devices/plaf103/bench.md#bench-lease) beside its configuration for as long as it runs, since an install it dispatches runs for minutes. `ota-trial` only asks that server, so it takes no lease of its own. `setup-send` takes the lease for its one frame. A plain `serve`, `broker` and `ota-images` take none: they only answer, beside the command that drives the feeder.

A server with the trial enabled starts only from a reviewable build: a clean commit of every input `build.rs` records. Those cover `wellfed-stock`, the crates it links, and the crates that decide and send the request in an installer (`wellfed-client`, `wellfed-install`). `server::Server::start` enforces it, so the binary and an in-process installer are refused alike; tests set `WELLFED_ALLOW_UNREVIEWED_OTA_BUILD`.

An older ledger, `feed-ledger.json`, also held feeds and probe queries. `serve` refuses to start while it is present. Check that no trial in it is submitted, uncertain or failed, then rename it.

## HTTP API

Write bodies are limited to 4096 bytes. Authenticate with the bearer token from the private configuration. The CLI handles it without putting the token in shell arguments.

| Route | Does |
| --- | --- |
| `GET /health` | Public process and build identity (`"service": "wellfed-stock"`) |
| `GET /api/status` | Authenticated stock telemetry, build identity, install trials and stock's OTA reports |
| `POST /api/ota/trial` | `{ "request_id": "UUID", "server_token": "UUID" }`: the stock-to-replacement install. Needs explicit trial mode and a live image lease |
| `POST /api/ota/trial/resolve` | Explicit uncertain-trial reconciliation. Sends no command |
| `POST /api/ota/trial/answered` | `{ "request_id": "UUID", "build": "64 hex", "bank": 0, "note": "…" }`: the latest submitted or uncertain trial's served build answered the device API from bank 0 after it. Closes it as `replacement_healthy` and sends no command. 409 while stock still reports, or for another build or bank |

Normal `serve` mode keeps trial dispatch disabled. Trial dispatch needs a clean, identifiable source commit. Test-only bypasses are not a deployment procedure; see [fixtures](fixtures.md) and [recovery limits](../devices/plaf103/recovery.md).

## Install the replacement

`wellfed-bench stock-ota` drives this whole sequence ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)).

1. Serve the frozen review:

   ```sh
   ./target/debug/wellfed-stock ota-images --directory private/verification/<review>
   ```

   You should see the review's `image.bin` served as `/ota/<token>/replacement.bin`. `ota-images` is only a transfer service for one reviewed build; it does not command an update. `--peer` defaults to the feeder found by its MAC on the hotspot interface.

   It refuses an image that is not a build's, is larger than the stock footprint (1,376,288 bytes), lacks the loader marker, or does not match the review's `ota_sha256`. The review's layout audit is what establishes the container header and byte-sum stock checks. There is no `original.bin` beside it: the way back is the UART stock recovery.

   The lease records the served digest and the build's identity: the startup identity of the review's frozen `build-manifest.json`, which must match the review's `manifest_sha256`.

2. Check the schedule snapshot is empty or absent. A captured snapshot is replayed to stock's schedule queries, so a trial needs it empty. With no snapshot (an installer's config), the queries go unanswered and stock keeps its own plans, which the installer imports.

3. Start a trial with `POST /api/ota/trial` against a `serve --enable-ota-trial`. It needs the lease, announces target version `2.0.0` with the lease's digest, and records both in the ledger.

4. Close it with `POST /api/ota/trial/answered`. Stock never reports after installing the build, and the build speaks no MQTT, so the operator's client reports instead. The report gives the build identity and bank the build's device API answered with after the submission, and a note of what it saw.

The server sends nothing, and closes only the latest submitted or uncertain trial, and only when both hold:

- **The answer names the served build on bank 0.** Its `build` equals the identity recorded at submission, and `bank` is 0. A record from before the identity was kept never closes this way.
- **Stock is not demonstrably running.** Its telemetry is not current (none for 90 s), and it has reported no version since the submission. A version report after the submission marks the trial for good (`stock_reported` in the ledger). Stock was still running then, so a dropped session that came back on 1.1.15 cannot be closed by an answer; only `ota-trial-resolve`'s version evidence resolves it. This fails safe: if stock also reports its version during a download, an install that did succeed waits for the resolution as before.

The answer is still the client's word. The server checks it against what it served, not against the device API itself. `wellfed-itest`'s bank-0 step asks for 120 s, past stock's telemetry window.

Operation with a nonempty plan, and stock's time-recovery behavior, are unvalidated; see [stock behavior](../devices/plaf103/stock/README.md).

## The replacement firmware (deleted roles)

The broker once served the replacement firmware too, as a role with its own credential and namespace (`firmware-credential`, health, requests and `ota-offer`). That role is deleted, with the firmware's MQTT transport. Builds take updates over the [device API](../firmware/ota-design.md#updates-over-the-device-api), which `wellfed-bench ota` pushes, and `wellfed-build` no longer reads `private/fw-mqtt.json`.

A configuration written while the role existed keeps its `firmware` section. `wellfed-stock` ignores it unread, so the file loads as it is.

## Evidence

- **Observed:** local dispensing on stock 1.1.15 was demonstrated on the research unit, before stock control was removed from this server. The code is in Git history.
- **Modeled:** `tests/protocol.rs` loads a seeding config from a file and replays its table exactly as written, with calibration held, then leaves the request unanswered once `plans` is removed.
- **Observed (UART captures of 1.1.15, October 23 and 28, 2026, `fw40-stock-ota-05-uart-01` and `fp04-stock-idle-20261028n`):** stock's `NTP` request carries its own clock in `ts` (milliseconds, rounded to the second), and it asks once per connection. On October 28 its clock read October 26, the reply held calibration (`calibrationTag = 0`), and stock refused the install that followed with error 1214, with no download (`fpbatch-20261028n`). That the held clock caused the refusal is likely, not confirmed statically.
- **Modeled:** `clock.rs` unit-tests the rule (within the bound either way, outside it, unknown, the flag, a captured empty and a nonempty table), and `tests/protocol.rs` answers an installer's `NTP` dated 9 minutes off, 2 days off, and 2 days off with the flag, and holds one with no `ts` or a zero after an in-bound heartbeat.
- **Modeled:** `tests/enroll_stack.rs` runs the real `serve`, which enrolls a simulated feeder with never-seen credentials, hears its version report, refuses a second identity and a guessed password, and after a restart without the window admits only the enrolled feeder. Not yet run against stock.
