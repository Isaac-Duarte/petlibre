# Consumer installation and rollback

This page explains how a feeder on **stock 1.1.15** can move to Wellfed with nothing but an installer app, and how it can go back. It is for owners wondering what installing involves, and for contributors working on the installer. The goal is an experience like the vendor app's, with your own server in place of the cloud.

No vendor code, credentials or device identifiers appear here. The evidence index for the static findings is private: `private/research/stock-installer/evidence.md`.

## The one idea

Stock's own updater does the hard part. Stock 1.1.15 will install any image we serve it into its other bank, with no UART and no soldering. So the installer only has to get stock talking to it, ask for one update, and serve the image. You hold Reset, which erases stock's settings and meals, and your phone hands the feeder your Wi-Fi over the feeder's own setup network. Before Wellfed overwrites stock's last copy, the installer saves the feeder's own stock image, so you can always go back.

| Step | Status |
| --- | --- |
| Stock installs Wellfed over its updater | Works |
| Promotion to bank 1 and confirmation | Works |
| Going back to stock over the air | Works, using a stock image we serve |
| The Reset hold and the setup frame, from the app | Works: run once on the research unit, from the phone |
| The guided installer, `wellfed install` | Experimental |
| Saving and restoring the feeder's own stock image, from the CLI and the app | Experimental: simulation only, not yet run on the research unit |

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

Stock's setup protocol removes the first two. Generic images remove the third.

## Stock's setup protocol

This is the app's main route. The vendor app provisions a PLAF103 through the feeder's **setup access point**, not over Bluetooth:

1. The feeder, in setup mode, runs its own open Wi-Fi access point, `PL-AF103-` and the last two bytes of its MAC, on channel 11 (observed).
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

**Saving it takes an answer.** The setup thread's wait is for the state stock also requires before an install, and that state clears when `BINDING` is answered. With no answer, stock joined the Wi-Fi and reached the broker, then refused the install with error 1204 ("device not bound") and, about 20 s after it arrived, went back to its setup network with nothing saved (observed). So the broker must be listening before the frame goes, and reachable within that time. An installer's server has no captured reply, so it answers with `code` 0, a nonzero `bindId` and an empty `cameraAuthInfo`. Stock accepts that, saves the frame, and takes an install (observed). What the vendor's `bindId` means is not established.

**Re-provisioning guard.** While the `0x3e9000` record is non-blank and the device state is bound, types 1–3 are acknowledged but not applied. A feeder must be reset or unbound first.

**Credentials.** The MQTT client ID, username and password the feeder presents are its own: they are loaded from flash at boot, not issued at bind. `BINDING` is an application-level association, which `wellfed-stock` already answers by replaying the captured cloud reply. The commands stock dispatches over MQTT are `OTA_UPGRADE`, `GET_CONFIG`, `BINDING`, `UNBIND`, `DEVICE_REBOOT`, `WIFI_RECONNECT_SERVICE`, `WIFI_CHANGE_SERVICE`, `ATTR_SET_SERVICE` and `ATTR_GET_SERVICE` ([stock behavior](../devices/plaf103/stock/README.md)).

**Getting into setup mode** costs stock's plans. A set-up stock never opens its setup access point on its own: when it cannot reach its Wi-Fi, it retries forever. The only way in is the Reset hold: Reset (the rightmost button) held about 5 s on an unlocked panel. That runs stock's full factory restore. It erases its broker, Wi-Fi and binding records, its feeding plans and their record rings, then restarts ([what the restore clears](../devices/plaf103/stock/README.md#reset-and-the-cloud)). The feeder then says it is ready for a Wi-Fi connection and opens its setup network (observed). The plan sectors read back erased (observed). The panel is unlocked by holding Lock about 5 s while the lock light is on.

**On a phone.** The app asks Android for a network whose name starts with `PL-AF103-`, and Android shows its own picker. Once joined, the process's sockets go over that network until the frame is out. A phone that can hold a local-only network alongside its Wi-Fi stays on both, as the research unit's Pixel did (observed). One that cannot leaves home for those seconds and has to be back within stock's wait, which is modeled but not run. It comes back at the address the frame named, or the run stops: the feeder would look in the wrong place. The password goes unencrypted over the setup network, so the screen says so.

**If the feeder does not arrive.** Two minutes after the frame with no feeder at the broker, the run stops with nothing asked of stock. A wrong password or a Wi-Fi with no 2.4 GHz band ends this way. Stock then keeps nothing and reopens its setup network, so a new frame needs no second Reset hold (static analysis; seen only for an unanswered `BINDING`, not yet for a wrong password). That is a new frame to a feeder visibly still in setup mode, unlike the install request, which is never resent. Once stock has reached the broker it has kept the frame and has no setup network left, so a later run only waits for it.

**Still open:**
- What exactly is the numeric id? Stock checks only its form, and repeats it in `BINDING`.
- What does `UNBIND` (`0e015fd4`, not decompiled) clear?
- Are the credentials fixed at the factory, or derived from the serial? Either way they arrive at our broker unchanged, and they survive the Reset hold (observed).

## What the installer's server must answer

`wellfed-stock` already implements the stock side:

- a broker on TCP 1883 that accepts stock's legacy MQTT 3.1 CONNECT;
- the `dl/PLAF103/<device-id>/…/post` and `/sub` topics;
- replies to `NTP` and to `BINDING` (a captured reply if there is one, otherwise a generic one, which a stock fresh from the setup frame needs before it saves the frame or takes an install), and to `GET_FEEDING_PLAN_EVENT` only with a captured plan snapshot. An installer's config has none, so that request goes unanswered: stock erases its plan table on any reply with a `plans` key, and a rollback must find the owner's plans intact ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use));
- acknowledgements of the startup, attribute, log and error events.

