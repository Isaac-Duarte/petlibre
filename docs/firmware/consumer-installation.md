# Consumer installation and rollback

This page explains how a feeder on **stock 1.1.15** can move to Wellfed with nothing but an installer app, and how it can go back. It is for owners wondering what installing involves, and for contributors working on the installer. The goal is an experience like the vendor app's, with your own server in place of the cloud.

No vendor code, credentials or device identifiers appear here. The evidence index for the static findings is private: `private/research/stock-installer/evidence.md`.

## The one idea

Stock's own updater does the hard part. Stock 1.1.15 will install any image we serve it into its other bank, with no UART and no soldering. So the installer only has to get stock talking to it: put stock in setup mode, point it at the installer's broker, ask for one update, and serve the image. Before Wellfed overwrites stock's last copy, the installer saves the feeder's own stock image, so you can always go back.

| Step | Status |
| --- | --- |
| Stock installs Wellfed over its updater | Works |
| Promotion to bank 1 and confirmation | Works |
| Going back to stock over the air | Works, using a stock image we serve |
| Pointing stock at the installer with its setup frame | Experimental: host-tested, not yet run on stock |
| The guided installer, `wellfed install` | Experimental |
| Saving and restoring the feeder's own stock image | Experimental |

## What already works on the bench

`wellfed-bench stock-ota` runs the whole move from stock to Wellfed ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)), and the trip back to stock works too ([Evidence](#evidence)). The image contract is in [stock OTA requirements](../devices/plaf103/stock/ota-requirements.md).

What made those runs work, and must stay in any installer:

| Requirement | Why |
| --- | --- |
| One `OTA_UPGRADE`, never resent | A refused precheck can erase stock's OTA status sector, and an uncertain request may still be downloading |
| Resolve a failed request before the next one | The server's ledger refuses a new trial while one is failed or uncertain. It resolves from fresh evidence: 20 min, 10 quiet connected minutes, a fresh version report |
| The image port reachable from the feeder | With only TCP 1883 allowed, stock connected for about 25 s, then reported `OTA_INFORM` error 2003 |
| Tolerate stock's ~61 s heartbeat | A 60 s telemetry bound restarted the quiet interval at every heartbeat |
| Ask stock for its attributes when its version report is stale | Idle stock reports its version only around a connect |
| A URL under 128 bytes, `Content-Length` title-cased, a direct `200` | Stock's parser limits ([requirements](../devices/plaf103/stock/ota-requirements.md#http-server-and-url-requirements)) |
| Mains power, idle, model and version checked first | Stock's own precheck; the server's trial gate checks the same reports |

What the bench still uses, and a consumer must not need:

- `wellfed-stock network apply`: a root firewall redirect of the feeder's TCP 1883, plus WAN blocking;
- device MQTT credentials captured from traffic (`wellfed-stock init`);
- a build carrying the owner's Wi-Fi credentials, broker credentials and update key.

Stock's setup protocol, below, removes the first two. Generic images remove the third.

## Stock's setup protocol

The vendor app provisions a PLAF103 through the feeder's **setup access point**, not over Bluetooth:

1. The feeder, in setup mode, runs its own Wi-Fi access point.
2. The app joins it and sends one UDP datagram to **192.168.1.1:17484** (`0x444C`). Stock's `Ap_Config_Udp_Thread` receives it on that port.
3. BLE service `ffee` is connected alongside (`ffe1` write, `ffe2` data, `ffe3` read, `ffe4` notify), but the frame itself goes over UDP.

**The frame.** PLAF103 is on the app's list of products sent without encryption, so the frame is plaintext with no checksum:

```text
'D' 'L' | type=3 | checksum flag=0 | 0 | payload length | lp(SSID) lp(password) lp("host:PPPP") lp(numeric id) | 0xFF
```

- **`lp(x)`** is one length byte, then the UTF-8 bytes.
- **Types:** 1 is Wi-Fi only; 2 server only; 3 both (what the app sends for PLAF103); 4 three other strings of up to 50 bytes, meaning unknown.
- **Field limits:** SSID up to 32 bytes; password up to 64; `host:PPPP` up to 100; numeric id up to 30, ASCII digits only.
- **`host:PPPP`** is the MQTT broker, which the app reads from its cloud configuration. Stock takes the host before the `:` and up to four port digits after it, so 1883 fits.
- **The numeric id** is most likely the account's user id. It must be digits, and nonzero.

**What stock does with it** (stock functions at runtime addresses):

- **Parsing:** `0e01a160` checks the `DL` prefix and the `0xFF` terminator, then dispatches on the type.
- **Wi-Fi:** stored by `0e007bb0`.
- **Server:** `0e007cf8` (`Set_Dev_Info`) stores the port, numeric id and host in a 0x484-byte record.
- **Applying it:** the setup thread `0e0084d8` sets the MQTT port, host and id. It then polls whether MQTT came up (`0e012780`, up to 20 tries). Only then does it persist the record to flash `0x3e8000`, and a 0x60-byte record to `0x3e9000`.
- **At every boot:** stock loads `0x3e8000` and connects to that host and port.

**Re-provisioning guard.** While the `0x3e9000` record is non-blank and the device state is bound, types 1–3 are acknowledged but not applied. A feeder must be reset or unbound first.

**Credentials.** The MQTT client ID, username and password the feeder presents are its own: they are loaded from flash at boot, not issued at bind. `BINDING` is an application-level association, which `wellfed-stock` already answers by replaying the captured cloud reply. The commands stock dispatches over MQTT are `OTA_UPGRADE`, `GET_CONFIG`, `BINDING`, `UNBIND`, `DEVICE_REBOOT`, `WIFI_RECONNECT_SERVICE`, `WIFI_CHANGE_SERVICE`, `ATTR_SET_SERVICE` and `ATTR_GET_SERVICE` ([stock behavior](../devices/plaf103/stock/README.md)).

**Getting into setup mode** is not yet verified. Stock's panel has one reset gesture: Reset (the rightmost button) held about 5 s on an unlocked panel starts the factory restore. No Wi-Fi-only hold exists ([stock behavior](../devices/plaf103/stock/README.md#panel-buttons-and-child-lock)). The panel is unlocked by holding Lock about 5 s while the lock light is on. The first physical test must show that the restore leaves stock in setup mode with its access point up, and clears the bound state the re-provisioning guard checks. The access point's name is not established either.

**Open questions,** to settle on the feeder:
- Is type 3 really what goes on the wire? Reading the decompiled app depends on its integer encoding, so a capture should confirm it.
- What exactly is the numeric id?
- How is a feeder reset or unbound? Is there a button hold? What does `UNBIND` (`0e015fd4`, not decompiled) clear?
- Are the credentials fixed at the factory, or derived from the serial? Either way they arrive at our broker unchanged.

**The planned first test.** With stock in setup mode, the bench host joins its access point. It sends one type-3 frame with `wellfed-stock setup-send --ssid <hotspot> --id <digits>`, which reads the passphrase from stdin and names this server's broker (`10.42.103.1:1883`) unless `--broker` is given. If stock then joins the hotspot and connects to `wellfed-stock serve` with its own credentials, the redirect and the captured credentials are no longer needed. The frame encoder (`wellfed_client::install::frame`) is host-tested against the parser's length rule and field limits.

## What the installer's server must answer

`wellfed-stock` already implements the stock side:

- a broker on TCP 1883 that accepts stock's legacy MQTT 3.1 CONNECT;
- the `dl/PLAF103/<device-id>/…/post` and `/sub` topics;
- replies to `NTP` and `BINDING`, and to `GET_FEEDING_PLAN_EVENT` only with a captured plan snapshot. An installer's config has none, so that request goes unanswered: stock erases its plan table on any reply with a `plans` key, and the installer must find the owner's plans intact ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use));
- acknowledgements of the startup, attribute, log and error events.

