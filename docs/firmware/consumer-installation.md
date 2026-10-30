# Consumer installation and rollback

This page explains how a feeder on **stock 1.1.15** can move to Wellfed with nothing but an installer app, and how it can go back. It is for owners wondering what installing involves, and for contributors working on the installer. The goal is an experience like the vendor app's, with your own server in place of the cloud.

No vendor code, credentials or device identifiers appear here. The evidence index for the static findings is private: `private/research/stock-installer/evidence.md`.

## The one idea

Stock's own updater does the hard part. Stock 1.1.15 will install any image we serve it into its other bank, with no UART and no soldering. So the installer only has to get stock talking to it, ask for one update, and serve the image. Stock finds its broker by a DNS name, so your router's DNS can send it to the installer without touching the feeder. Before Wellfed overwrites stock's last copy, the installer saves the feeder's own stock image and its feeding plans, so you can always go back, and your meals come along.

| Step | Status |
| --- | --- |
| Stock installs Wellfed over its updater | Works |
| Promotion to bank 1 and confirmation | Works |
| Going back to stock over the air | Works, using a stock image we serve |
| Moving stock to the installer by DNS | Experimental: host-tested, not yet run on stock |
| The Reset-hold fallback with the setup frame | Experimental: host-tested, not yet run on stock |
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

Moving stock by DNS, below, removes the first two; stock's setup protocol is the fallback. Generic images remove the third.

## Moving stock by DNS

