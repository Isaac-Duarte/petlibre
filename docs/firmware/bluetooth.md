# Bluetooth LE and BLE setup mode (FW34, FW45)

Stock 1.1.15 uses Bluetooth LE to give an offline feeder its Wi-Fi and MQTT settings from the vendor app ([stock behavior](../devices/plaf103/stock/README.md#bluetooth-le)). The `feeder` firmware keeps that role in **BLE setup mode** (FW45):
- **When:** the owner holds Lock, then Mic, for 5 s on an unlocked panel. A normal boot never starts the BLE stack.
- **What:** the owner's phone (or `wellfed setup`) runs a Noise XX handshake with the feeder and asks, encrypted, for Wi-Fi and enrollment. A Dispense press confirms, and the phone's key becomes an owner key.
- **Then:** the feeder saves the Wi-Fi for its next join and reboots without BLE.

FW34 brought the stack up in a `ble` bring-up firmware, which sent signed FW25 `wifi` requests in the clear; FW45 retired both on October 24, 2026.

The AmebaD's Bluetooth is LE only. The SDK's `bt_flags.h` sets `F_BT_BREDR_SUPPORT 0`, and the stack has no A2DP or LE Audio, so the feeder never appears as an audio device to a phone.

## Build

A firmware opts in with `bluetooth = true` in `[package.metadata.wellfed]`, and the device names its BLE source (`"bluetooth"` in `device.json`). Both are identity inputs. Only such a firmware gets these overlays; every other build restores the pinned files, so every other firmware's image is unchanged:

- **`.config`:** sets only `CONFIG_BT_EN=y`, which builds the BT board code and links `btgap.a`. The C macro `CONFIG_BT` stays undefined, so the boot-time FTL init, the BT AT commands and the vendor examples stay out.
- **`ftl.c`:** dropped from its makefile. The stack's bond store is FTL, which erases and programs `0x102000–0x105000`, inside OTA bank 0. `wellfed_ble.c` defines the two FTL entry points the stack calls, and both refuse.
- **`check_sw()` in `hci_board.c`:** pinned to an erased sector's value. The board code reads active-low switches from system data at flash `0x3028`, and those switches choose:
  - which controller patch loads (the embedded one, or one read from a fixed address inside our banks);
  - whether PA26 becomes a trace UART;
  - whether the controller logs;
  - whether the host stack starts at all.

  The research unit's word is `0xFFFFFFFF` (static, from the Oct 22 full dump), so the pin matches it. The image then never depends on a unit's system data, and this read takes no flash lease.
- **`wellfed_ble.c`:** staged beside `example_entry.c` and added to the example makefile.

The link audit is unchanged and passes: `ftl_init` and every flash erase and program routine stay absent, and no new eFuse writer is linked (`bt_write_phy_efuse_value` has no caller and is collected). The image carries the 11.5 KB embedded controller patch (`rtlbt_fw`). The first-boot image is 832 KB, well within a bank.

## Runtime

`wellfed_ble.c` starts one task (`wellfed_ble`, 1024 words, priority 1). It waits up to 60 s for the Wi-Fi shim's radio, because `hci_board_init` refuses before `wifi_on`. It then brings the stack up in the vendor example's order and runs the GAP message loop:
- **One link.** `gap_config_le_key_storage_flag(0)` keeps no keys, and BLE pairing is off: a just-works request is rejected. Access control is the Noise handshake and the Dispense press.
- **Advertising:** only while Rust asks for it. It is connectable, about 100 ms, with the 128-bit service UUID; the scan response carries the name `Wellfed`.
- **GATT callbacks** only copy bytes into rings (control 1 KiB, outgoing notifications 512 B) and record events.
- **Rust requests** (notify, advertise, disconnect) go through the task's IO queue, so every stack call runs on the task that started the stack.

The Rust side is the `feeder` firmware's loop task (`feeder/src/setup.rs`), which steps `wellfed_hal::ble::BleSetup`, the link:
- **Started** only once setup mode begins: the stack's heap and C tasks cost nothing on a normal boot (FW44).
- **Advertising:** while the power monitor reports the adapter (mains, as stock), and never while connected or after setup has finished.
- **Messages:** the link reassembles a host's framed writes into one held message and frames each reply; what the messages say is the loop's.
- **Idle hosts:** a connection that has not finished within 150 s is dropped (the handshake, then up to a minute for the press).
- **End:** about 3 s after `Done`, or after 15 minutes with no session, the loop reboots through the watchdog. KM0 counts that reset only for an unconfirmed build.
- **Records:** `PLBLE1` lines `ready heap_free=…`, `advertising on=…`, `connect`, `mtu`, `drop reason=…`, `disconnect` and `failed stage=…`; `PLSET1` lines `start`, `session`, `asked wifi=…`, `enroll result=…`, `wifi saved=…`, `reply …`, `fault stage=…` and `reboot ended=…`. None carries a key, a label or a credential.

## Setup exchange

The UUIDs and framing are in `wellfed_core::ble_setup`. Service `504c5345-5455-5000-8000-000000000000` has one control characteristic (`…0001`, write and notify). Every message in either direction is a u16 little-endian length, then that many bytes, at most 448. Each side reassembles the pieces itself, so nothing depends on the negotiated MTU.

The messages are `wellfed_proto::setup` (postcard), carried by Noise XX with the prologue `wellfed-setup-v1`. The client is the initiator and the feeder the responder under its identity key ([pairing](pairing.md)):
1. The client enables notifications and sends XX's first message (its ephemeral key).
2. The feeder answers with the second, which carries its identity key encrypted. The client now knows the feeder's key; its first 8 bytes match the feeder's `PLID1` fingerprint.
3. The client sends the third message: its own static key, and its `Request` (a label and optional Wi-Fi) as the encrypted payload. The passphrase never crosses the air in the clear.
4. The feeder checks the label and credentials against the stores' rules, offers the key to pairing, and replies `Confirming { seconds: 60 }`.
5. A Dispense press within 60 s admits it. The settings writer enrolls the key as an owner, then saves the Wi-Fi. The feeder replies `Done { enrolled, wifi }` and reboots without BLE about 3 s later. The Wi-Fi task's next join, after the reboot, uses the saved credentials.

Refusals, each followed by the link closing (setup mode stays for another try):
- No press in time: `Refused(Unconfirmed)`. Another ask already waiting: `Busy`. Eight clients already: `Full`. A store that cannot be written: `NotWritable`. A label or credentials the stores refuse: `Invalid`.
- A failed handshake (another protocol or version, tampering), a message after the ask, or one over 448 bytes drops the link without a reply.
- A client that disconnects while its ask waits cancels it, so the next Dispense press feeds again. One that leaves after its press still gets enrolled and its Wi-Fi saved, and a client connecting meanwhile is dropped (`fault stage=busy`) until that finishes.

Nothing over BLE can feed, play or change any setting but Wi-Fi and the client list.

Host: `wellfed setup --ssid NAME` (the owner's CLI) reads the passphrase from stdin, creates the computer's client key in `private/wellfed-owner.json` on first use, and records the feeder's identity there after `Done`. It goes through BlueZ (`bluer`) and finds the feeder by its service UUID among devices advertising now, or by `--address`. The protocol is `wellfed-client`'s, which the Android app shares.

## Evidence

- **Physical (Oct 23), the retired FW34 `ble` firmware** (`fw34-setup-01`–`03`, OTA v70; its HMAC-signed exchange is gone):
  - With the hotspot down the feeder advertised about 124 s after boot (`ready heap_free=131072`).
  - `wellfed-local ble-wifi` sent the hotspot's credentials. The reply was `ok:true, pending:true`, and the UART showed `request op=wifi ok=1`, then `PLCFG1 net saved generation=16 stored=1`; the link stayed up through the NOR write.
  - When the hotspot came up, join attempt 14 connected with source `stored`, after 13 failed attempts on the build's, and advertising stopped once DNS checked.
  - `bluetoothd` 5.87 on the host segfaulted twice around a filtered discovery, so the client checks the service UUID itself.
- **Physical (Oct 23), earlier BLE speaker build:**
  - The stack came up beside Wi-Fi on the research unit (`PLBLE1 ready heap_free=…`, `BT INIT success`), and BlueZ saw it advertising.
  - Links settled at MTU 247, data length 251, 2M PHY and a 7.5–15 ms interval.
  - A signed 10 s stream played clean.
  - With BLE running, the smoke tier passed: OTA install and confirmation, signed requests, refusals, and two settings saves to NOR.
  - That audio demo has since been removed for this setup role.
- **Host (Oct 24, FW45):**
  - `wellfed-client/tests/setup.rs`: the client against the feeder's responder through BLE framing (both keys exchanged, the ask decoded and checked), refusals, a tampered handshake and a foreign prologue failing closed.
  - `feeder/tests/setup.rs`: setup mode in the loop against the fake peripheral and a scripted writer (press, enroll, Wi-Fi save, `Done`, reboot); an unanswered ask; a client that leaves; a foreign first message; mains-only advertising and the idle reboot.
  - `wellfed-hal/tests/ble.rs`: advertising on request only, framed messages held until consumed, oversized frames and long sessions dropped, the radio failure.
  - `wellfed-hal/tests/network.rs` covers the mid-boot save being joined with a fresh count.

## Open questions for the bench

- **Setup end to end (FW45):** the Lock and Mic hold starts setup mode, `wellfed setup` and a Dispense press enroll the key and save the Wi-Fi, and the feeder reboots and joins with `source: stored`.
- **Flash writes with BLE up:** the settings writer and confirmation record mask interrupts during a sector operation. Watch for HCI disruption.
- **Heap:** `ready heap_free` read 45–70 KB with the speaker build.
- **Task stacks:** the C-created BT tasks (`bte_task`, `trace_task`, the HCI send task and `wellfed_ble`) are outside the stack audit. They rest on the SDK's sizes and FW44's setup-mode heap figure.
- **Starting the stack beside a joined Wi-Fi:** FW44 started it at boot; setup mode starts it later, with Wi-Fi already up.
