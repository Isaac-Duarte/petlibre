# Consumer installation and rollback

How a feeder on **stock 1.1.15** could move to the replacement firmware with no tools beyond an installer app, and how it could go back. The goal is an experience like the vendor app's, with a self-hosted server in place of the cloud.

Each finding is labeled **physical** (observed on the research unit), **static** (from the stock image or the vendor app, not executed) or **design** (not built). The evidence index for the static findings is private: `private/research/stock-installer/evidence.md`. No vendor code, credentials or device identifiers appear here.

## What is already shown

**Physical (October 23, 2026, research unit):**
- **`fw40-stock-ota-05`:** stock 1.1.15 installed a replacement build through its own MQTT-triggered HTTP updater, with no UART write. It downloaded the image in about 40 s, checked its byte-sum and wrote it to bank 0. The loader then booted the build from bank 0.
- **`fw40-stock-ota-06-promote`:** the build's bank-0 recovery role installed the same build, signed, into bank 1, where it confirmed.
- **`fw40-stock-ota-07`:** the whole sequence in one command, with a fresh build: install, recovery-role health from bank 0 (the MQTT era's), and promotion to bank 1.
- **`fw41-roundtrip-01`:** to stock over our updater and back over stock's, with no UART write and nobody at the bench.
- **Tool:** `wellfed-bench stock-ota` runs this whole sequence ([bench](../devices/plaf103/bench.md#stock-to-a-replacement-stock-ota)). The image contract is in [stock OTA requirements](../devices/plaf103/stock/ota-requirements.md).

What made those runs work on the bench, and must stay in any installer:

| Requirement | Why |
| --- | --- |
| One `OTA_UPGRADE`, never resent | A refused precheck can erase stock's OTA status sector; an uncertain request may still be downloading |
| Resolve a failed request before the next one | The server's ledger refuses a new trial while one is failed or uncertain; it resolves from fresh evidence: 20 min, 10 quiet connected minutes, a fresh version report |
| The image port reachable from the feeder | `fw40-stock-ota-02`: with only TCP 1883 allowed, stock connected for about 25 s, then reported `OTA_INFORM` error 2003 |
| Tolerate stock's ~61 s heartbeat | `fw40-stock-ota-03`: a 60 s telemetry bound restarted the quiet interval at every heartbeat |
| Ask stock for its attributes when its version report is stale | Idle stock reports its version only around a connect (`fw40-stock-ota-04`) |
| A URL under 128 bytes, `Content-Length` title-cased, a direct `200` | Stock's parser limits ([requirements](../devices/plaf103/stock/ota-requirements.md#http-server-and-url-requirements)) |
| Mains power, idle, model and version checked first | Stock's own precheck; the server's trial gate checks the same reports |

What the bench still uses and a consumer must not need:
- `wellfed-stock network apply`: a root firewall redirect of the feeder's TCP 1883, plus WAN blocking;
- device MQTT credentials captured from traffic (`wellfed-stock init`);
- a build carrying the owner's Wi-Fi credentials, broker credentials and update key.

The setup protocol below removes the first two (host-tested in [the installer](#the-installer); not yet run on stock). Generic images remove the third.

## Stock's setup protocol (static)

The vendor app provisions a PLAF103 through the feeder's **setup access point**, not over Bluetooth:
1. The feeder, in setup mode, runs its own Wi-Fi access point.
2. The app joins it and sends one UDP datagram to **192.168.1.1:17484** (`0x444C`). Stock's `Ap_Config_Udp_Thread` receives it on that port.
3. BLE service `ffee` is connected alongside (`ffe1` write, `ffe2` data, `ffe3` read, `ffe4` notify), but the frame itself goes over UDP.

**The frame.** PLAF103 is on the app's list of products sent without encryption, so the frame is plaintext with no checksum:

```text
'D' 'L' | type=3 | checksum flag=0 | 0 | payload length | lp(SSID) lp(password) lp("host:PPPP") lp(numeric id) | 0xFF
```

- **`lp(x)`** is one length byte, then the UTF-8 bytes.
- **Types:**
  - type 1: Wi-Fi only;
  - type 2: server only;
  - type 3: both (what the app sends for PLAF103);
  - type 4: three other strings of up to 50 bytes, meaning unknown.
- **Field limits:**
  - SSID: up to 32 bytes;
  - password: up to 64 bytes;
  - `host:PPPP`: up to 100 bytes;
  - numeric id: up to 30 bytes, ASCII digits only.
- **`host:PPPP`** is the MQTT broker, which the app reads from its cloud configuration. Stock takes the host before the `:` and up to four port digits after it, so 1883 fits.
- **The numeric id** is most likely the account's user id. It must be digits and nonzero.

**What stock does with it** (stock functions at runtime addresses):
- **Parsing:** `0e01a160` checks the `DL` prefix and the `0xFF` terminator, then dispatches on the type.
- **Wi-Fi:** stored by `0e007bb0`.
- **Server:** `0e007cf8` (`Set_Dev_Info`) stores the port, numeric id and host in a 0x484-byte record.
- **Applying it:** the setup thread `0e0084d8` sets the MQTT port, host and id. It then polls whether MQTT came up (`0e012780`, up to 20 tries). Only then does it persist the record to flash `0x3e8000`, and a 0x60-byte record to `0x3e9000`.
- **At every boot:** stock loads `0x3e8000` and connects to that host and port.

**Re-provisioning guard.** While the `0x3e9000` record is non-blank and the device state is bound, types 1–3 are acknowledged but not applied. A feeder must be reset or unbound first.

**Credentials.** The MQTT client ID, username and password the feeder presents are its own: they are loaded from flash at boot, not issued at bind. `BINDING` is an application-level association, which `wellfed-stock` already answers by replaying the captured cloud reply. The commands stock dispatches over MQTT are `OTA_UPGRADE`, `GET_CONFIG`, `BINDING`, `UNBIND`, `DEVICE_REBOOT`, `WIFI_RECONNECT_SERVICE`, `WIFI_CHANGE_SERVICE`, `ATTR_SET_SERVICE` and `ATTR_GET_SERVICE` ([stock behavior](../devices/plaf103/stock/README.md)).

**Open questions (to settle physically):**
- Is type 3 really what goes on the wire? Reading the decompiled app depends on its integer encoding, so a capture should confirm it.
- What exactly is the numeric id?
- How is a feeder reset or unbound? Is there a button hold? What does `UNBIND` (`0e015fd4`, not decompiled) clear?
- Are the credentials fixed at the factory, or derived from the serial? Either way they arrive at our broker unchanged.

**Getting into setup mode (static, unverified):** stock's panel has one reset gesture: Reset (the rightmost button) held about 5 s on an unlocked panel starts the factory restore, and no Wi-Fi-only hold exists ([stock behavior](../devices/plaf103/stock/README.md#panel-buttons-and-child-lock)). The panel is unlocked by holding Lock about 5 s while the lock light is on. That the restore leaves stock in setup mode with its access point up, and clears the bound state the re-provisioning guard checks, is the first thing the physical test must show; the access point's name is not established either.

**Planned physical test:** with stock on the feeder and in setup mode, the bench host joins its access point. It then sends one type-3 frame with `wellfed-stock setup-send --ssid <hotspot> --id <digits>`, which reads the passphrase from stdin and names this server's broker (`10.42.103.1:1883`) unless `--broker` is given. The frame encoder (`wellfed_client::install::frame`) is host-tested against the parser's length rule and field limits. If stock then joins the hotspot and connects to `wellfed-stock serve` with its own credentials, the redirect and the captured credentials are no longer needed.

## What a self-hosted server must answer (static, partly physical)

`wellfed-stock` already implements the stock side (physical: stock 1.1.15 local dispensing):
- a broker on TCP 1883 that accepts stock's legacy MQTT 3.1 CONNECT;
- the `dl/PLAF103/<device-id>/…/post` and `/sub` topics;
- replies to `NTP`, `BINDING` and `GET_FEEDING_PLAN_EVENT`;
- acknowledgements of the startup, attribute, log and error events.

For a consumer it must also accept a device whose credentials it has never seen. **Trust on first use (host-tested, October 26):** an installer's config (`wellfed-stock init-installer`) names no device, and `serve --enroll-minutes N` admits the first stock-shaped CONNECT within the window, records its own client ID, username, password and address beside the config (`installer.json`'s record is `installer.device.json`), and pins it; nobody else logs in, then or later ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use)). Stock's client ID is its device ID (physical: the captured CONNECT's client ID equals its topics' device ID). The captured-credential `init` and the `network` redirect stay for the bench until the frame has provisioned stock on hardware.

## The install flow (design)

1. **Setup mode:** the user resets the feeder into its setup access point, the same step as "add device" in the vendor app.
2. **Provision:** the installer joins that access point and sends one type-3 frame: the home network, plus the installer's own `host:1883`.
3. **Connect:** stock joins the home network and connects to the installer's broker with its own credentials, which the installer records.
4. **Pre-flight:** model PLAF103, version 1.1.15 on an allow-list, mains power, idle. These are stock's own precheck conditions; the bench's `stock-ready` stage checks them.
5. **Install:** the installer serves the image and sends one `OTA_UPGRADE`. Stock's progress reports drive a progress bar, about 40 s.
6. **First boot:** the build boots from bank 0 and answers the device API while it waits for an update (until October 25 its recovery role reported MQTT health instead).
7. **Backup:** the build streams bank 1, which still holds stock, to the installer, which verifies it (see rollback below).
8. **Promote:** only once a verified backup is stored, the installer offers the same build signed over the device API. Bank 0 installs it into bank 1, and its trial boot confirms; about 3.5 min on the bench with the MQTT updater.
9. **Hand-off:** the confirmed replacement enters the pairing flow (FW45): the owner's app pairs as its first client, and a server joins later by invite if the owner runs one.

Steps 1–3 replace the bench's firewall redirect and captured credentials. Steps 5, 6 and 8 are what ran physically. Pairing comes before the backup and the promotion in practice: a replacement fresh from stock knows no client key, and both go over the device API.

## The installer

**Host-tested, October 26 (simulation; nothing run on the feeder).** One run, `wellfed install`, goes from stock to a confirmed replacement. Its steps are a sans-IO state machine in [`wellfed_client::install`](../../src/client/wellfed-client/src/install.rs), which the Android app and `wellfed-server` will drive the same way; the CLI supplies the IO.

| Step | Decided in `wellfed-client` | IO in the CLI |
| --- | --- | --- |
| 1. Provision | The type-3 frame, refused before anything is sent if stock would misread a field; sent once | The owner puts stock into setup mode and joins this computer to its access point (platform Wi-Fi); one UDP datagram to 192.168.1.1:17484; the owner rejoins the home network |
| 2. Stock ready | Online, 1.1.15 (any other version ends the run), on its adapter | `wellfed-stock serve --enable-ota-trial --enroll-minutes N` as a child: the broker trusts the first feeder (above); its status document, polled |
| 3. Confirm the feeder | The owner checks the enrolled device ID and address before anything is sent; a contested enrollment ends the run | The status document's `enrollment` (never its credentials) and a `[y/N]` prompt. A bench config with a captured device needs no check |
| 4. Request | Exactly one `OTA_UPGRADE` per run; refused or uncertain ends the run | `ota-images` as a child, then `POST /api/ota/trial`. What the server recorded is read back: refused only when the server refused and a successful ledger read shows no record, uncertain otherwise. The server itself refuses (409) while the enrollment is contested |
| 5. Install | Progress; stock's refusal ends the run (even before the request's outcome is read); stock going quiet means it restarted | The trial phase, and the OTA reports since the request that name it (or nothing) |
| 6. Pair | The installer must be an owner before anything else | A key the build already knows opens a session at once; otherwise the owner holds Lock, then Dispense, on the unlocked panel and keys in the code (`wellfed recover`'s exchange). The address is where stock connected from |
| 7. Bank 0 | Only the served build moves on: on bank 0 the stock server's record is then closed; on bank 1 (a run resumed after activation) only the confirmation is left | `Status` over the device API; `POST /api/ota/trial/answered`, asked again while stock's telemetry is still current |
| 8. Backup | **No promotion without a `VerifiedBackup`** | `--stock-backup`: read and checked before the feeder is touched |
| 9. Promote | `update::Installer`, the same push as the bench's. Its error ends the run; once writing began, bank 1 may be partly overwritten, so the verified backup is then the copy of stock | Chunks over the device API until bank 1 is activated; a dropped session is reopened and the install resumes where bank 0 says |
| 10. Confirm | Done only when bank 1 runs the build and reports the manifest's version confirmed; bank 0 means it fell back | `Status` and `UpdateState` after the restart |

**Trust on first use, and its risk.** The broker trusts whichever stock-shaped client reaches it first inside the window. A host on the same network that wins that race (or a second feeder) would be enrolled instead, and without a check it could be walked through the whole run to a false "Installed" while the real feeder stays on stock. So the run stops for the owner to confirm the enrolled device ID (on the feeder's label or in the vendor app) and its address (the router's client list) before the install request, and any second stock-shaped identity inside the window, or a wrong password for the enrolled one, marks the enrollment contested (`installer.contested.json` beside the config): the status shows it, the server refuses to install, and the run ends. A contest counts only inside the window: once it has closed, a second identity (even the real feeder arriving late, after a stranger was enrolled) is refused like any wrong password but not flagged, and the owner's confirmation of the device ID and address is the only check left. A contested enrollment is cleared by removing both files and running again on a network only the feeder joins. The device API's pairing (the setup code keyed in on the feeder's own panel) is the second check: a false feeder cannot complete it.

**The backup seam.** `VerifiedBackup::verify` is the only way to make the token the promotion waits for. It takes either stock 1.1.15's bare application payload, whose SHA-256 is the pinned payload's (`802ef620…`, the stock review's `payload_sha256`), or the vendor's exact container (`05147476…`). The payload is pinned because the bank holds the payload, and a factory-flashed unit may never have been written from that exact container. A container is never taken apart: any other header over the right payload is refused, since its fields are not checked and a restore would write it as given. Until the backup stream exists, `--stock-backup` takes a copy the owner already holds (on the bench, the stock review's `image.bin`); the stream will produce the payload and pass the same check. Without one, the run stops at the gate with the replacement on bank 0 and stock intact in bank 1, and `wellfed install --from-bank0 --stock-backup …` resumes from pairing without asking stock anything. The same resume finishes a run that stopped after activation: it finds the build on bank 1 and waits for its confirmation.

**Open question: which bank stock runs from.** Every install so far started from stock in bank 1, so stock wrote bank 0 and promotion overwrote bank 1. A consumer unit whose stock ran from bank 0 (after a vendor update, say) would have stock write the replacement into bank 1, leaving its own copy in bank 0; the steps above, the backup source and the promotion all assume the research unit's layout and refuse anything else (the served build must answer from bank 0). Settle on a unit in that state before relying on the installer for consumers.

**What the run needs:** the replacement's frozen review (stock installs its `image.bin`) and the signed manifest of the same build for the promotion; a `wellfed-stock` built from a clean commit (`--enable-ota-trial` refuses anything else); a broker address on the home network that the feeder can reach on TCP 1883 and 8088. It holds the bench lease beside the stock config for its whole length.

**Not done:** the consumer image (no build Wi-Fi or update key) and its setup mode at first boot; until then the replacement joins with its build's Wi-Fi, so on the bench the home network is the hotspot. BLE setup as the pairing step (the CLI uses the LAN recovery window). The backup stream. Deleting `init` and `network` from `wellfed-stock`, which waits for the frame to work on hardware.

### Physical procedure (planned, needs the owner)

1. **Return the unit to stock** over the air: `wellfed-bench itest --only stock-roundtrip` runs both ways; for an install run, only its first half is wanted (`wellfed-bench ota --review <stock review>`, FW41), leaving stock 1.1.15 on bank 1 and the replacement in bank 0. The stock review's `image.bin`, if its SHA-256 is the pinned `05147476…`, is the backup the run needs.
2. **Installer config:** `wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json` (the hotspot is the home network on the bench, since the build joins it). The redirect must be off (`network remove`, a host firewall change the owner makes), so stock reaches the broker only because the frame said so. The bench's `server.json` is untouched (the record is named for the installer's config); the trial ledger beside both is the same feeder's, so a trial a bench run left open must be resolved first. Joining stock's access point needs a Wi-Fi interface other than the one serving the hotspot.
3. **Owner at the panel:** unlock stock (Lock about 5 s if the lock light is on), hold Reset about 5 s; note what stock does (sound, lights) and the access point's name. Join the bench host to it when `wellfed install` asks.
4. **Run:** `wellfed install --stock-config private/installer.json --review <qa review> --manifest <its signed manifest> --stock-backup <stock review>/image.bin --ssid <hotspot>`. It sends the frame, waits for stock to enroll and report, shows the enrolled device ID and address for the owner to confirm against the feeder's label, asks it once, waits for the replacement, asks the owner for the recovery hold (Lock, then Dispense) and the code, promotes, and waits for confirmation.
5. **Check and restore:** the replacement confirmed on bank 1; `private/installer.device.json` holds stock's own identity and nothing captured was used; then remove `private/installer.json` and the files named for it (`installer.device.json`, `installer.contested.json`), have the owner restore the redirect if the bench needs it, and record the run.

## Rollback

The vendor's firmware cannot be shipped with the app. Each feeder already holds its own copy, though, until the promotion overwrites bank 1.

**Primary: the installer saves the feeder's own stock (design).**
- **Where it is:** after stock installs the build into bank 0, bank 1 still holds the stock application, with only the first byte of its 8-byte marker zeroed. Stock's activation does exactly that and nothing more.
- **Who reads it:** the build on bank 0 can read bank 1 (`sdk::bank1_read`, used today for write readback).
- **Upload:** before the installer offers the promotion, bank 0 streams bank 1 to it as a chunked response on the device API (FW47), the same Noise stream the app uses for everything else, not over MQTT. The transport carries no trust: the hash check below is what shows the copy is intact.
- **Promotion waits for it:** promotion overwrites bank 1, the feeder's only copy of stock, so the installer refuses to promote until a verified backup is stored. An owner cannot lose stock without holding a copy of it.
- **Rebuild:** the installer is the server during an install, so the backup lands in the app. It restores the marker byte and rebuilds the OTA container: 32-byte header, payload length and byte-sum, as in [the container format](../devices/plaf103/stock/ota-requirements.md#the-image-container).
- **Checks:** the rebuilt image must match the stock image profile. For 1.1.15 it must match the pinned payload hash exactly, so a mismatch shows a damaged read before anything relies on it.
- **Storage:** per device, in the app and, if the user wants, on their server.
- **Export and restore:** the app and the server's web UI show the backup (stock version, verified), export it unchanged as a file, and offer "Restore stock", the signed install below. After promotion the feeder holds no copy, so the backup is served from the app or server, never read back from the feeder.
- **Why not keep it on the feeder:** the only place it fits is bank 1, which means running from bank 0 for good, with no known-good fallback bank (FW11), and the first update into bank 1 would destroy it anyway. No other 1.3 MiB of flash is established as free.
- **Why this route:** these are the user's own bytes from their own device, so there is no redistribution question.

**Secondary: the vendor's update service (static).** The vendor app asks the cloud for a device's update metadata (private evidence E12). An installer could fetch the official package the same way with the user's own account. This depends on the vendor's service, and may offer only the latest version.

**Rolling back.** Our firmware installs a stock image as a signed update (FW41: `wellfed-bench ota --review <stock review>`, physical in `fw41-roundtrip-01`). The bank-0 role writes it into bank 1. Stock's own KM0 then runs, so our trial counter never counts it.

What a rollback keeps and leaves behind:
- **Stock's settings stay:** `0x3e8000`–`0x3f0000` are never written by our firmware, so stock comes back still provisioned. It points at the installer's broker if steps 1–3 were used; re-pairing with the vendor app returns it to the cloud.
- **Our storage stays:** `0x340000`–`0x370000` holds the journal, network credentials, the confirm record and the lifetime statistics. Stock does not read it.
- **Bank 0 keeps our build** as a fallback, which the loader never chooses while stock in bank 1 is valid.
- **Stock's OTA status sector** (`0x3ef000`) may differ from the original after the install.
- **No automatic fallback:** if stock fails to boot from bank 1, recovery is UART only (`wellfed-bench stock-prepare` / `stock-execute`).

## Where the installer lives (design)

The decisions are in [product architecture](product-architecture.md); for the installer they mean:
- **Installer steps:** the setup frame, the single stock request, the progress tracking and the backup gate live in the sans-IO `wellfed-client` crate ([the installer](#the-installer); the backup receiver comes with the stream), so the Android app, `wellfed-server` and the CLI run the same steps the bench now proves. Each caller supplies the IO: the image server, the broker and the sockets.
- **Android app (Flutter):** it joins the feeder's access point through platform Wi-Fi APIs and serves the image itself, in the foreground for about 3 minutes.
- **Server and CLI:** `wellfed-server` or the CLI can run the same install for owners who self-host. `wellfed-stock` stays the bench's stock-protocol tool.
- **Afterwards:** the feeder is handed to the pairing flow (FW45), not to a server. The app talks to it directly on the LAN, and a server is optional.