Stock keeps its broker as a DNS name, connects to it on plaintext MQTT port 1883 with no TLS and no pinning, and looks the name up each time it starts ([where stock connects](../devices/plaf103/stock/README.md#where-stock-connects)). So if your network's DNS answers that name with the installer's address, stock connects to the installer instead of the vendor's cloud, with its own credentials. Nothing is written to the feeder: its Wi-Fi, settings and feeding plans stay as they are, and the installer imports the plans before it overwrites stock.

This is the installer's first route. What you do:

1. **Override one name.** In your router's DNS settings, or a resolver you run that your router hands out, make stock's broker name resolve to the computer running the installer. Many routers call this a local DNS record or a host override.
2. **Restart the feeder.** Unplug its adapter for about 10 s, with its batteries out if fitted, and plug it back in. Stock keeps its session to the cloud until it restarts.
3. **Wait for stock.** The installer's broker trusts the first stock that connects, and shows you its device ID and address to confirm, as after a setup frame ([trust on first use](#the-installer)).
4. **Remove the override** once the install is done. The replacement does not use it, but a feeder rolled back to stock would keep looking for the installer. While the override stands, every device on your network that uses the same name reaches the installer instead of the vendor: other feeders from that vendor lose their cloud connection, and a second stock feeder could reach the installer's broker first.

The broker name is the vendor's, and Wellfed does not ship it: `wellfed install --broker-name` takes it from you. Stock looks it up at every boot, so a resolver that logs queries shows it. On the bench, `wellfed-stock broker-name <flash read>` reads it from stock's settings.

> [!NOTE]
> Stock asks the resolver its network hands out first, then a public one. If your router's resolver answers the name, stock uses that answer. If your router hands out a public resolver, the override has no effect; if its resolver does not answer, stock most likely asks the public one instead. Either way the installer waits for a stock that never arrives. Nothing is sent to stock then, so you can fix the override and run again.

**What the installer checks.** It refuses a name that is an address, and a broker on any port but 1883. Once you have set the override, it looks the name up from its own computer and says what it found: the installer alone, the installer among other addresses (stock may take any of them, so make it the only answer), another address, or nothing. That is advice only: your computer may ask a different resolver than the feeder. Only then does it open its broker's enrollment window and ask you to restart the feeder. The real check is stock arriving at the installer's broker and reporting 1.1.15. If it does not within the enrollment window and its grace, the run ends at that step with nothing sent to stock.

**What the installer's broker does differently** for a stock that was already set up: nothing, so far as is known. Stock presents its own credentials, which enrollment trusts on first use. A set-up stock sends no `BINDING` when it connects (observed), so the missing captured binding reply does not matter. The server never sends the messages that rewrite stock's broker or Wi-Fi settings, or erase its plans ([stock server](../development/stock-server.md#a-set-up-stock-moved-by-dns)).

**Modeled and unproven.** The DNS step and enrollment of a never-seen identity are host-tested. Unproven on hardware: stock following a resolver override (rather than the bench's firewall redirect), and the time stock takes to retry a broker that is not yet listening.

**Stock's clock.** The installer's server sets stock's clock only when stock's own clock is within 10 minutes of it, as a stock fresh from its vendor's cloud should be ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use)). Otherwise it leaves the clock alone, since we have not checked whether stock runs meals that came due while its clock jumps. Stock with its clock two days behind refused an install with error 1214 (observed once; the clock as the cause is likely). The run then ends and names `--calibrate-clock`. Pass it to have the clock set anyway, if you accept that stock may run those meals. An install with the clock set is not yet run on stock.

**Where DNS cannot be changed**, the fallback is stock's setup protocol, below. It starts with the Reset hold, which erases stock's feeding plans, so the installer warns you first and asks you to note them in the vendor app.

## Stock's setup protocol

This is the fallback route. The vendor app provisions a PLAF103 through the feeder's **setup access point**, not over Bluetooth:

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

**Getting into setup mode** costs stock's plans. A set-up stock never opens its setup access point on its own: when it cannot reach its Wi-Fi, it retries forever. The only way in is the Reset hold: Reset (the rightmost button) held about 5 s on an unlocked panel. That runs stock's full factory restore. It erases its broker, Wi-Fi and binding records, its feeding plans and their record rings, then restarts ([what the restore clears](../devices/plaf103/stock/README.md#reset-and-the-cloud)). So an install through this route imports no plans. The panel is unlocked by holding Lock about 5 s while the lock light is on. That the restart then opens the setup access point, and its name, are not yet seen on hardware.

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
- replies to `NTP` and to `BINDING` (only with a captured reply, which a stock fresh from the setup frame asks for), and to `GET_FEEDING_PLAN_EVENT` only with a captured plan snapshot. An installer's config has none, so that request goes unanswered: stock erases its plan table on any reply with a `plans` key, and the installer must find the owner's plans intact ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use));
- acknowledgements of the startup, attribute, log and error events.

For a consumer it must also accept a device whose credentials it has never seen. **Trust on first use:** an installer's config (`wellfed-stock init-installer`) names no device. `serve --enroll-minutes N` admits the first stock-shaped CONNECT within the window. It records that client's ID, username, password and address beside the config (`installer.json`'s record is `installer.device.json`) and pins it, so nobody else logs in, then or later ([stock server](../development/stock-server.md#an-installers-server-trust-on-first-use)). Stock's client ID is its device ID. A stock moved by DNS arrives the same way, with the same credentials. The captured-credential `init` and the `network` redirect stay for the bench until either route has moved stock on hardware.

## The install flow

1. **Point the name:** your DNS answers stock's broker name with the installer's address ([moving stock by DNS](#moving-stock-by-dns)).
2. **Restart:** you restart the feeder, and stock looks the name up again.
3. **Connect:** stock connects to the installer's broker with its own credentials, which the installer records.
4. **Pre-flight:** model PLAF103, version 1.1.15 on an allow-list, mains power, idle. These are stock's own precheck conditions; the bench's `stock-ready` stage checks them.
5. **Install:** the installer serves the image and sends one `OTA_UPGRADE`. Stock's progress reports drive a progress bar, for about 40 s.
6. **First boot:** the build boots from bank 0 and answers the device API while it waits for an update. A consumer image has no network yet, so it starts BLE setup mode: the installer pairs and sends your home Wi-Fi, and the build reboots and joins it.
7. **Pair:** the next steps go over the device API, which needs a client key. With a consumer image, the Bluetooth setup in step 6 is that pairing: the installer becomes the first owner. A bench image joins its build network instead, knows no client key, and the CLI pairs through the LAN recovery window.
8. **Backup:** the build streams bank 1, which still holds stock, to the installer, which verifies it ([rollback](#rollback)).
9. **Stock's plans:** the build serves stock's two plan sectors, and the installer converts each plan or refuses it by name ([importing them](../devices/plaf103/stock/README.md#importing-them)).
10. **Promote:** only once a verified backup is stored, the installer offers the same build, signed, over the device API. A consumer image trusts only the release key and keys its owner enrolled, so the installer first enrolls its own update key (you press Dispense), then signs the build with it above the feeder's floor. Bank 0 installs it into bank 1, and its trial boot confirms.
11. **Plans applied:** the converted plans go onto the confirmed build, only if it has no plans yet.
12. **Hand-off:** the confirmed build enters the [pairing](pairing.md) flow. Your app is its first client, and a server joins later by invite if you run one.

Steps 1–3 replace the bench's firewall redirect and captured credentials. On the DNS route, stock's plans are still in place at step 9.

Where DNS cannot be changed, the fallback replaces steps 1 and 2: you note your plans, reset the feeder into its setup access point (which erases its plans), and the installer sends one type-3 frame there with your home network and its own `host:1883`.

## The installer

One run, `wellfed install`, goes from stock to a confirmed replacement. It is built in three layers:

- **The steps** are a sans-IO state machine in [`wellfed_client::install`](../../src/client/wellfed-client/src/install.rs). Every decision between steps is made there.
- **The driver** is [`wellfed-install`](../../src/client/wellfed-install), a tokio crate that does the IO each step asks for: stock's server, the request, pairing, the backup, the promotion, the plans' import, and the record a resumed run reads. The CLI and the Android app link the same driver.
- **The platform** supplies what only it owns, through the driver's traits: the person at the feeder (prompts), the Bluetooth radio, finding a feeder on the network, the client key, and how progress is shown. The CLI uses a terminal (or the bench's JSON lines), BlueZ, mDNS and its key file, and prints each progress event as a line. The app uses its screens, Android's Bluetooth and NSD, and its secure storage.

Stock's side runs in the driver's own process, since an app cannot spawn the `wellfed-stock` binary. The broker, the server's session with stock and the image server are tasks the run starts and stops ([stock server](../development/stock-server.md#how-the-crate-is-laid-out)). The CLI also serves the server's loopback API, for the bench's page.

| Step | Decided in `wellfed-client` | IO in `wellfed-install` (and the CLI's flags) |
| --- | --- | --- |
| 1. Provision | DNS first: the override, refused unless it names a DNS name and the broker listens on port 1883, and its lookup judged as advice only. The fallback: a warning that the Reset hold erases stock's plans, which the owner must accept, then the type-3 frame, refused before anything is sent if stock would misread a field; sent once | `--broker-name`: the guidance, the owner's word that the override is set and the feeder restarted, and a lookup from this computer. `--ssid`: the warning and a `[y/N]`, then the owner puts stock into setup mode and joins this computer to its access point (platform Wi-Fi); one UDP datagram to 192.168.1.1:17484; the owner rejoins the home network |
| 2. Stock ready | Online, 1.1.15 (any other version ends the run), on its adapter | Stock's server in this process, with the trial enabled and an enrollment window of `--enroll-minutes`: the broker trusts the first feeder, and holds that feeder's CONNACK until the server has subscribed, so stock's startup reports reach it. Its status, polled. If stock still reports no version after 90 s, the owner restarts it once to replay them |
| 3. Confirm the feeder | The owner checks the enrolled device ID and address before anything is sent; a contested enrollment ends the run | The status document's `enrollment` (never its credentials) and a `[y/N]` prompt. With `--expect-device`, the owner's answer given ahead: the pinned device passes without a prompt, and any other ends the run naming both. A bench config with a captured device needs no check |
| 4. Request | Exactly one `OTA_UPGRADE` per run; refused or uncertain ends the run | The image server in this process, serving the review to the enrolled feeder's address, then the trial request. What the server recorded is read back: refused only when the server refused and a successful ledger read shows no record, uncertain otherwise (an unanswered request included). The server itself refuses while the enrollment is contested |
| 5. Install | Progress; stock's refusal ends the run, even before the request's outcome is read; stock going quiet means it restarted | The trial phase, and the OTA reports since the request that name it (or nothing) |
| 6. Pair | The installer must be an owner before anything else. The route follows the review's provisioning (`PairRoute`): a consumer image by Bluetooth setup, then a wait for its join; a bench image by the recovery window | A key the build already knows opens a session at once. A run whose broker enrolled the same stock device as an earlier run reads that run's record (`installing.json` under `--backups`): it looks for the feeder that run paired with where it last answered, where stock connected from and over mDNS, and stops naming `--address` if it does not answer. Otherwise the key file's feeder is tried only where stock connected from (or `--address`), since the key file may know another feeder, and setup follows if it does not answer. The run that finishes removes its record. A consumer image: `wellfed setup`'s exchange over BlueZ (`--ble-address`, or the first feeder advertising setup) with the home network (`--home-network`'s file; or `--home-ssid`, or the frame's `--ssid`, the passphrase read once), the code shown to key in; then the feeder is looked for over mDNS by the identity it proved, and where stock connected from, until a session opens. A bench image: the owner holds Lock, then Dispense, on the unlocked panel and keys in the code (`wellfed recover`'s exchange), at the address stock connected from |
| 7. Bank 0 | Only the served build moves on. On bank 0 the stock server's record is then closed; on bank 1 (a run resumed after activation) only the confirmation is left, and a run that signs for itself must know the version it promoted | `Status` over the device API; the server's record closed on that answer, asked again while stock's telemetry is still current. A resumed run reads the version from the manifest kept beside the backup |
| 8. Update key | Only when the image does not build the signer's key in. `update_key::Enroll`: a key already listed as an update signer is skipped; the enrollment goes once; a refusal ends the run before promotion; an unanswered one is judged by the client list (the key listed as a signer moves on, anything else ends the run), never sent again | `--update-key` (created as `wellfed update-key new` would if missing; a bench review signs with the bench key, which its image builds in). With `--manifest` no key is enrolled, since the manifest is promoted as signed, and `--update-key` beside it is refused. `ListClients`, then `EnrollUpdateKey` with the owner pressing Dispense within 60 s, then `ListClients` again if it went unanswered |
| 9. Backup | **No promotion without a `VerifiedBackup`.** `Bank1Reader` reads bank 1 a page at a time, and `StockBackup::from_bank` restores the marker's first word, checks the payload against the pin and rebuilds the vendor's container | `ReadBank1` over the device API (owners only, bank 0 only), the copy kept under `--backups` (`private/stock-backups/<identity>/stock-1.1.15.bin`), or one kept by an earlier run. `--stock-backup` instead takes a copy the owner holds, checked before the feeder is touched |
| 10. Stock's plans | `StockPlans::decode_parts` reads the sectors as stock's boot does, and `plans::convert` converts each plan or refuses it by name. **A table that cannot be read stops the run before promotion**, unless the owner chose to go on without it | `ReadStockPlans` over the device API (owners only, bank 0 only). The 441 bytes are kept beside the backup (`private/stock-backups/<identity>/stock-plans.bin`, never replaced by a different read), and the import is shown. `--without-stock-plans` promotes even when the sectors cannot be read |
| 11. Version | The floor (`UpdateState`'s highest confirmed version) decides. A given manifest at or below it is refused (it would need a downgrade statement); a run that signs for itself picks one above the floor and above every version it signed before | `--manifest`, or the reviewed build verified as `wellfed-build sign` verifies it and signed with the update key above the floor, the bench's `private/verification/*.manifest` and the manifests kept under `--backups`. The manifest is kept as `promotion-v<N>.manifest` beside the backup before it is offered. The bench reads only the default `private/stock-backups`, so it does not see promotions kept under another `--backups`. A build that moves the motor at startup is refused: sign it attended and pass `--manifest` |
| 12. Promote | `update::Installer`, the same push as the bench's. Its error ends the run. Once writing began, bank 1 may be partly overwritten, so the verified backup is then the copy of stock | Chunks over the device API until bank 1 is activated. A dropped session is reopened, and the install resumes where bank 0 says |
| 13. Confirm | Done only when bank 1 runs the build and reports the manifest's version confirmed; bank 0 means it fell back | `Status` and `UpdateState` after the restart |
| 14. Apply the plans | Only with converted plans, and only onto a writable table with every slot empty: plans already set stop it. One `PutPlans` from the generation read; `Stale` or another refusal changed nothing and ends the run. A table already holding the import counts as applied. An unanswered put is never sent again: the plans read back decide, the import there is done, anything else is uncertain. A run resumed after the promotion reads no sectors, so it takes the copy an earlier run kept, under the same check | `GetPlans`, then the one `PutPlans`, then `GetPlans` again if it went unanswered. A resumed run loads `stock-plans.bin` from beside the backup, or says that stock's plans were not read |

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
- a broker address on the home network that the feeder can reach on TCP 1883 and 8088. The DNS route needs port 1883 exactly, since stock keeps its stored port;
- for the DNS route, stock's broker name and a DNS override you can set.

It holds the bench lease beside the stock config for its whole length. On the bench it runs as the server's `install` job instead, on the server's lease, so `wellfed-bench serve` never stops for it ([install as a job](../devices/plaf103/bench.md#install-as-a-job)).

**How it asks.** The run stops at each step that needs you: the override, the restart, the feeder check, the recovery hold, and on the fallback route the Reset warning, setup mode and rejoining your network. By default it asks at the terminal: press Enter once you have done what it says, or answer `y` to a question (anything else is no). With `--prompts lines` it writes each question as one JSON line on standard output and reads one JSON answer line from standard input, for a program that runs it, such as the bench server. Everything else it prints goes to standard error. The fallback route and `--home-ssid` read the passphrase at the terminal, so `--ssid` and `--home-ssid` are refused with `--prompts lines`; `--home-network` is not.

Two steps need presses at the panel inside a window the feeder times itself: the setup code (`key-setup-code`, its six presses by button name) and the Dispense press that enrolls the update key (`press-dispense`). The run does not stop for these. It shows them and goes on at once, since only the feeder's own check of the code or the press decides the outcome, and a wait for an answer would only spend the window. At a terminal they are the same instructions as before, and nothing is read for them. In `lines` mode each is a prompt line that takes done, but the run reads that answer only before it writes its next prompt, so each prompt still gets one answer, in order. The Dispense prompt goes out a second after the enrollment request (`PRESS_AFTER`), by which time the feeder should be waiting for the press. That is a timing assumption, not a signal from the feeder. If it fails, the press comes first: on a consumer image's panel, locked since its boot, it does nothing, the window closes `Unconfirmed`, and the run stops without a second request. Neither press needs the panel unlocked. The feeder takes the code's presses and the confirming Dispense whatever the lock says.

In either mode, a standard input that closes before an answer ends the run. It is never read as no, nor as done. In `lines` mode an answer of the wrong kind, such as `"yes"` to the restart, ends it too.

**Stopping it.** SIGTERM or Ctrl-C ends the run the way its end does: it stops the image server and the stock server's broker, then exits with an error that names the step it stopped in. It also names a write whose outcome had not settled:

- the install request, from when it is sent until the replacement answers;
- the promotion's push, until bank 1 confirms it;
- the plans' put, until the run ends.

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
4. **Run** `wellfed install --stock-config private/installer.json --review <feeder review> --manifest <its signed manifest> --ssid <hotspot> --calibrate-clock`. The flag sets stock's clock, which on the bench is usually days behind. The Reset hold has erased stock's plans by then, so no meal comes due in the jump. The run first warns that the Reset hold erases stock's plans, Wi-Fi and settings, and asks `[y/N]`; answer `y`. When it asks, hold Reset about 5 s, note what stock does (sound, lights) and the access point's name, and join the bench host to it. It sends the frame and waits for stock to enroll and report. It shows the enrolled device ID and address for the owner to confirm against the feeder's label, asks for the install once, and waits for the replacement. It asks the owner for the recovery hold (Lock, then Dispense) and the code, reads and keeps the stock backup from bank 1 and stock's plan sectors, shows the plan import, promotes, waits for confirmation and sets the converted plans. The review must be a build that serves `ReadBank1`; with an older one, pass `--stock-backup <stock review>/image.bin`.
5. **Check and restore.** You should see the replacement confirmed on bank 1, and `private/installer.device.json` holding stock's own identity, with nothing captured used. Only once the run has ended at Done, remove `private/installer.json` and the files named for it (`installer.device.json`, `installer.contested.json`): a resumed run needs the enrollment to find its feeder. Then have the owner restore the redirect if the bench needs it, and record the run.

### Running a consumer install on the bench

This is the first consumer install from stock, not yet run (everything about it is static analysis and host tests). It runs unattended as the bench server's `install` job: the server keys in the Bluetooth setup code and presses Dispense for the update key on a [virtual panel](../devices/plaf103/bench.md#virtual-panel) build, and the owner's `--expect-device` answers the feeder check. It runs the [DNS route](#running-the-dns-route-on-the-bench) with a consumer image, so start from that section's state: the owner's DNS drop-in, the WAN block without the redirect, the hotspot up as a bounded foreground process, `wellfed-stock` built from a clean commit, and `wellfed-bench serve` running. The bench host's Bluetooth adapter must be on and in range. No motion is planned, but the job presses Dispense, so keep the hopper empty.

1. **Clear the replacement's stored Wi-Fi first,** while a bench image still runs on bank 1: `target/release/wellfed wifi --clear`. A consumer image starts Bluetooth setup only with no Wi-Fi stored; with the hotspot still stored it would join, and the run would pair by the known key without trying Bluetooth.
2. **Build and freeze a consumer image with the virtual panel:** `make -C src/firmware/vendor firstboot FIRMWARE=feeder PROVISION=consumer VARIANT=virtual-panel` (it needs `private/release-key.pub.json`), then `target/release/wellfed-build review build --build private/firmware-build/plaf103/firstboot-consumer --output private/verification/<name>-consumer-review`. The variant reads press lines on its UART; it is for the bench only and never ships.
3. **Pin the feeder:** read stock's MQTT client ID from evidence that does not come from this installer's broker: the feeder's label, the vendor app, or an earlier enrollment you confirmed against the label. This is your answer to the feeder check, given ahead. Keep it out of tracked files.

   > [!NOTE]
   > Never copy the ID from `private/installer.device.json` alone. That file holds whatever reached the broker first, so a pin taken from it would pass an impostor that won an earlier run.
4. **Go to stock,** as steps 5 to 7 of the DNS route do: you should see stock 1.1.15 on bank 1, and the installer's config written. Have the feeder's Bluetooth address ready: BlueZ on the bench needs it, and `bluetoothctl scan on` lists it while a feeder advertises setup.
5. **Install, as the server's job:**

   ```sh
   target/release/wellfed-bench install --stock-config private/installer.json --broker-name "$NAME" \
     --review private/verification/<name>-consumer-review --update-key private/ota-key.json \
     --home-network private/hotspot.json --ble-address <address> --expect-device <stock's client ID> \
     --virtual-panel --calibrate-clock
   ```

   The run asks one thing, the override set in step 2 of the DNS route: answer done (`wellfed-bench answer JOB 1 done`, the page, or an agent's `job_answer`). The job restarts the feeder itself with relay 1. The bench key as `--update-key` keeps `wellfed-bench ota` working afterwards: the consumer image does not build it in, so the run enrolls it.

   You should see stock enroll and the line saying the enrolled device is the one `--expect-device` pins. If another device enrolled, the run stops there, naming both, and nothing is sent to stock. Then stock installs the image, and `The replacement has no network yet, so it starts Bluetooth setup by itself`. The job's output then records each press: `press 1 of 6` to `press 6 of 6` for the `key-setup-code` prompt, each followed by `Pressed <button> for 150 ms`. The run reports the pairing, the feeder rebooting, and the replacement answering on the hotspot. Then one `press 1 of 1, dispense` for the `press-dispense` prompt, and the enrollment. After that it reads and keeps the backup and stock's plans, reports `The feeder's floor is version N`, signs the build above it (`promotion-v<M>.manifest` beside the backup), promotes, waits for bank 1's confirmation and sets the plans.

   A press that does not echo and release stops the installer, and the job ends with that press named. Nothing is pressed again. If it stopped during pairing, the run says pairing is safe to run again; if it stopped during the enrollment, the run names it as unsettled, and `wellfed update-key show` says whether the key went in.
6. **Check:** `target/release/wellfed update-key show` lists the bench key; `target/release/wellfed status` shows the consumer build on bank 1; `private/stock-backups/<identity>/` holds the backup, `stock-plans.applied.bin` and the promotion manifest.
7. **Restore** as the DNS route's step 10 does, then return the unit to a bench image with `wellfed-bench ota` (it now accepts the consumer feeder, which lists the bench key). Record the run: versions, digests, the floor and the signed version, and the job's press lines.

If the run stops after the pairing, the same job with `--from-bank0` in place of `--broker-name` resumes it. The image lease is gone by then (the image server removes it when it stops), so the run pairs with the feeder it recorded beside the backups (`private/stock-backups/installing.json`: the identity the feeder proved and where it answered), or the one the key file knows. It looks there and over mDNS, where only that identity can complete the session, and never starts Bluetooth setup once the key knows the feeder: if none answers, it stops and asks for `--address`. The enrollment then lists the key first.

At a terminal, the same run works with the bench server stopped and the owner at the panel: `target/release/wellfed install` with the same options, less `--virtual-panel`, and `--home-ssid <hotspot SSID>` in place of `--home-network` if you would rather type the passphrase. It shows the code and the Dispense press as instructions, and the owner keys them in.

### Running the DNS route on the bench

This proves the consumer's first route end to end, with a stock that holds plans. It runs unattended once the owner has changed the hotspot's DNS and the firewall, but for the feeder check, which is the owner's to answer. The feeder's WAN stays blocked throughout, so a failed override fails closed: stock cannot reach the vendor's cloud through its public resolver. It starts from a confirmed `feeder` on bank 1, the hotspot up as a bounded foreground process, `wellfed-bench serve` running, and a fresh `feeder` review with its signed manifest ([proving rollback](#proving-rollback-on-the-bench), step 1). `wellfed-stock` must be built from a clean commit. No motion: the seeded plans never run.

1. **Read the name:** `NAME=$(target/release/wellfed-stock broker-name <a full flash read of the feeder>)`. Any full read works, since our firmware never writes stock's settings. It prints the name alone; never write it into a tracked file.
2. **The owner points the name at the bench host:** add `address=/<the name>/10.42.103.1` to a dnsmasq drop-in the hotspot reads (NetworkManager's shared mode reads `/etc/NetworkManager/dnsmasq-shared.d/`), and restart the hotspot. This changes the host's network configuration, so it is the owner's. Check it: `dig +short @10.42.103.1 "$NAME"` should print `10.42.103.1` and nothing else.
3. **Seed:** write `private/seed-plans.json` and `private/seed.json` and start `wellfed-stock serve --config private/seed.json` ([seeding](../development/stock-server.md#seed-stocks-plans-on-the-bench)).
4. **The owner swaps the redirect for the WAN block alone,** with that `serve` listening (`apply` checks the broker first): `sudo wellfed-stock network remove`, then `sudo wellfed-stock network apply --redirect off`. Nothing is redirected, so stock reaches the server only by name, and its forwarded traffic stays blocked. `network preview --redirect off` shows the rules first: no `dnat`, the forward rejects and both input accepts.

   **Check before going on:** `sudo nft list table inet wellfed_local` must show both forward rejects for the feeder's MAC and no `dnat`. If it does not (an `apply` that failed after the `remove` leaves the feeder's WAN open), do not boot stock: stop here until the owner has the block in place.
5. **Go to stock:** `target/release/wellfed-bench ota <name>-stock --review <stock review>`. You should see stock 1.1.15 on bank 1, and in `wellfed-stock status --config private/seed.json`, stock online with version 1.1.15. With the redirect off, that shows DNS alone moved it. Give stock a minute to ask for its plans.
6. **Check the seed:** `target/release/wellfed-bench dump <name>-seed`, then `target/release/wellfed-stock plans private/flash-backups/<name>-seed-full/flash-read.bin`. You should see both plans, each disabled with a note that its skip is still ahead. **Stop here** if the report does not list both: the install would prove nothing about a nonempty import. Boot stock again with `target/release/wellfed-bench boot <name>-seed-boot`.
7. **Leave the plans alone:** stop the seeding `serve` and delete `private/seed.json`. Resolve any trial the seeding run left open ([the install ledger](../development/stock-server.md#the-install-ledger)). Then write the installer's config, removing any `installer.device.json` and `installer.contested.json` a past run left: `target/release/wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`.
8. **Install,** as the server's job: `target/release/wellfed-bench install --stock-config private/installer.json --broker-name "$NAME" --review <feeder review> --manifest <its signed manifest> --calibrate-clock`. The flag sets stock's clock, which on the bench is days behind; the seeded plans are skipped until 2100, so none comes due in the jump. The stock server's log line for `NTP` records the `ts` stock sent and the skew. The run asks three things:
   - the override, set in step 2: answer done (`wellfed-bench answer JOB 1 done`, the page, or an agent's `job_answer`);
   - the restart, which the job does itself with relay 1, off for 10 s and on again, with the installer's broker already listening;
   - the feeder check: the owner compares the enrolled device ID and address with the feeder and answers yes or no on the page or with `wellfed-bench answer`. An agent may not answer it. If the owner already holds the device ID, `--expect-device <ID>` answers it ahead instead.

   The bench key is already enrolled, so no recovery hold is asked for. The bench host does not ask the hotspot's resolver, so its lookup line may say the name points elsewhere; the run waits for stock regardless.

   You should see stock enroll from its hotspot address and report 1.1.15, one install request, the replacement on bank 0 paired with the known key, and the stock backup kept or reused. The plan import should list both seeded plans, disabled, with the skip note (and the meal-call note on the second). Then the promotion, the confirmation on bank 1, and the plans set.
9. **Check:** `target/release/wellfed plans list` shows the two plans, disabled, at their seeded times and portions, and `stock-plans.applied.bin` sits beside the kept backup.
10. **Restore:** `wellfed plans delete` both plans. Remove `private/installer.json` and the files named for it. The owner removes the DNS drop-in and restarts the hotspot, then, with a `serve` listening, puts the redirect back (`sudo wellfed-stock network remove`, then `sudo wellfed-stock network apply`). Stop the hotspot as before and record the run: versions, digests, the import report and what the owner changed.

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
  - `wellfed restore-stock --version <n>` is "Restore stock", the signed install below.

  After promotion the feeder holds no copy, so the backup comes from the owner's copy, never read back from the feeder.
- **Saving a copy:** when an install ends, the Android app can hand the backup to any app through the share sheet.
- **Still to come:** the Android app and the server's web UI will show the backup (stock version, verified), copy it to the owner's server and offer "Restore stock", through the same `wellfed-client` steps.
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

- **Stock's settings stay.** Our firmware never writes `0x3e8000`–`0x3f0000`, so stock comes back still provisioned. After the DNS route it still names the vendor's broker, so it returns to the cloud once the override is gone. After the fallback it points at the installer's broker; re-pairing with the vendor app returns it to the cloud.
- **Our storage stays.** `0x340000`–`0x370000` holds the journal, network credentials, the confirmation record and the lifetime statistics. Stock does not read it.
- **Bank 0 keeps our build** as a fallback, which the loader never chooses while stock in bank 1 is valid.
- **Stock's OTA status sector** (`0x3ef000`) may differ from the original after the install.
- **No automatic fallback.** If stock fails to boot from bank 1, recovery is UART only (`wellfed-bench stock-prepare` / `stock-execute`).

### Proving rollback on the bench

This needs the owner. It starts from a confirmed `feeder` on bank 1 and an older `feeder` on bank 0. The bench rules apply: one holder, the hotspot and `wellfed-stock serve` as bounded foreground processes, `network apply` already in place, and everything restored afterwards.

1. **Build:** `make -C src/firmware/vendor device firstboot FIRMWARE=feeder`, then `wellfed-build review build --build private/firmware-build/plaf103/firstboot --output private/verification/fw43-01-review` and `wellfed-build sign --review private/verification/fw43-01-review --version <next> --output private/verification/fw43-01-manifest.bin`, with the next manifest version.
2. **Go to stock:** `wellfed-bench ota --review <stock review>` from the confirmed build. You should see stock 1.1.15 on bank 1, and the older build on bank 0.
3. **Check the gate holds.** Plant a foreign file at `private/stock-backups/<identity>/stock-1.1.15.bin` (any bytes; the identity is the key file's 64 hex). Write an installer's config (`wellfed-stock init-installer --mqtt-listen 10.42.103.1:1883 --output private/installer.json`): the bench's captured `server.json` holds a plan snapshot, which `wellfed install` refuses. Then run `wellfed install --stock-config private/installer.json --review private/verification/fw43-01-review --manifest private/verification/fw43-01-manifest.bin`, and confirm the enrolled feeder. Stock installs the build into bank 0, the run pairs (the bench key is already enrolled), and it stops at the backup step: you should see the kept file refused and nothing promoted. Check the feeder answers from bank 0 (`wellfed status`), then remove the planted file.
4. **Take the backup:** `wellfed install --from-bank0 --stock-config private/installer.json --review … --manifest …`. It reads bank 1 (1,792 pages), keeps it, promotes and confirms. Check that `sha256sum private/stock-backups/<identity>/stock-1.1.15.bin` prints `05147476…` (so its payload is `802ef620…`), and that `cmp` finds the file identical to the stock review's `image.bin`.
5. **Export:** `wellfed backup export --output private/fw43-export.bin`, then `cmp` it with the kept file.
6. **Roll back.** With `wellfed-stock serve --config private/installer.json` running (the installer's config from step 3, or a captured config without a `plans` key, never the bench's `server.json`, whose `plans: []` would erase stock's plans as soon as stock comes back) and a UART capture (`wellfed-flash read observe`), run `wellfed restore-stock --version <next+1>`. Bank 1 gives way, bank 0 writes stock, and the restore reports activated. You should see stock's `app_main:version:` banner from bank 1 on the UART, and stock connecting to the broker it was provisioned to with no new setup frame (`/api/status`: online, `softwareVersion` 1.1.15).
7. **Go back and restore:** `wellfed-bench stock-ota` with a `feeder` review, back to the replacement confirmed on bank 1. Stop `serve` and the hotspot as before, and record the run: versions, digests and the kept file's path.

## Where the installer lives

The decisions are in [product architecture](product-architecture.md). For the installer they mean:

- **The steps live in `wellfed-client`:** the setup frame, the single stock request, the progress tracking, the backup gate and the backup receiver. The Android app, `wellfed-server` and the CLI run the same steps the bench proves. Each caller supplies the IO: the image server, the broker and the sockets.
- **The Android app** runs the DNS route, and serves the image itself from a foreground service ([the app](../../src/app/README.md#the-install)). Its route through the feeder's access point is not built: it waits for a person to see stock's Reset hold once.
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
- **Modeled** (October 28): the DNS route's steps, the reset warning and the `broker-name` read, host-tested on synthetic bytes. Not yet run on stock.
- **Modeled** (October 26): the installer, trust on first use, the setup-frame encoder, the backup gate, the stock backup read and `wellfed restore-stock`, host-tested and simulated, on synthetic bytes. Nothing has run on the feeder yet.
