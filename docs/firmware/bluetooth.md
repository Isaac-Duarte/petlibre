# FW34 Bluetooth LE and Wi-Fi setup

Stock 1.1.15 uses Bluetooth LE to give an offline feeder its Wi-Fi and MQTT settings from the vendor app ([stock behavior](stock-behavior.md#bluetooth-le)). The `ble` firmware keeps that role:
- **When:** after about two minutes without a network, it advertises a private setup service.
- **What:** a host sends it Wi-Fi credentials as the same signed FW25 `wifi` request the server sends over MQTT.
- **Then:** the feeder joins the new network at its next attempt, without a restart.

The AmebaD's Bluetooth is LE only. The SDK's `bt_flags.h` sets `F_BT_BREDR_SUPPORT 0`, and the stack has no A2DP or LE Audio, so the feeder never appears as an audio device to a phone.

## Build

A firmware opts in with `bluetooth = true` in `[package.metadata.petlibre]`, and the device names its BLE source (`"bluetooth"` in `device.json`). Both are identity inputs. Only such a firmware gets these overlays; every other build restores the pinned files, so every other firmware's image is unchanged:

- **`.config`:** sets only `CONFIG_BT_EN=y`, which builds the BT board code and links `btgap.a`. The C macro `CONFIG_BT` stays undefined, so the boot-time FTL init, the BT AT commands and the vendor examples stay out.
- **`ftl.c`:** dropped from its makefile. The stack's bond store is FTL, which erases and programs `0x102000–0x105000`, inside OTA bank 0. `petlibre_ble.c` defines the two FTL entry points the stack calls, and both refuse.
- **`check_sw()` in `hci_board.c`:** pinned to an erased sector's value. The board code reads active-low switches from system data at flash `0x3028`, and those switches choose:
  - which controller patch loads (the embedded one, or one read from a fixed address inside our banks);
  - whether PA26 becomes a trace UART;
  - whether the controller logs;
  - whether the host stack starts at all.

  The research unit's word is `0xFFFFFFFF` (static, from the Oct 22 full dump), so the pin matches it. The image then never depends on a unit's system data, and this read takes no flash lease.
- **`petlibre_ble.c`:** staged beside `example_entry.c` and added to the example makefile.

The link audit is unchanged and passes: `ftl_init` and every flash erase and program routine stay absent, and no new eFuse writer is linked (`bt_write_phy_efuse_value` has no caller and is collected). The image carries the 11.5 KB embedded controller patch (`rtlbt_fw`). The first-boot image is 832 KB, well within a bank.

## Runtime

`petlibre_ble.c` starts one task (`petlibre_ble`, 1024 words, priority 1). It waits up to 60 s for the Wi-Fi shim's radio, because `hci_board_init` refuses before `wifi_on`. It then brings the stack up in the vendor example's order and runs the GAP message loop:
- **One link.** `gap_config_le_key_storage_flag(0)` keeps no keys, and pairing is off: a just-works request is rejected. Access control is the request signature.
- **Advertising:** only while Rust asks for it. It is connectable, about 100 ms, with the 128-bit service UUID; the scan response carries the name `PetLibre`.
- **GATT callbacks** only copy bytes into rings (control 1 KiB, outgoing notifications 512 B) and record events.
- **Rust requests** (notify, advertise, disconnect) go through the task's IO queue, so every stack call runs on the task that started the stack.

The Rust side is `petlibre_hal::ble::BleSetup`, run by the `ble` firmware's task (`petlibre-ble`, 1024 words; stack audit bound 1568 B):
- **Advertising:** on once the feeder has been without DNS-checked connectivity (`net_status`) for 120 s since it was last online or since the stack came up; off as soon as it is online again.
- **Idle hosts:** a connection that has not finished within 60 s is dropped.
- **Records:** `PLBLE1` lines `ready heap_free=…`, `advertising on=…`, `connect`, `mtu`, `request op=… ok=…`, `drop reason=…`, `disconnect` and `failed stage=…`. None carries a credential.

## Setup exchange

The UUIDs and framing are in `petlibre_core::ble_setup`. Service `504c5345-5455-5000-8000-000000000000` has one control characteristic (`…0001`, write and notify). Every message in either direction is a u16 little-endian length, then that many bytes of JSON, at most 448. Each side reassembles the pieces itself, so nothing depends on the negotiated MTU.

1. The host enables control notifications. The feeder draws a fresh `request_auth::Session` and notifies `{"op":"hello","nonce":"<16 hex>"}`.
2. The host sends one signed request under that nonce, built and signed exactly as the server does over MQTT: `{"id":…,"op":"wifi","ssid":<hex>,"psk":<hex>,"nonce":…,"seq":1,"mac":…}`, or `"clear":1`, or no fields to read.
3. The feeder replies with the transport's own `wifi` reply:
   - credentials go to the same queue (`network::request`) and the settings writer saves them;
   - the reply never echoes the passphrase;
   - each hello allows one request, so a replayed or reused request is refused.
4. When the writer publishes a new store generation, the Wi-Fi task's next join reads the stored credentials again and gives them a fresh 5 attempts before the build's take over (`network.rs`). Offline, the next attempt comes within the 30 s backoff.

Refusals:
- A bad or missing signature gets `{"ok":false,"error":"auth"}`.
- Any other operation gets `unsupported`.
- An unsigned or malformed message, or one over 448 bytes, drops the link.
- An image without a request key drops every link.

Nothing over BLE can feed, play or change any other setting.

Host: `petlibre-local ble-wifi --ssid NAME` reads the passphrase from stdin. `--clear` returns to the build's credentials, and with neither flag it reads which are in use. It goes through BlueZ (`bluer`) and finds the feeder by its service UUID among devices currently advertising, or by `--address`.

## Evidence

- **Physical (Oct 23), this build** (`fw34-setup-01`–`03`, OTA v70):
  - With the hotspot down the feeder advertised about 124 s after boot (`ready heap_free=131072`).
  - `petlibre-local ble-wifi` sent the hotspot's credentials. The reply was `ok:true, pending:true`, and the UART showed `request op=wifi ok=1`, then `PLCFG1 net saved generation=16 stored=1`; the link stayed up through the NOR write.
  - When the hotspot came up, join attempt 14 connected with source `stored`, after 13 failed attempts on the build's, and advertising stopped once DNS checked.
  - `bluetoothd` 5.87 on the host segfaulted twice around a filtered discovery, so the client checks the service UUID itself.
- **Physical (Oct 23), earlier BLE speaker build:**
  - The stack came up beside Wi-Fi on the research unit (`PLBLE1 ready heap_free=…`, `BT INIT success`), and BlueZ saw it advertising.
  - Links settled at MTU 247, data length 251, 2M PHY and a 7.5–15 ms interval.
  - A signed 10 s stream played clean.
  - With BLE running, the smoke tier passed: OTA install and confirmation, signed requests, refusals, and two settings saves to NOR.
  - That audio demo has since been removed for this setup role.
- **Host:** `petlibre-hal/tests/ble.rs` covers:
  - advertising timing;
  - a signed `wifi` request queuing credentials;
  - wrong-key, stale-nonce and non-`wifi` refusals;
  - malformed and oversized drops;
  - the radio failure.

  `petlibre-hal/tests/network.rs` covers the mid-boot save being joined with a fresh count.

## Open questions for the bench

- **Setup end to end:** an offline feeder advertises after 2 minutes, takes credentials, and joins.
- **Flash writes with BLE up:** the settings writer and confirmation record mask interrupts during a sector operation. Watch for HCI disruption.
- **Heap:** `ready heap_free` read 45–70 KB with the speaker build.
- **Task stacks:** the C-created BT tasks (`bte_task`, `trace_task`, the HCI send task and `petlibre_ble`) are outside the stack audit. They rest on the SDK's sizes and the heap figure.
- **Power:** stock advertises only on mains power; this build does not yet look at PA19.
