# Consumer installation and rollback

How a feeder on **stock 1.1.15** could move to the replacement firmware with no tools beyond an installer app, and how it could go back. The goal is an experience like the vendor app's, with a self-hosted server in place of the cloud.

Each finding is labeled **physical** (observed on the research unit), **static** (from the stock image or the vendor app, not executed) or **design** (not built). The evidence index for the static findings is private: `private/research/stock-installer/evidence.md`. No vendor code, credentials or device identifiers appear here.

## What is already shown

**Physical (October 23, 2026, research unit):**
- **`fw40-stock-ota-05`:** stock 1.1.15 installed a replacement build through its own MQTT-triggered HTTP updater, with no UART write. It downloaded the image in about 40 s, checked its byte-sum and wrote it to bank 0. The loader then booted the build from bank 0.
- **`fw40-stock-ota-06-promote`:** the build's bank-0 recovery role installed the same build, signed, into bank 1, where it confirmed.
- **`fw40-stock-ota-07`:** the whole sequence in one command, with a fresh build: install, recovery-role health from bank 0, and promotion to bank 1.
- **`fw41-roundtrip-01`:** to stock over our updater and back over stock's, with no UART write and nobody at the bench.
- **Tool:** `wellfed-bench stock-ota` runs this whole sequence ([bench](../hardware/bench.md#stock-to-a-replacement-stock-ota)). The image contract is in [stock OTA requirements](stock-ota-requirements.md).

What made those runs work on the bench, and must stay in any installer:

| Requirement | Why |
| --- | --- |
| One `OTA_UPGRADE`, never resent | A refused precheck can erase stock's OTA status sector; an uncertain request may still be downloading |
| Resolve a failed request before the next one | The server's ledger refuses a new trial while one is failed or uncertain; it resolves from fresh evidence: 20 min, 10 quiet connected minutes, a fresh version report |
| The image port reachable from the feeder | `fw40-stock-ota-02`: with only TCP 1883 allowed, stock connected for about 25 s, then reported `OTA_INFORM` error 2003 |
| Tolerate stock's ~61 s heartbeat | `fw40-stock-ota-03`: a 60 s telemetry bound restarted the quiet interval at every heartbeat |
| Ask stock for its attributes when its version report is stale | Idle stock reports its version only around a connect (`fw40-stock-ota-04`) |
| A URL under 128 bytes, `Content-Length` title-cased, a direct `200` | Stock's parser limits ([requirements](stock-ota-requirements.md#http-server-and-url-requirements)) |
| Mains power, idle, model and version checked first | Stock's own precheck; the server's trial gate checks the same reports |

What the bench still uses and a consumer must not need:
- `wellfed-local network apply`: a root firewall redirect of the feeder's TCP 1883, plus WAN blocking;
- device MQTT credentials captured from traffic (`wellfed-local init`);
- a build carrying the owner's Wi-Fi credentials, broker credentials and update key.

The setup protocol below removes the first two. Generic images remove the third.

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

**Credentials.** The MQTT client ID, username and password the feeder presents are its own: they are loaded from flash at boot, not issued at bind. `BINDING` is an application-level association, which `wellfed-local` already answers by replaying the captured cloud reply. The commands stock dispatches over MQTT are `OTA_UPGRADE`, `GET_CONFIG`, `BINDING`, `UNBIND`, `DEVICE_REBOOT`, `WIFI_RECONNECT_SERVICE`, `WIFI_CHANGE_SERVICE`, `ATTR_SET_SERVICE` and `ATTR_GET_SERVICE` ([stock behavior](stock-behavior.md)).

**Open questions (to settle physically):**
- Is type 3 really what goes on the wire? Reading the decompiled app depends on its integer encoding, so a capture should confirm it.
- What exactly is the numeric id?
- How is a feeder reset or unbound? Is there a button hold? What does `UNBIND` (`0e015fd4`, not decompiled) clear?
- Are the credentials fixed at the factory, or derived from the serial? Either way they arrive at our broker unchanged.

**Planned physical test:** with stock on the feeder and in setup mode, the bench host joins its access point. It then sends one type-3 frame with `wellfed-local setup-send --ssid <hotspot> --id <digits>`, which reads the passphrase from stdin and names this server's broker (`10.42.103.1:1883`) unless `--broker` is given. The frame encoder is host-tested against the parser's length rule and field limits. If stock then joins the hotspot and connects to `wellfed-local` with its own credentials, the redirect and the captured credentials are no longer needed.

## What a self-hosted server must answer (static, partly physical)

`wellfed-local` already implements the stock side (physical: stock 1.1.15 local dispensing):
- a broker on TCP 1883 that accepts stock's legacy MQTT 3.1 CONNECT;
- the `dl/PLAF103/<device-id>/…/post` and `/sub` topics;
- replies to `NTP`, `BINDING` and `GET_FEEDING_PLAN_EVENT`;
- acknowledgements of the startup, attribute, log and error events.

For a consumer it must also accept a device whose credentials it has never seen. The plan is trust on first use during an install window: the first CONNECT carrying a PLAF103 client ID while the installer is open is recorded as that device. This replaces the captured-credential `init` (design).

## The install flow (design)

1. **Setup mode:** the user resets the feeder into its setup access point, the same step as "add device" in the vendor app.
2. **Provision:** the installer joins that access point and sends one type-3 frame: the home network, plus the installer's own `host:1883`.
3. **Connect:** stock joins the home network and connects to the installer's broker with its own credentials, which the installer records.
4. **Pre-flight:** model PLAF103, version 1.1.15 on an allow-list, mains power, idle. These are stock's own precheck conditions; the bench's `stock-ready` stage checks them.
5. **Install:** the installer serves the image and sends one `OTA_UPGRADE`. Stock's progress reports drive a progress bar, about 40 s.
6. **First boot:** the build boots from bank 0, and its recovery role reports health (`"role":"recovery"`) while it waits for an update.
7. **Backup:** the build uploads bank 1, which still holds stock (see rollback below).
8. **Promote:** the installer offers the same build signed. The recovery role installs it into bank 1, and its trial boot confirms; about 3.5 min on the bench.
9. **Hand-off:** the firmware is pointed at the user's self-hosted server, and the installer's keys go with it.

Steps 1–3 replace the bench's firewall redirect and captured credentials. Steps 5, 6 and 8 are what ran physically.

## Rollback

The vendor's firmware cannot be shipped with the app. Each feeder already holds its own copy, though, until the promotion overwrites bank 1.

**Primary: the installer saves the feeder's own stock (design).**
- **Where it is:** after stock installs the build into bank 0, bank 1 still holds the stock application, with only the first byte of its 8-byte marker zeroed. Stock's activation does exactly that and nothing more.
- **Who reads it:** the bank-0 recovery role can read bank 1 (`sdk::bank1_read`, used today for write readback).
- **Upload:** before the installer offers the promotion, the recovery role streams bank 1 to it on a new `backup/put` topic. That topic needs its own broker rule, and is chunked like `ota/data`.
- **Rebuild:** the installer is the server during an install, so the backup lands in the app. It restores the marker byte and rebuilds the OTA container: 32-byte header, payload length and byte-sum, as in [the container format](stock-ota-requirements.md#the-image-container).
- **Checks:** the rebuilt image must match the stock image profile. For 1.1.15 it must match the pinned payload hash exactly, so a mismatch shows a damaged read before anything relies on it.
- **Storage:** per device, in the app and, if the user wants, on their server.
- **Why this route:** these are the user's own bytes from their own device, so there is no redistribution question.

**Secondary: the vendor's update service (static).** The vendor app asks the cloud for a device's update metadata (private evidence E12). An installer could fetch the official package the same way with the user's own account. This depends on the vendor's service, and may offer only the latest version.

**Rolling back.** Our firmware installs a stock image as a signed update (FW41: `wellfed-bench ota --review <stock review>`, physical in `fw41-roundtrip-01`). The bank-0 role writes it into bank 1. Stock's own KM0 then runs, so our trial counter never counts it.

What a rollback keeps and leaves behind:
- **Stock's settings stay:** `0x3e8000`–`0x3f0000` are never written by our firmware, so stock comes back still provisioned. It points at the installer's broker if steps 1–3 were used; re-pairing with the vendor app returns it to the cloud.
- **Our storage stays:** `0x340000`–`0x360000` holds the journal, network credentials and the confirm record. Stock does not read it.
- **Bank 0 keeps our build** as a fallback, which the loader never chooses while stock in bank 1 is valid.
- **Stock's OTA status sector** (`0x3ef000`) may differ from the original after the install.
- **No automatic fallback:** if stock fails to boot from bank 1, recovery is UART only (`wellfed-bench stock-prepare` / `stock-execute`).

## Product architecture (design)

- **Self-hosted server:** `wellfed-local` grown into the feeder's permanent home: the broker, the device protocols (stock and replacement), the API, and the Svelte web UI the bench already uses. It ships as one binary or container.
- **Installer library:** a Rust crate shared by the CLI, desktop and mobile apps. It covers the setup frame, the image server, the single stock request, the progress tracking and the backup receiver: the code the bench now proves.
- **Apps:** Tauri, desktop and mobile, on the same Svelte UI.
  - **BLE:** Tauri has no official BLE plugin. Desktop can use `btleplug`; mobile needs a plugin or thin native code.
  - **Setup network:** joining the feeder's access point needs platform Wi-Fi APIs.
  - **Install-time server:** the app is the server only during an install, about 3 minutes in the foreground, since iOS suspends background apps.
  - **Afterwards:** the app pairs over BLE and talks to the feeder directly on the LAN; the self-hosted server is optional ([product architecture](product-architecture.md)).