For a consumer it must also accept a device whose credentials it has never seen. **Trust on first use:** an installer's config (`wellfed-stock init-installer`) names no device. `serve --enroll-minutes N` admits the first stock-shaped CONNECT within the window. It records that client's ID, username, password and address beside the config (`installer.json`'s record is `installer.device.json`) and pins it, so nobody else logs in, then or later ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use)). Stock's client ID is its device ID. The captured-credential `init` and the `network` redirect stay for the bench until the frame has provisioned stock on hardware.

## The install flow

1. **Setup mode:** you reset the feeder into its setup access point, the same step as "add device" in the vendor app.
2. **Provision:** the installer joins that access point and sends one type-3 frame: your home network, plus the installer's own `host:1883`.
3. **Connect:** stock joins your home network and connects to the installer's broker with its own credentials, which the installer records.
4. **Pre-flight:** model PLAF103, version 1.1.15 on an allow-list, mains power, idle. These are stock's own precheck conditions; the bench's `stock-ready` stage checks them.
5. **Install:** the installer serves the image and sends one `OTA_UPGRADE`. Stock's progress reports drive a progress bar, for about 40 s.
6. **First boot:** the build boots from bank 0 and answers the device API while it waits for an update.
7. **Pair:** a build fresh from stock knows no client key, and the next two steps go over the device API, so the installer pairs first.
8. **Backup:** the build streams bank 1, which still holds stock, to the installer, which verifies it ([rollback](#rollback)).
9. **Stock's plans:** the build serves stock's two plan sectors, and the installer converts each plan or refuses it by name ([importing them](../devices/plaf103/stock/README.md#importing-them)).
10. **Promote:** only once a verified backup is stored, the installer offers the same build, signed, over the device API. Bank 0 installs it into bank 1, and its trial boot confirms.
11. **Plans applied:** the converted plans go onto the confirmed build, only if it has no plans yet.
12. **Hand-off:** the confirmed build enters the [pairing](pairing.md) flow. Your app is its first client, and a server joins later by invite if you run one.

Steps 1–3 replace the bench's firewall redirect and captured credentials.

## The installer

One run, `wellfed install`, goes from stock to a confirmed replacement. Its steps are a sans-IO state machine in [`wellfed_client::install`](../../src/client/wellfed-client/src/install.rs). The Android app and `wellfed-server` will drive it the same way; the CLI supplies the IO.

| Step | Decided in `wellfed-client` | IO in the CLI |
| --- | --- | --- |
| 1. Provision | The type-3 frame, refused before anything is sent if stock would misread a field; sent once | The owner puts stock into setup mode and joins this computer to its access point (platform Wi-Fi); one UDP datagram to 192.168.1.1:17484; the owner rejoins the home network |
| 2. Stock ready | Online, 1.1.15 (any other version ends the run), on its adapter | `wellfed-stock serve --enable-ota-trial --enroll-minutes N` as a child: the broker trusts the first feeder; its status document, polled |
| 3. Confirm the feeder | The owner checks the enrolled device ID and address before anything is sent; a contested enrollment ends the run | The status document's `enrollment` (never its credentials) and a `[y/N]` prompt. A bench config with a captured device needs no check |
| 4. Request | Exactly one `OTA_UPGRADE` per run; refused or uncertain ends the run | `ota-images` as a child, then `POST /api/ota/trial`. What the server recorded is read back: refused only when the server refused and a successful ledger read shows no record, uncertain otherwise. The server itself refuses (409) while the enrollment is contested |
| 5. Install | Progress; stock's refusal ends the run, even before the request's outcome is read; stock going quiet means it restarted | The trial phase, and the OTA reports since the request that name it (or nothing) |
| 6. Pair | The installer must be an owner before anything else | A key the build already knows opens a session at once. Otherwise the owner holds Lock, then Dispense, on the unlocked panel and keys in the code (`wellfed recover`'s exchange). The address is where stock connected from |
| 7. Bank 0 | Only the served build moves on. On bank 0 the stock server's record is then closed; on bank 1 (a run resumed after activation) only the confirmation is left | `Status` over the device API; `POST /api/ota/trial/answered`, asked again while stock's telemetry is still current |
| 8. Backup | **No promotion without a `VerifiedBackup`.** `Bank1Reader` reads bank 1 a page at a time, and `StockBackup::from_bank` restores the marker byte, checks the payload against the pin and rebuilds the vendor's container | `ReadBank1` over the device API (owners only, bank 0 only), the copy kept under `--backups` (`private/stock-backups/<identity>/stock-1.1.15.bin`), or one kept by an earlier run. `--stock-backup` instead takes a copy the owner holds, checked before the feeder is touched |
| 9. Stock's plans | `StockPlans::decode_parts` reads the sectors as stock's boot does, and `plans::convert` converts each plan or refuses it by name. **A table that cannot be read stops the run before promotion**, unless the owner chose to go on without it | `ReadStockPlans` over the device API (owners only, bank 0 only). The 441 bytes are kept beside the backup (`private/stock-backups/<identity>/stock-plans.bin`, never replaced by a different read), and the import is shown. `--without-stock-plans` promotes even when the sectors cannot be read |
| 10. Promote | `update::Installer`, the same push as the bench's. Its error ends the run. Once writing began, bank 1 may be partly overwritten, so the verified backup is then the copy of stock | Chunks over the device API until bank 1 is activated. A dropped session is reopened, and the install resumes where bank 0 says |
| 11. Confirm | Done only when bank 1 runs the build and reports the manifest's version confirmed; bank 0 means it fell back | `Status` and `UpdateState` after the restart |
| 12. Apply the plans | Only with converted plans, and only onto a writable table with every slot empty: plans already set stop it. One `PutPlans` from the generation read; `Stale` or another refusal changed nothing and ends the run. A table already holding the import counts as applied. An unanswered put is never sent again: the plans read back decide, the import there is done, anything else is uncertain. A run resumed after the promotion reads no sectors, so it takes the copy an earlier run kept, under the same check | `GetPlans`, then the one `PutPlans`, then `GetPlans` again if it went unanswered. A resumed run loads `stock-plans.bin` from beside the backup, or says that stock's plans were not read |

**Trust on first use, and its risk.** The broker trusts whichever stock-shaped client reaches it first inside the window. A host on the same network that wins that race (or a second feeder) would be enrolled instead. Without a check, it could be walked through the whole run to a false "Installed" while the real feeder stays on stock. So:

- The run stops for the owner to confirm the enrolled device ID (on the feeder's label or in the vendor app) and its address (the router's client list) before the install request.
- Any second stock-shaped identity inside the window, or a wrong password for the enrolled one, marks the enrollment contested (`installer.contested.json` beside the config). The status shows it, the server refuses to install, and the run ends.
- A contest counts only inside the window. Once it has closed, a second identity (even the real feeder arriving late, after a stranger was enrolled) is refused like any wrong password, but not flagged. The owner's confirmation of the device ID and address is then the only check left.
- A contested enrollment is cleared by removing both files and running again on a network only the feeder joins.
- The device API's pairing is the second check: a false feeder cannot complete the setup code keyed in on the feeder's own panel.

**The backup gate.** `VerifiedBackup::verify` (and the `StockBackup` the stream makes) is the only way to make the token the promotion waits for.
- It takes either stock 1.1.15's bare application payload, whose SHA-256 is the pinned payload's (`802ef620…`, the stock review's `payload_sha256`), or the vendor's exact container (`05147476…`).
- The payload is pinned because the bank holds the payload, and a factory-flashed unit may never have been written from that exact container.
- A container is never taken apart. Any other header over the right payload is refused, since its fields are not checked and a restore would write it as given.
- At the backup step the run reads bank 1 from the replacement on bank 0, and keeps the rebuilt container. `--stock-backup` takes a copy the owner already holds instead (on the bench, the stock review's `image.bin`), kept the same way before promotion.
- A kept file is written whole or not at all (a temporary file hard-linked into place). One that does not check out stops the run, with the path to delete.
- If the read is refused, or the bank does not match the pin, the run stops at the gate with the replacement on bank 0 and stock intact in bank 1. `wellfed install --from-bank0 …` resumes from pairing without asking stock anything, and reuses a copy kept by an earlier run. The same resume finishes a run that stopped after activation: it finds the build on bank 1 and waits for its confirmation.

**Which bank stock runs from** is an open question. Every install so far started from stock in bank 1, so stock wrote bank 0 and promotion overwrote bank 1. A consumer unit whose stock ran from bank 0 (after a vendor update, say) would have stock write the replacement into bank 1, leaving its own copy in bank 0. The steps above, the backup source and the promotion all assume the research unit's layout, and refuse anything else: the served build must answer from bank 0. This needs settling on a unit in that state before the installer is relied on for consumers.

**What a run needs:**
- the replacement's frozen review (stock installs its `image.bin`), and the signed manifest of the same build for the promotion;
- a `wellfed-stock` built from a clean commit (`--enable-ota-trial` refuses anything else);
- a broker address on the home network that the feeder can reach on TCP 1883 and 8088.

It holds the bench lease beside the stock config for its whole length.

**Not done yet:**
- The consumer image (no build Wi-Fi or update key), and its setup mode at first boot. Until then the replacement joins with its build's Wi-Fi, so on the bench the home network is the hotspot.
- Bluetooth setup as the pairing step; the CLI uses the LAN recovery window.
- Deleting `init` and `network` from `wellfed-stock`, which waits for the frame to work on hardware.

### Running an install on the bench

This needs the owner at the panel.

1. **Return the unit to stock** over the air. `wellfed-bench itest --only stock-roundtrip` runs both ways; for an install run, only its first half is wanted: `wellfed-bench ota --review <stock review>`. You should see stock 1.1.15 on bank 1 and the replacement in bank 0. If the stock review's `image.bin` has the pinned SHA-256 `05147476…`, it is the backup the run needs.
2. **Write the installer config:** `wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`. On the bench the hotspot is the home network, since the build joins it.
   - The redirect must be off (`network remove`, a host firewall change the owner makes), so stock reaches the broker only because the frame said so.
   - The bench's `server.json` is untouched; the record is named for the installer's config. The trial ledger beside both is the same feeder's, so resolve any trial a bench run left open first.
   - Joining stock's access point needs a Wi-Fi interface other than the one serving the hotspot.
3. **At the panel:** unlock stock (Lock about 5 s if the lock light is on), then hold Reset about 5 s. Note what stock does (sound, lights) and the access point's name. Join the bench host to it when `wellfed install` asks.
4. **Run** `wellfed install --stock-config private/installer.json --review <feeder review> --manifest <its signed manifest> --ssid <hotspot>`. It sends the frame and waits for stock to enroll and report. It shows the enrolled device ID and address for the owner to confirm against the feeder's label, asks for the install once, and waits for the replacement. It asks the owner for the recovery hold (Lock, then Dispense) and the code, reads and keeps the stock backup from bank 1 and stock's plan sectors, shows the plan import, promotes, waits for confirmation and sets the converted plans. The review must be a build that serves `ReadBank1`; with an older one, pass `--stock-backup <stock review>/image.bin`.
5. **Check and restore.** You should see the replacement confirmed on bank 1, and `private/installer.device.json` holding stock's own identity, with nothing captured used. Then remove `private/installer.json` and the files named for it (`installer.device.json`, `installer.contested.json`), have the owner restore the redirect if the bench needs it, and record the run.

## Rollback

The vendor's firmware can't ship with the app. But each feeder already holds its own copy of stock, until promotion overwrites bank 1.

**The main route: the installer saves the feeder's own stock.**

- **Where it is.** After stock installs the build into bank 0, bank 1 still holds the stock application, with only the first byte of its 8-byte marker zeroed. Stock's activation does exactly that and nothing more.
- **Who reads it.** The build on bank 0 can read bank 1 (`sdk::bank1_read`, also used for write readback).
- **Upload.** Before the installer offers the promotion, bank 0 streams bank 1 to it on the device API: `ReadBank1 { offset }`, answered by `Bank1Page`s of 768 bytes, owners only, served only from bank 0, and refused `Busy` while an install holds the flash ([device API](device-api.md#stock-backup)). The transport carries no trust; the hash check below is what shows the copy is intact.
- **Promotion waits for it.** Promotion overwrites bank 1, the feeder's only copy of stock. So the installer refuses to promote until a verified backup is stored. You cannot lose stock without holding a copy of it.
- **Rebuild.** The installer is the server during an install, so the backup lands in the app. It restores the marker byte and rebuilds the OTA container: a 32-byte header, the payload length and byte-sum, as in [the container format](../devices/plaf103/stock/ota-requirements.md#the-image-container).
- **Checks.** The rebuilt image must match the stock image profile. For 1.1.15 it must match the pinned payload hash exactly, so a damaged read shows before anything relies on it.
- **Storage** is per device. The CLI keeps it under the owner's data directory, `private/stock-backups/<identity>/stock-1.1.15.bin`, written once and never replaced by a different copy. The app, and your server if you want, will keep it the same way.
- **Export and restore** from the CLI:
  - `wellfed backup read` reads and keeps it from a replacement on bank 0, outside an install;
  - `wellfed backup show` prints its version, digest and path;
  - `wellfed backup export --output <file>` checks it again and copies it byte for byte to a new file;
  - `wellfed restore-stock --version <n>` is "Restore stock", the signed install below.

  After promotion the feeder holds no copy, so the backup comes from the owner's copy, never read back from the feeder.
- **Still to come:** the Android app and the server's web UI will show the backup (stock version, verified), export it as a file, copy it to the owner's server and offer "Restore stock", through the same `wellfed-client` steps.
- **Why not keep it on the feeder.** The only place it fits is bank 1. That means running from bank 0 for good, with no known-good fallback bank, and the first update into bank 1 would destroy it anyway. No other 1.3 MiB of flash is established as free.
- **Why this route.** These are your own bytes, from your own device, so there is no redistribution question.

**A second route: the vendor's update service.** The vendor app asks the cloud for a device's update metadata (private evidence E12). An installer could fetch the official package the same way with the user's own account. This depends on the vendor's service, and may offer only the latest version.

**Rolling back.** Our firmware installs a stock image as a signed update (`wellfed-bench ota --review <stock review>`). The bank-0 build writes it into bank 1. Stock's own KM0 then runs, so our trial counter never counts it.

`wellfed restore-stock` does the same from the owner's backup.
- `wellfed_signing::Frozen::stock` signs it as the stock image. Only the pinned container passes, as for a stock review.
- The downgrade statement is bound to the feeder's identity and the floor it reports.
- The push is the installer's own: bank 1 gives way, and bank 0 writes and activates stock.
- Nothing re-offers once the push has returned. (Within it, the shared installer offers again only where the update protocol needs it, as after bank 1 gives way.)
- A refusal ends the restore. A feeder that stops answering after the offer leaves the result **uncertain**, since it may already run stock, which serves no device API. That is reported as such, and never re-offered.

What a rollback keeps and leaves behind:

- **Stock's settings stay.** Our firmware never writes `0x3e8000`–`0x3f0000`, so stock comes back still provisioned. It points at the installer's broker if steps 1–3 were used; re-pairing with the vendor app returns it to the cloud.
- **Our storage stays.** `0x340000`–`0x370000` holds the journal, network credentials, the confirmation record and the lifetime statistics. Stock does not read it.
- **Bank 0 keeps our build** as a fallback, which the loader never chooses while stock in bank 1 is valid.
- **Stock's OTA status sector** (`0x3ef000`) may differ from the original after the install.
- **No automatic fallback.** If stock fails to boot from bank 1, recovery is UART only (`wellfed-bench stock-prepare` / `stock-execute`).

### Proving rollback on the bench

This needs the owner. It starts from a confirmed `feeder` on bank 1 and an older `feeder` on bank 0. The bench rules apply: one holder, the hotspot and `wellfed-stock serve` as bounded foreground processes, `network apply` already in place, and everything restored afterwards.

1. **Build:** `make -C src/firmware/vendor device firstboot FIRMWARE=feeder`, then `wellfed-build review build --build private/firmware-build/plaf103/firstboot --output private/verification/fw43-01-review` and `wellfed-build sign --review private/verification/fw43-01-review --version <next> --output private/verification/fw43-01-manifest.bin`, with the next manifest version.
2. **Go to stock:** `wellfed-bench ota --review <stock review>` from the confirmed build. You should see stock 1.1.15 on bank 1, and the older build on bank 0.
3. **Check the gate holds.** Plant a foreign file at `private/stock-backups/<identity>/stock-1.1.15.bin` (any bytes; the identity is the key file's 64 hex). Then run `wellfed install --stock-config private/server.json --review private/verification/fw43-01-review --manifest private/verification/fw43-01-manifest.bin`. Stock installs the build into bank 0, the run pairs (the bench key is already enrolled), and it stops at the backup step: you should see the kept file refused and nothing promoted. Check the feeder answers from bank 0 (`wellfed status`), then remove the planted file.
4. **Take the backup:** `wellfed install --from-bank0 --stock-config private/server.json --review … --manifest …`. It reads bank 1 (1,792 pages), keeps it, promotes and confirms. Check that `sha256sum private/stock-backups/<identity>/stock-1.1.15.bin` prints `05147476…` (so its payload is `802ef620…`), and that `cmp` finds the file identical to the stock review's `image.bin`.
5. **Export:** `wellfed backup export --output private/fw43-export.bin`, then `cmp` it with the kept file.
6. **Roll back.** With `wellfed-stock serve` running and a UART capture (`wellfed-flash read observe`), run `wellfed restore-stock --version <next+1>`. Bank 1 gives way, bank 0 writes stock, and the restore reports activated. You should see stock's `app_main:version:` banner from bank 1 on the UART, and stock connecting to the broker it was provisioned to with no new setup frame (`/api/status`: online, `softwareVersion` 1.1.15).
7. **Go back and restore:** `wellfed-bench stock-ota` with a `feeder` review, back to the replacement confirmed on bank 1. Stop `serve` and the hotspot as before, and record the run: versions, digests and the kept file's path.

## Where the installer lives

The decisions are in [product architecture](product-architecture.md). For the installer they mean:

- **The steps live in `wellfed-client`:** the setup frame, the single stock request, the progress tracking, the backup gate and the backup receiver. The Android app, `wellfed-server` and the CLI run the same steps the bench proves. Each caller supplies the IO: the image server, the broker and the sockets.
- **The Android app** joins the feeder's access point through platform Wi-Fi APIs, and serves the image itself, in the foreground, for about 3 minutes.
- **The server and the CLI** can run the same install for owners who self-host. `wellfed-stock` stays the bench's stock-protocol tool.
- **Afterwards,** the feeder is handed to the pairing flow, not to a server. The app talks to it directly on the LAN, and a server is optional.

## Evidence

- **Observed** (October 23, 2026, research unit):
  - `fw40-stock-ota-05`: stock 1.1.15 installed a replacement build through its own MQTT-triggered HTTP updater, with no UART write. It downloaded the image in about 40 s, checked its byte-sum and wrote it to bank 0. The loader then booted the build from bank 0.
  - `fw40-stock-ota-06-promote`: the build on bank 0 installed the same build, signed, into bank 1, where it confirmed. Promotion took about 3.5 min with the updater of the time.
  - `fw40-stock-ota-07`: the whole sequence in one command, with a fresh build: install, health from bank 0, and promotion to bank 1.
  - `fw41-roundtrip-01`: to stock over our updater and back over stock's, with no UART write and nobody at the bench.
  - The bench requirements came from earlier tries: `fw40-stock-ota-02` (the image port blocked, error 2003), `fw40-stock-ota-03` (the 60 s telemetry bound) and `fw40-stock-ota-04` (the stale version report).
- **Observed:** stock 1.1.15 dispensing locally through `wellfed-stock`. Stock's client ID equals the device ID in its topics, from a captured CONNECT.
- **Recovered:** stock's setup protocol, frame and re-provisioning guard, from the stock image and the vendor app (not executed). The reset gesture is recovered, not verified.
- **Modeled** (October 26): the installer, trust on first use, the setup-frame encoder, the backup gate, the stock backup read and `wellfed restore-stock`, host-tested and simulated, on synthetic bytes. Nothing has run on the feeder yet.