For a consumer it must also accept a device whose credentials it has never seen. **Trust on first use:** an installer's config (`wellfed-stock init-installer`) names no device. `serve --enroll-minutes N` admits the first stock-shaped CONNECT within the window. It records that client's ID, username, password and address beside the config (`installer.json`'s record is `installer.device.json`) and pins it, so nobody else logs in, then or later ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use)). Stock's client ID is its device ID. The captured-credential `init` and the `network` redirect stay for the bench.

## The install flow

1. **Reset:** you hold Reset, and the feeder opens its setup access point (which erases its plans).
2. **Frame:** the installer sends one type-3 frame there with your home network and its own `host:1883`.
3. **Connect:** stock joins your network and connects to the installer's broker with its own credentials, which the installer records.
4. **Pre-flight:** model PLAF103, version 1.1.15 on an allow-list, mains power, idle. These are stock's own precheck conditions; the bench's `stock-ready` stage checks them.
5. **Install:** the installer serves the image and sends one `OTA_UPGRADE`. Stock's progress reports drive a progress bar, for about 40 s.
6. **First boot:** the build boots from bank 0 and answers the device API while it waits for an update. A consumer image has no network yet, so it starts BLE setup mode: the installer pairs and sends your home Wi-Fi, and the build reboots and joins it.
7. **Pair:** the next steps go over the device API, which needs a client key. With a consumer image, the Bluetooth setup in step 6 is that pairing: the installer becomes the first owner. A bench image joins its build network instead, knows no client key, and the CLI pairs through the LAN recovery window.
8. **Backup:** the build streams bank 1, which still holds stock, to the installer, which verifies it ([rollback](#rollback)).
9. **Promote:** only once a verified backup is stored, the installer offers the same build, signed, over the device API. A consumer image trusts only the release key and keys its owner enrolled, so the installer first enrolls its own update key (you press Dispense), then signs the build with it above the feeder's floor. Bank 0 installs it into bank 1, and its trial boot confirms.
10. **Hand-off:** the confirmed build enters the [pairing](pairing.md) flow. Your app is its first client, and a server joins later by invite if you run one.

Steps 1–3 replace the bench's firewall redirect and captured credentials. The replacement starts with no meals: the installer does not read stock's plans, and you set meals up in Wellfed afterwards.

After the setup frame, stock's clock is set whatever it reads, since the Reset hold has erased its plans and no meal can come due in the jump. A run with no frame, which reached stock some other way, leaves the clock as it reads, since stock may still hold plans. Stock with its clock two days behind refused an install with error 1214 (observed once; the clock as the cause is likely). The run then stops, naming the error.

## The installer

One run, `wellfed install`, goes from stock to a confirmed replacement. It is built in three layers:

- **The steps** are a sans-IO state machine in [`wellfed_client::install`](../../src/client/wellfed-client/src/install.rs). Every decision between steps is made there.
- **The driver** is [`wellfed-install`](../../src/client/wellfed-install), a tokio crate that does the IO each step asks for: stock's server, the request, pairing, the backup, the promotion, and the record a resumed run reads. The CLI and the Android app link the same driver.
- **The platform** supplies what only it owns, through the driver's traits: the person at the feeder (prompts), the Bluetooth radio, finding a feeder on the network, the client key, and how progress is shown. The CLI uses a terminal (or the bench's JSON lines), BlueZ, mDNS and its key file, and prints each progress event as a line. The app uses its screens, Android's Bluetooth and NSD, and its secure storage.

Stock's side runs in the driver's own process, since an app cannot spawn the `wellfed-stock` binary. The broker, the server's session with stock and the image server are tasks the run starts and stops ([stock server](../development/stock-server.md#how-the-crate-is-laid-out)). The CLI also serves the server's loopback API, for the bench's page.

| Step | Decided in `wellfed-client` | IO in `wellfed-install` (and the CLI's flags) |
| --- | --- | --- |
| 1. Provision | The type-3 frame, refused before anything is sent if stock would misread a field; sent once | `--ssid`: the owner is told the Reset hold erases stock's plans, puts stock into setup mode and joins this computer to its access point (platform Wi-Fi); one UDP datagram to 192.168.1.1:17484; the owner rejoins the home network |
| 2. Stock ready | Online, 1.1.15 (any other version ends the run), on its adapter | Stock's server in this process, with the trial enabled and an enrollment window of `--enroll-minutes`: the broker trusts the first feeder, and holds that feeder's CONNACK until the server has subscribed, so stock's startup reports reach it. Its status, polled. If stock still reports no version after 90 s, the owner restarts it once to replay them |
| 3. Confirm the feeder | The owner checks the enrolled device ID and address before anything is sent; a contested enrollment ends the run | The status document's `enrollment` (never its credentials) and a `[y/N]` prompt. With `--expect-device`, the owner's answer given ahead: the pinned device passes without a prompt, and any other ends the run naming both. A bench config with a captured device needs no check |
| 4. Request | Exactly one `OTA_UPGRADE` per run; refused or uncertain ends the run | The image server in this process, serving the review to the enrolled feeder's address, then the trial request. What the server recorded is read back: refused only when the server refused and a successful ledger read shows no record, uncertain otherwise (an unanswered request included). The server itself refuses while the enrollment is contested |
| 5. Install | Progress; stock's refusal ends the run, even before the request's outcome is read; stock going quiet means it restarted | The trial phase, and the OTA reports since the request that name it (or nothing) |
| 6. Pair | The installer must be an owner before anything else. The route follows the review's provisioning (`PairRoute`): a consumer image by Bluetooth setup, then a wait for its join; a bench image by the recovery window | A key the build already knows opens a session at once. A run whose broker enrolled the same stock device as an earlier run reads that run's record (`installing.json` under `--backups`): it looks for the feeder that run paired with where it last answered, where stock connected from and over mDNS, and stops naming `--address` if it does not answer. Otherwise the key file's feeder is tried only where stock connected from (or `--address`), since the key file may know another feeder, and setup follows if it does not answer. The run that finishes removes its record. A consumer image: `wellfed setup`'s exchange over BlueZ (`--ble-address`, or the first feeder advertising setup) with the home network (`--home-network`'s file; or `--home-ssid`, or the frame's `--ssid`, the passphrase read once), the code shown to key in; then the feeder is looked for over mDNS by the identity it proved, and where stock connected from, until a session opens. A bench image: the owner holds Lock, then Dispense, on the unlocked panel and keys in the code (`wellfed recover`'s exchange), at the address stock connected from |
| 7. Bank 0 | Only the served build moves on. On bank 0 the stock server's record is then closed; on bank 1 (a run resumed after activation) only the confirmation is left, and a run that signs for itself must know the version it promoted | `Status` over the device API; the server's record closed on that answer, asked again while stock's telemetry is still current. A resumed run reads the version from the manifest kept beside the backup |
| 8. Update key | Only when the image does not build the signer's key in. `update_key::Enroll`: a key already listed as an update signer is skipped; the enrollment goes once; a refusal ends the run before promotion; an unanswered one is judged by the client list (the key listed as a signer moves on, anything else ends the run), never sent again | `--update-key` (created as `wellfed update-key new` would if missing; a bench review signs with the bench key, which its image builds in). With `--manifest` no key is enrolled, since the manifest is promoted as signed, and `--update-key` beside it is refused. `ListClients`, then `EnrollUpdateKey` with the owner pressing Dispense within 60 s, then `ListClients` again if it went unanswered |
| 9. Backup | **No promotion without a `VerifiedBackup`.** `Bank1Reader` reads bank 1 a page at a time, and `StockBackup::from_bank` restores the marker's first word, checks the payload against the pin and rebuilds the vendor's container | `ReadBank1` over the device API (owners only, bank 0 only), the copy kept under `--backups` (`private/stock-backups/<identity>/stock-1.1.15.bin`), or one kept by an earlier run. `--stock-backup` instead takes a copy the owner holds, checked before the feeder is touched |
| 10. Version | The floor (`UpdateState`'s highest confirmed version) decides. A given manifest at or below it is refused (it would need a downgrade statement); a run that signs for itself picks one above the floor and above every version it signed before | `--manifest`, or the reviewed build verified as `wellfed-build sign` verifies it and signed with the update key above the floor, the bench's `private/verification/*.manifest` and the manifests kept under `--backups`. The manifest is kept as `promotion-v<N>.manifest` beside the backup before it is offered. The bench reads only the default `private/stock-backups`, so it does not see promotions kept under another `--backups`. A build that moves the motor at startup is refused: sign it attended and pass `--manifest` |
| 11. Promote | `update::Installer`, the same push as the bench's. Its error ends the run. Once writing began, bank 1 may be partly overwritten, so the verified backup is then the copy of stock | Chunks over the device API until bank 1 is activated. A dropped session is reopened, and the install resumes where bank 0 says |
| 12. Confirm | Done only when bank 1 runs the build and either reports the manifest's version confirmed or is ready to feed, which an unconfirmed trial never is. A reinstall of a build the feeder confirmed before skips the trial and leaves the floor below the version. Bank 0 means it fell back | `Status` and `UpdateState` after the restart |

**Trust on first use, and its risk.** The broker trusts whichever stock-shaped client reaches it first inside the window. A host on the same network that wins that race (or a second feeder) would be enrolled instead. Without a check, it could be walked through the whole run to a false "Installed" while the real feeder stays on stock. So:

- The run stops for the owner to confirm the enrolled device ID (on the feeder's label or in the vendor app) and its address (the router's client list) before the install request.
- The owner may answer that check ahead with `--expect-device <ID>`, the stock MQTT client ID they hold from evidence independent of the broker: the label, the vendor app, or an earlier enrollment they confirmed against the label. An ID copied from what the broker enrolled proves nothing, since an impostor that won an earlier run would then pass. The run then passes the check for that device without asking and prints that the pin matched. Any other device stops the run, naming both IDs, before anything is sent to stock. The address goes unchecked: by pinning, the owner relies on the ID, which is what stock's credentials are bound to.
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
- an installer's config (`wellfed-stock init-installer`), given with `--stock-config`. A config with a plan snapshot, as the bench's captured one has, is refused before anything starts: the server would send that snapshot to stock, which replaces its own plans with it;
- the replacement's frozen review (stock installs its `image.bin`), under a checkout's `private/`. For the promotion, either the signed manifest of the same build (`--manifest`), signed by a key the image trusts, or a key for the run to sign it with: `--update-key` (created if missing), or for a bench review the bench key;
- for a consumer image, the home network to give it over Bluetooth and a Bluetooth adapter in range of the feeder. `--home-network <file>` reads the network from a private JSON file of its `ssid` and `password` (the bench's `private/hotspot.json` has that shape), and the passphrase is never printed. `--home-ssid`, or `--ssid` on the frame route, reads the passphrase at the terminal instead;
- a `wellfed` built from a clean commit of the sources that decide and send the install request (`wellfed-stock`, `wellfed-client` and `wellfed-install`): stock's server refuses to enable the trial otherwise, in any process, and the CLI checks it before the feeder is touched (a run from bank 0 dispatches nothing and is not checked);
- a broker address on the home network that the feeder can reach on TCP 1883 and 8088.

It holds the bench lease beside the stock config for its whole length. On the bench it runs as the server's `install` job instead, on the server's lease, so `wellfed-bench serve` never stops for it ([install as a job](../devices/plaf103/bench.md#install-as-a-job)).

**How it asks.** The run stops at each step that needs you: setup mode, rejoining your network, the feeder check, the recovery hold, and a restart if stock's reports were lost. By default it asks at the terminal: press Enter once you have done what it says, or answer `y` to a question (anything else is no). With `--prompts lines` it writes each question as one JSON line on standard output and reads one JSON answer line from standard input, for a program that runs it, such as the bench server. Everything else it prints goes to standard error. The fallback route and `--home-ssid` read the passphrase at the terminal, so `--ssid` and `--home-ssid` are refused with `--prompts lines`; `--home-network` is not.

Two steps need presses at the panel inside a window the feeder times itself: the setup code (`key-setup-code`, its six presses by button name) and the Dispense press that enrolls the update key (`press-dispense`). The run does not stop for these. It shows them and goes on at once, since only the feeder's own check of the code or the press decides the outcome, and a wait for an answer would only spend the window. At a terminal they are the same instructions as before, and nothing is read for them. In `lines` mode each is a prompt line that takes done, but the run reads that answer only before it writes its next prompt, so each prompt still gets one answer, in order. The Dispense prompt goes out a second after the enrollment request (`PRESS_AFTER`), by which time the feeder should be waiting for the press. That is a timing assumption, not a signal from the feeder. If it fails, the press comes first: on a consumer image's panel, locked since its boot, it does nothing, the window closes `Unconfirmed`, and the run stops without a second request. Neither press needs the panel unlocked. The feeder takes the code's presses and the confirming Dispense whatever the lock says.

In either mode, a standard input that closes before an answer ends the run. It is never read as no, nor as done. In `lines` mode an answer of the wrong kind, such as `"yes"` to the restart, ends it too.

**Stopping it.** SIGTERM or Ctrl-C ends the run the way its end does: it stops the image server and the stock server's broker, then exits with an error that names the step it stopped in. It also names a write whose outcome had not settled:

- the install request, from when it is sent until the replacement answers;
- the promotion's push, until bank 1 confirms it;

That write may have run, and it is not sent again: read what the feeder runs before you start another run. Pairing is only an owner enrollment, which is safe to repeat. If the install request is still unsettled while the run pairs, the report says so: the enrollment may be repeated, the request may not.

**Not done yet:**
- A consumer install from stock on hardware. The builder makes a consumer image (`PROVISION=consumer`: no build Wi-Fi, the release key built in), and with nothing stored it starts BLE setup mode at boot ([product architecture](product-architecture.md#setup-and-pairing)); `wellfed setup` has paired one on the research unit from bank 1. `wellfed install` now drives the whole consumer run: Bluetooth setup as its pairing step, the join, its update key's enrollment, and the promotion signed above the floor. Those steps are host-tested against the state machine (static analysis and unit tests); no simulator test runs the CLI's Bluetooth path, and nothing of it has run on a feeder. [Running a consumer install on the bench](#running-a-consumer-install-on-the-bench) is the first try.
- The release key. It is not generated yet, so a consumer image's own key is a stand-in, and the run signs with the owner's (or the bench's) enrolled key.
- Deleting `init` and `network` from `wellfed-stock`, which waits for the frame to work on hardware.

### Running an install on the bench

This needs the owner at the panel. The frame route reads the passphrase at a terminal, so it is no bench job: `wellfed install` refuses to run beside `wellfed-bench serve`, and the owner stops the server for it and starts it again afterwards.

1. **Return the unit to stock** over the air. `wellfed-bench itest --only stock-roundtrip` runs both ways; for an install run, only its first half is wanted: `wellfed-bench ota --review <stock review>`. You should see stock 1.1.15 on bank 1 and the replacement in bank 0. If the stock review's `image.bin` has the pinned SHA-256 `05147476…`, it is the backup the run needs.
2. **Write the installer config:** `wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`. On the bench the hotspot is the home network, since the build joins it.
   - The redirect must be off (`network remove`, a host firewall change the owner makes), so stock reaches the broker only because the frame said so.
   - The bench's `server.json` is untouched; the record is named for the installer's config. The trial ledger beside both is the same feeder's, so resolve any trial a bench run left open first.
   - Joining stock's access point needs a Wi-Fi interface other than the one serving the hotspot.
3. **At the panel:** unlock stock (Lock about 5 s if the lock light is on). Hold nothing else yet: the run asks for the Reset hold after its warning.
4. **Run** `wellfed install --stock-config private/installer.json --review <feeder review> --manifest <its signed manifest> --ssid <hotspot>`. When it asks, hold Reset about 5 s, which erases stock's plans, Wi-Fi and settings, note what stock does (sound, lights) and the access point's name, and join the bench host to it. It sends the frame and waits for stock to enroll and report. It shows the enrolled device ID and address for the owner to confirm against the feeder's label, asks for the install once, and waits for the replacement. It asks the owner for the recovery hold (Lock, then Dispense) and the code, reads and keeps the stock backup from bank 1, promotes and waits for confirmation. The review must be a build that serves `ReadBank1`; with an older one, pass `--stock-backup <stock review>/image.bin`.
5. **Check and restore.** You should see the replacement confirmed on bank 1, and `private/installer.device.json` holding stock's own identity, with nothing captured used. Only once the run has ended at Done, remove `private/installer.json` and the files named for it (`installer.device.json`, `installer.contested.json`): a resumed run needs the enrollment to find its feeder. Then have the owner restore the redirect if the bench needs it, and record the run.

### Running a consumer install on the bench

This is the consumer install from stock, run on the research unit (observed). It runs unattended as the bench server's `install` job: the server keys in the Bluetooth setup code and presses Dispense for the update key on a [virtual panel](../devices/plaf103/bench.md#virtual-panel) build, and the owner's `--expect-device` answers the feeder check. Stock reaches the installer's broker by the bench's firewall redirect, so start from the bench's usual state: the redirect in place (`wellfed-stock network apply`, the owner's), the hotspot up as a bounded foreground process, `wellfed-stock` built from a clean commit, and `wellfed-bench serve` running. This form of the run, by the redirect, has not run yet. It sends no setup frame, so stock's clock is left as it reads, and unless the clock is already right the job ends in the clock stop (error 1214) with nothing installed. The bench host's Bluetooth adapter must be on and in range. No motion is planned, but the job presses Dispense, so keep the hopper empty.

1. **Clear the replacement's stored Wi-Fi first,** while a bench image still runs on bank 1: `target/release/wellfed wifi --clear`. A consumer image starts Bluetooth setup only with no Wi-Fi stored; with the hotspot still stored it would join, and the run would pair by the known key without trying Bluetooth.
2. **Build and freeze a consumer image with the virtual panel:** `make -C src/firmware/vendor firstboot FIRMWARE=feeder PROVISION=consumer VARIANT=virtual-panel` (it needs `private/release-key.pub.json`), then `target/release/wellfed-build review build --build private/firmware-build/plaf103/firstboot-consumer --output private/verification/<name>-consumer-review`. The variant reads press lines on its UART; it is for the bench only and never ships.
3. **Pin the feeder:** read stock's MQTT client ID from evidence that does not come from this installer's broker: the feeder's label, the vendor app, or an earlier enrollment you confirmed against the label. This is your answer to the feeder check, given ahead. Keep it out of tracked files.

   > [!NOTE]
   > Never copy the ID from `private/installer.device.json` alone. That file holds whatever reached the broker first, so a pin taken from it would pass an impostor that won an earlier run.
4. **Go to stock:** `target/release/wellfed-bench ota <name>-stock --review <stock review>`. You should see stock 1.1.15 on bank 1. Resolve any trial left open ([the install ledger](../development/stock-server.md#the-install-ledger)), then write the installer's config, removing any `installer.device.json` and `installer.contested.json` a past run left: `target/release/wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`. Have the feeder's Bluetooth address ready: BlueZ on the bench needs it, and `bluetoothctl scan on` lists it while a feeder advertises setup.
5. **Install, as the server's job:**

   ```sh
   target/release/wellfed-bench install --stock-config private/installer.json \
     --review private/verification/<name>-consumer-review --update-key private/ota-key.json \
     --home-network private/hotspot.json --ble-address <address> --expect-device <stock's client ID> \
     --virtual-panel
   ```

   The run asks nothing unless stock's reports were lost, when the job restarts the feeder itself with relay 1. The bench key as `--update-key` keeps `wellfed-bench ota` working afterwards: the consumer image does not build it in, so the run enrolls it.

   You should see stock enroll and the line saying the enrolled device is the one `--expect-device` pins. If another device enrolled, the run stops there, naming both, and nothing is sent to stock. Then stock installs the image, and `The replacement has no network yet, so it starts Bluetooth setup by itself`. The job's output then records each press: `press 1 of 6` to `press 6 of 6` for the `key-setup-code` prompt, each followed by `Pressed <button> for 150 ms`. The run reports the pairing, the feeder rebooting, and the replacement answering on the hotspot. Then one `press 1 of 1, dispense` for the `press-dispense` prompt, and the enrollment. After that it reads and keeps the backup, reports `The feeder's floor is version N`, signs the build above it (`promotion-v<M>.manifest` beside the backup), promotes and waits for bank 1's confirmation.

   A press that does not echo and release stops the installer, and the job ends with that press named. Nothing is pressed again. If it stopped during pairing, the run says pairing is safe to run again; if it stopped during the enrollment, the run names it as unsettled, and `wellfed update-key show` says whether the key went in.
6. **Check:** `target/release/wellfed update-key show` lists the bench key; `target/release/wellfed status` shows the consumer build on bank 1; `private/stock-backups/<identity>/` holds the backup and the promotion manifest.
7. **Restore:** remove `private/installer.json` and the files named for it, stop the hotspot as before, then return the unit to a bench image with `wellfed-bench ota` (it now accepts the consumer feeder, which lists the bench key). Record the run: versions, digests, the floor and the signed version, and the job's press lines.

If the run stops after the pairing, the same job with `--from-bank0` resumes it. The image lease is gone by then (the image server removes it when it stops), so the run pairs with the feeder it recorded beside the backups (`private/stock-backups/installing.json`: the identity the feeder proved and where it answered), or the one the key file knows. It looks there and over mDNS, where only that identity can complete the session, and never starts Bluetooth setup once the key knows the feeder: if none answers, it stops and asks for `--address`. The enrollment then lists the key first.

At a terminal, the same run works with the bench server stopped and the owner at the panel: `target/release/wellfed install` with the same options, less `--virtual-panel`, and `--home-ssid <hotspot SSID>` in place of `--home-network` if you would rather type the passphrase. It shows the code and the Dispense press as instructions, and the owner keys them in.

## Rollback

The vendor's firmware can't ship with the app. But each feeder already holds its own copy of stock, until promotion overwrites bank 1.

**The main route: the installer saves the feeder's own stock.**

- **Where it is.** After stock installs the build into bank 0, bank 1 still holds the stock application, with only the first word of its 8-byte marker overwritten. Stock's activation programs that one 4-byte word and nothing more (inferred from the bytes and the static read). On the research unit it left `00 00 21 01` over the marker's `38 31 39 35` (physical observation, October 29, 2026): programming only clears bits, so each byte is the marker's with some bits gone.
- **Who reads it.** The build on bank 0 can read bank 1 (`sdk::bank1_read`, also used for write readback).
- **Upload.** Before the installer offers the promotion, bank 0 streams bank 1 to it on the device API: `ReadBank1 { offset }`, answered by `Bank1Page`s of 768 bytes, owners only, served only from bank 0, and refused `Busy` while an install holds the flash ([device API](device-api.md#stock-backup)). The transport carries no trust; the hash check below is what shows the copy is intact.
- **Promotion waits for it.** Promotion overwrites bank 1, the feeder's only copy of stock. So the installer refuses to promote until a verified backup is stored. You cannot lose stock without holding a copy of it.
- **Rebuild.** The installer is the server during an install, so the backup lands in the app. It restores the marker's first word, accepting only bytes that lost bits from the marker's (the whole bank must still hash to the pin), and rebuilds the OTA container: a 32-byte header, the payload length and byte-sum, as in [the container format](../devices/plaf103/stock/ota-requirements.md#the-image-container).
- **Checks.** The rebuilt image must match the stock image profile. For 1.1.15 it must match the pinned payload hash exactly, so a damaged read shows before anything relies on it.
- **Storage** is per device. The CLI keeps it under the owner's data directory, `private/stock-backups/<identity>/stock-1.1.15.bin`, written once and never replaced by a different copy. The app, and your server if you want, will keep it the same way.
- **Export and restore** from the CLI:
  - `wellfed backup read` reads and keeps it from a replacement on bank 0, outside an install;
  - `wellfed backup show` prints its version, digest and path;
  - `wellfed backup export --output <file>` checks it again and copies it byte for byte to a new file;
  - `wellfed restore-stock` is "Restore stock", the signed install below.

  After promotion the feeder holds no copy, so the backup comes from the owner's copy, never read back from the feeder.
- **Saving a copy:** when an install ends, the Android app can hand the backup to any app through the share sheet.
- **Restoring from the app:** the app shows the kept backup (stock version, checked) and offers "Restore original firmware". If the phone holds no copy, it says so and takes a file you saved, through the same check.
- **Still to come:** the app, connected to a server, will show the backup, copy it to the owner's server and offer "Restore stock", through the same `wellfed-client` steps.
- **Why not keep it on the feeder.** The only place it fits is bank 1. That means running from bank 0 for good, with no known-good fallback bank, and the first update into bank 1 would destroy it anyway. No other 1.3 MiB of flash is established as free.
- **Why this route.** These are your own bytes, from your own device, so there is no redistribution question.

**A second route: the vendor's update service.** The vendor app asks the cloud for a device's update metadata (private evidence E12). An installer could fetch the official package the same way with the user's own account. This depends on the vendor's service, and may offer only the latest version.

**Rolling back.** Our firmware installs a stock image as a signed update (`wellfed-bench ota --review <stock review>`). The bank-0 build writes it into bank 1. Stock's own KM0 then runs, so our trial counter never counts it.

`wellfed restore-stock` and the app do the same from the owner's backup, through one driver, `wellfed_install::restore`.
- **The key.** A stock offer always needs a downgrade statement bound to the feeder's identity, the manifest's digest and the floor, so nothing can be signed ahead of time. The restore makes an update key when it starts, enrolls it (you press Dispense at the feeder, the physical consent) and signs on the phone or computer. `--key` on the CLI gives a key the feeder already trusts instead, for the bench, and enrolls nothing.
- **The key's file** sits beside the backup only until the restore ends. If the enrollment went through and the run ended before its offer, the next run within the hour finds the key listed: it sends no second enrollment and asks for no second press. A press is consent to this restore, not to one days later, so a file older than an hour is removed and the run enrolls a new key, with a new press. The file goes before the offer does, with the manifest and statement already signed in memory, so a run killed after that cannot offer again with no press; a feeder that never answered the offer costs a second press. It also goes once the feeder says the key is not enrolled, and in the app when you leave the screen. A spent key stays listed on the feeder with its private half gone: it signs nothing more, but it takes a client slot until someone removes it.
- `wellfed_signing::Frozen::stock` signs it as the stock image, at a version above the feeder's floor (`--version` overrides that). Only the pinned container passes, as for a stock review.
- The downgrade statement is bound to the feeder's identity and the floor it reports.
- The push is the installer's own, from either bank: from bank 1, the normal case after promotion, bank 1 gives way and bank 0 writes and activates stock. Both banks read one store and one list of enrolled keys, so bank 0 takes the same offer.
- Nothing re-offers once the push has returned. (Within it, the shared installer offers again only where the update protocol needs it, as after bank 1 gives way.)
- A refusal ends the restore. A feeder that stops answering after the offer leaves the result **uncertain**, since it may already run stock, which serves no device API. That is reported as such, and never re-offered.

What a rollback keeps and leaves behind:

- **Stock's settings stay.** Our firmware never writes `0x3e8000`–`0x3f0000`, so stock comes back still provisioned. It still names the installer's broker and retries forever: hold Reset for about 5 seconds and set it up in the vendor's app.
- **Our storage stays, except the owner's Wi-Fi, plans and clients.** `0x340000`–`0x370000` holds the journal, the confirmation record, the lifetime statistics and the feeder's identity. Stock does not read it. Bank 0 leaves a handover mark when it activates stock, and the next Wellfed boot, a later install or the loader's fallback, clears the stored Wi-Fi, every plan and every client, and resets the update floor and candidate, before anything reads them. A loader fallback does too, which is fine: the owner's signed downgrade already gave up that floor. It keeps the identity, since the owner's backup is filed by it. So a later consumer install behaves like a first one, starts Bluetooth setup by itself with no meals, and a phone that was the owner pairs again. An image with build Wi-Fi (a bench image) pairs through the recovery window, so it keeps all three and only erases the mark.
- **The mark is bank 0's build's to write.** Bank 0 runs the build from an earlier install or update, so a bank 0 older than this feature writes no mark, and the feeder keeps its people until a factory reset. A later install then finds them as they were, and nothing is cleared.
- **A fallback feeds nothing.** If the loader falls back to Wellfed, the feeder has no plans and no network until it is set up again, so no meal runs by itself. Stock's plans stay in stock's sectors, for stock if it comes back.
- **A cut stays recoverable.** The record goes first, then the mark, then the bank 1 marker. A cut before the mark leaves this Wellfed with its owner. A cut after it leaves a boot that clears itself into Bluetooth setup, with no meals, whether or not stock went in. The clearing itself repeats after a cut, because the mark goes last. A record with no marker still refuses the same manifest as failed, as it always did. A run cut after give-way or mid-write leaves its manifest pending, and the next restore cancels that once before its own offer; an install running now stops that restore, and a cancel in any doubt stops it too.
- **Bank 0 keeps our build** as a fallback, which the loader never chooses while stock in bank 1 is valid.
- **Stock's OTA status sector** (`0x3ef000`) may differ from the original after the install.
- **No automatic fallback from a stock that hangs.** The loader falls back to bank 0 only when bank 1 is invalid, and Wellfed is then in Bluetooth setup. If a valid stock fails to boot from bank 1, recovery is UART only (`wellfed-bench stock-prepare` / `stock-execute`).

### Proving rollback on the bench

This needs the owner. It starts from a confirmed `feeder` on bank 1 and an older `feeder` on bank 0. The bench rules apply: one holder, the hotspot and `wellfed-stock serve` as bounded foreground processes, `network apply` already in place, and everything restored afterwards.

1. **Build:** `make -C src/firmware/vendor device firstboot FIRMWARE=feeder`, then `wellfed-build review build --build private/firmware-build/plaf103/firstboot --output private/verification/fw43-01-review` and `wellfed-build sign --review private/verification/fw43-01-review --version <next> --output private/verification/fw43-01-manifest.bin`, with the next manifest version.
2. **Go to stock:** `wellfed-bench ota --review <stock review>` from the confirmed build. You should see stock 1.1.15 on bank 1, and the older build on bank 0.
3. **Check the gate holds.** Plant a foreign file at `private/stock-backups/<identity>/stock-1.1.15.bin` (any bytes; the identity is the key file's 64 hex). Write an installer's config (`wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`): the bench's captured `server.json` holds a plan snapshot, which `wellfed install` refuses. Then run `wellfed install --stock-config private/installer.json --review private/verification/fw43-01-review --manifest private/verification/fw43-01-manifest.bin`, and confirm the enrolled feeder. Stock installs the build into bank 0, the run pairs (the bench key is already enrolled), and it stops at the backup step: you should see the kept file refused and nothing promoted. Check the feeder answers from bank 0 (`wellfed status`), then remove the planted file.
4. **Take the backup:** `wellfed install --from-bank0 --stock-config private/installer.json --review … --manifest …`. It reads bank 1 (1,792 pages), keeps it, promotes and confirms. Check that `sha256sum private/stock-backups/<identity>/stock-1.1.15.bin` prints `05147476…` (so its payload is `802ef620…`), and that `cmp` finds the file identical to the stock review's `image.bin`.
5. **Export:** `wellfed backup export --output private/fw43-export.bin`, then `cmp` it with the kept file.
6. **Roll back.** With `wellfed-stock serve --config private/installer.json` running (the installer's config from step 3, or a captured config without a `plans` key, never the bench's `server.json`, whose `plans: []` would erase stock's plans as soon as stock comes back) and a UART capture (`wellfed-flash read observe`), run `wellfed restore-stock --key private/ota-key.json --version <next+1>`. Bank 1 gives way, bank 0 writes stock, and the restore reports activated. You should see stock's `app_main:version:` banner from bank 1 on the UART, and stock connecting to the broker it was provisioned to with no new setup frame (`/api/status`: online, `softwareVersion` 1.1.15).
7. **Go back and restore:** `wellfed-bench stock-ota` with a `feeder` review, back to the replacement confirmed on bank 1. Stop `serve` and the hotspot as before, and record the run: versions, digests and the kept file's path.

## Where the installer lives

The decisions are in [product architecture](product-architecture.md). For the installer they mean:

- **The steps live in `wellfed-client`:** the setup frame, the single stock request, the progress tracking, the backup gate and the backup receiver. The Android app, `wellfed-server` and the CLI run the same steps the bench proves. Each caller supplies the IO: the image server, the broker and the sockets.
- **The Android app** hands stock your Wi-Fi over its setup network, and serves the image itself from a foreground service ([the app](../../src/app/README.md#the-install)).
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
- **Recovered** (October 28, read-only static pass over the 1.1.15 image): the broker kept as a DNS name in `0x3e8000` on plaintext 1883 with no TLS or pinning; setup mode reached only through the Reset hold, whose factory restore erases the plans; the plan reply's fields.
- **Observed** (October 23, `fw40-stock-ota-07-uart-01`): stock looking its broker name up through the network's resolver and then a public one, and a set-up stock connecting with no `BINDING`.
- **Observed** (October 30): the restore, run on the research unit from the app from bank 0 and from bank 1, and by the bench job from bank 1.
- **Modeled** (October 26): the installer, trust on first use, the setup-frame encoder, the backup gate, the stock backup read and `wellfed restore-stock`, host-tested and simulated, on synthetic bytes. Nothing has run on the feeder yet.
