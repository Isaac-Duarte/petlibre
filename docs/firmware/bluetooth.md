# Bluetooth LE and setup mode

This page explains how the feeder uses Bluetooth LE: only to set itself up, only when you ask or it has no network, and never as an audio device. It is for owners setting up a feeder and for contributors working on the setup path. How keys and pairing work in general is in [pairing](pairing.md).

## The one idea

Bluetooth is off unless you turn it on with the buttons, or the feeder has no network to join. Stock 1.1.15 uses Bluetooth LE to give an offline feeder its Wi-Fi and MQTT settings from the vendor app ([stock behavior](../devices/plaf103/stock/README.md#bluetooth-le)). The `feeder` firmware keeps that job in **setup mode**:

- **When:** you hold Lock, then Mic, for 5 s on an unlocked panel. A feeder with no build Wi-Fi and none stored (a consumer image, fresh from stock or after a factory reset) also starts setup mode at boot, unless it has an enabled meal plan. A boot with a network to join never starts the Bluetooth stack.
- **What:** your phone (or `wellfed setup`) runs a Noise XX handshake with the feeder and shows a one-time code of 6 button presses. You key it in on the feeder, which ticks for each press. Both sides prove the code to each other, and only then does the phone send its Wi-Fi, and its key becomes an owner key.
- **Then:** the feeder saves the Wi-Fi for its next join, and reboots without Bluetooth.

The AmebaD's Bluetooth is LE only. The SDK's `bt_flags.h` sets `F_BT_BREDR_SUPPORT 0`, and the stack has no A2DP or LE Audio, so the feeder never appears to a phone as an audio device.

Nothing over Bluetooth can feed, play a sound, or change any setting except Wi-Fi and the client list.

## Setting up from a computer

`wellfed setup --ssid NAME` is the owner's CLI for setup mode.

- It reads the Wi-Fi passphrase from stdin.
- On first use it creates the computer's client key in `private/wellfed-owner.json`.
- It prints the code with each button's position and name.
- After `Done` it records the feeder's identity in the same file.

It goes through BlueZ (`bluer`), and finds the feeder by its service UUID among devices advertising now, or by `--address`. The protocol is `wellfed-client`'s, which the Android app shares.

> [!NOTE]
> The CLI's own search can miss a feeder that is advertising. If it does, run `bluetoothctl scan le`, then pass the feeder's address with `--address`.

## Setup exchange

The UUIDs and framing are in `wellfed_core::ble_setup`. Service `504c5345-5455-5000-8000-000000000000` has one control characteristic (`…0001`, write and notify). Every message in either direction is a u16 little-endian length, then that many bytes, at most 448. Each side reassembles the pieces itself, so nothing depends on the negotiated MTU.

The messages are `wellfed_proto::setup` (postcard), carried by Noise XX with the prologue `wellfed-setup-v2`. The client is the initiator, and the feeder the responder under its identity key:

1. The client enables notifications and sends XX's first message (its ephemeral key).
2. The feeder answers with the second, which carries its identity key, encrypted.
3. The client sends the third message: its own static key, with its `Request` (a label) as the encrypted payload.
4. The feeder checks the label, offers the key to pairing, and replies `EnterCode { seconds: 60 }`. The client shows its code: 6 presses, each one of the four buttons, numbered from the left (1 Lock, 2 Dispense, 3 Mic, 4 Reset).
5. The owner keys the code in within 60 s. Pairing takes every gesture until the code is complete. Each single-button press counts; chords and holds count for nothing. Nothing reaches the panel, so no press feeds, toggles the lock or resets the keys. The speaker ticks (`key`) for each press that counts.
6. The feeder and the client prove the code to each other (`wellfed_session::setup_code`, [below](#why-a-code)), within 15 s of the last press.
7. Once the feeder has proven the code, the client's last step carries the Wi-Fi. When that step opens, the settings writer enrolls the key as an owner, then saves the Wi-Fi. The feeder replies `Done { enrolled, wifi }` and reboots without Bluetooth about 3 s later. The Wi-Fi task's next join, after the reboot, uses the saved credentials. Only now does the client trust the feeder's key; its first 8 bytes match the feeder's `PLID1` fingerprint.
8. The speaker plays `coin` once the feeder replies `Done`, and `alert` when the code was wrong or never finished. The `sound` setting off mutes all three.

**Refusals.** Each is followed by the link closing; setup mode stays on for another try.

| Case | Answer |
| --- | --- |
| Code not keyed in, or the proof not finished, in time | `Refused(Unconfirmed)` |
| The client's proof failing at the feeder | `Refused(WrongCode)` |
| Another ask already waiting | `Busy` |
| Eight clients already | `Full` |
| A store that cannot be written | `NotWritable` |
| A label or credentials the stores refuse | `Invalid` |

- **The feeder's proof failing at the client:** the client stops without sending its Wi-Fi, and reports a mismatch (mistyped, or another device answering). The feeder then times the proof out.
- **A failed handshake** (another protocol or version, tampering), a message out of turn, or one over 448 bytes drops the link without a reply.
- **A client that disconnects** while its ask waits for the code cancels it, so the next Dispense press feeds again. One that leaves after the proof still gets enrolled and its Wi-Fi saved. A client connecting meanwhile is dropped (`fault stage=busy`) until that finishes.
- **A second client** whose ask arrives while an earlier client's link has vanished replaces that stale offer (`a_new_client_replaces_an_ask_whose_link_vanished`).

### Why a code

XX alone trusts whichever device answers. With only a Dispense press as the check, a device in radio range during the window could relay between the phone and the feeder. The phone would see the relay's key as the feeder's, the press would enroll the relay as an owner, and the relay would read the Wi-Fi passphrase. The feeder has no display for a comparison, so the owner carries a secret the other way: from the phone's screen to the feeder's buttons.

**The proof** is modeled on Bluetooth's passkey entry:

- **Bound to the handshake.** Both sides bind every step to their own Noise handshake hash. A relay has a different handshake on each leg, so a step from one leg never opens on the other.
- **One press at a time.** A proof of the whole code at once could be tried against every code offline. So for press `i`, each side sends a commitment: HMAC-SHA256, keyed by a fresh nonce, over the side, `i`, the press and the hash. It opens it (sends the nonce) only after the other side's commitment to press `i` has arrived. The feeder opens first. Each message opens the sender's last commitment and commits to its next press, so the proof is 7 messages each way.
- **The odds.** A relay must commit to each press before it learns it, on one leg or the other. Failing one leg teaches it one press. So it fools the phone with odds of 4^-5 (1 in 1,024), and gets enrolled with odds of 4^-6 (1 in 4,096), once per code the owner keys in. Six presses trade some margin for a code that is quick to key in; `PRESSES` sets it. Each code is used once.
- **The secret goes last.** The client sends the Wi-Fi only in its last step, after the feeder's last opening has matched, and accepts `Done` only after that. A relay that fails the proof gets neither the passphrase nor a believed `Done`.

## Runtime

`wellfed_ble.c` starts one task (`wellfed_ble`, 1024 words, priority 1). It waits up to 60 s for the Wi-Fi shim's radio, because `hci_board_init` refuses before `wifi_on`. It then brings the stack up in the vendor example's order and runs the GAP message loop.

- **One link, no bonding.** `gap_config_le_key_storage_flag(0)` keeps no keys, and BLE pairing is off: a just-works request is rejected. Access control is the Noise handshake and the owner's code.
- **Advertising** happens only while Rust asks for it. It is connectable, about every 100 ms, with the 128-bit service UUID. The scan response carries the name `Wellfed`.
- **GATT callbacks** only copy bytes into rings (control 1 KiB, outgoing notifications 512 B) and record events.
- **Rust requests** (notify, advertise, disconnect) go through the task's IO queue, so every stack call runs on the task that started the stack.

The Rust side is the `feeder` firmware's loop task. Its setup mode (`wellfed_feeder::feeder::setup`) steps `wellfed_hal::ble::BleSetup`, the link, through the firmware's `Platform`:

- **Started** only once setup mode begins, so the stack's heap and C tasks cost nothing on a boot with a network.
- **At boot:** the loop decides once per boot. It waits until the settings writer has loaded the Wi-Fi store and can write it, and has loaded the plans. Then it starts setup mode if the image has no build Wi-Fi (`Platform::build_wifi`, from the runtime's `BUILD_WIFI`), nothing is stored and no plan is enabled. A boot a reset storm holds waits for its hold to end. Credentials cleared later in the boot start nothing until the next boot.
- **Advertising** only while the power monitor reports the adapter (mains, as stock does), and never while connected or after setup has finished.
- **Messages:** the link reassembles a host's framed writes into one held message and frames each reply. What the messages say is the loop's business.
- **Idle hosts:** a connection that has not finished within 150 s is dropped (the handshake, then up to a minute for the code).
- **The end:** about 3 s after `Done`, or after 15 minutes with no session, the loop reboots through the watchdog. The restart is deliberate, so it ends KM0's reset-storm count, however soon after the boot it comes; only an unconfirmed build's trial counts it. An unprovisioned feeder that idles out enters setup mode again on the next boot. The closing restart waits for a feed in flight.
- **Records:** `PLBLE1` lines `ready heap_free=…`, `advertising on=…`, `connect`, `mtu`, `drop reason=…`, `disconnect` and `failed stage=…`; `PLSET1` lines `start cause=hold|unprovisioned`, `session`, `asked wifi=…`, `enroll result=…`, `wifi saved=…`, `reply …`, `fault stage=…` and `reboot ended=…`. None carries a key, a label or a credential.

**The speaker in setup mode.** The feeder's two firmware tasks are taken, so setup mode's loop steps the speaker itself. Linking the speaker into the feeder costs about 9 KB of flash and 4.8 KB of static RAM, mostly the codec's DMA ring. Setup mode's state lives in the owner's static, built at compile time: built on the loop's stack, it pushed the stack bound to 8,228 of 6,144 bytes. With it static, the loop's bound was 4,396 bytes, under the audit's limit of 4,608.

## Build

A firmware opts in with `bluetooth = true` in `[package.metadata.wellfed]`, and the device names its BLE source (`"bluetooth"` in `device.json`). Both are identity inputs. Only such a firmware gets these overlays; every other build restores the pinned files, so every other firmware's image is unchanged.

- **`.config`** sets only `CONFIG_BT_EN=y`, which builds the BT board code and links `btgap.a`. The C macro `CONFIG_BT` stays undefined, so the boot-time FTL init, the BT AT commands and the vendor examples stay out.
- **`ftl.c`** is dropped from its makefile. The stack's bond store is FTL, which erases and programs `0x102000–0x105000`, inside OTA bank 0. `wellfed_ble.c` defines the two FTL entry points the stack calls, and both refuse.
- **`check_sw()` in `hci_board.c`** is pinned to an erased sector's value. The board code reads active-low switches from system data at flash `0x3028`, and those switches choose:
  - which controller patch loads (the embedded one, or one read from a fixed address inside our banks);
  - whether PA26 becomes a trace UART;
  - whether the controller logs;
  - whether the host stack starts at all.

  The research unit's word is `0xFFFFFFFF`, so the pin matches it. The image then never depends on a unit's system data, and this read takes no flash lease.
- **`wellfed_ble.c`** is staged beside `example_entry.c` and added to the example makefile.

The link audit is unchanged and passes: `ftl_init` and every flash erase and program routine stay absent, and no new eFuse writer is linked (`bt_write_phy_efuse_value` has no caller and is collected). The image carries the 11.5 KB embedded controller patch (`rtlbt_fw`).

## What's open

- **Not yet on hardware:** a deliberately mistyped code (`wrong-code` at the CLI, then `refused reason=unconfirmed` and `alert` after 15 s), and the stale-offer fix. Host tests cover both.
- **CLI discovery:** why the CLI's search misses an advertising feeder.
- **Flash writes with BLE up:** the settings writer and the confirmation record mask interrupts during a sector operation. Watch for HCI disruption.
- **Heap:** `ready heap_free` read 45–70 KB with an earlier speaker build.
- **Task stacks:** the C-created BT tasks (`bte_task`, `trace_task`, the HCI send task and `wellfed_ble`) are outside the stack audit. They rest on the SDK's sizes and the setup-mode heap figure in the [resource budget](resource-budget.md#ram).
- **Starting the stack beside a joined Wi-Fi:** the resource budget started it at boot; setup mode starts it later, with Wi-Fi already up.
- **Setup at an unprovisioned boot:** host-tested and simulated only. No consumer image has started it on a feeder, and its heap has been measured from bank 1 only, never from bank 0, where stock installs the image. The stack needs `wifi_on` first, which a consumer image's Wi-Fi task still calls with nothing to join.

## Evidence

- **Modeled** (October 29, 2026), setup at an unprovisioned boot: the firmware's platform on the SDK fake starts it once the stores load, and not with stored or build credentials or an enabled plan (`wellfed-fw-feeder/tests/setup.rs`). The simulator, on an image with no build Wi-Fi, starts it, pairs `wellfed-client`'s setup session over a simulated link, saves the Wi-Fi, and starts nothing on the next boot (`wellfed-feeder-sim/tests/setup.rs`).
- **Observed** (October 24, 2026), the setup code (owner pressing, no motion, a bounded `serve` stopped afterwards):
  - `ota fw45-code-02` installed `feeder` v111 (`e5650894`), which confirmed with clean health.
  - `capture fw45-code-05`: the CLI's code keyed in: six `pair setup keyed=` records, each with a `key` tick logged; `pair setup state=entered`, `PLSET1 proving`, `proven wifi=1`. The CLI's key was already enrolled (`result=duplicate`), and the Wi-Fi saved (generation 21): `reply done enrolled=already wifi=saved`, `coin`, the reboot without Bluetooth, the identity unchanged (`8019237d8eba8b9d`, as the CLI recorded), and the rejoin with `source: stored`.
  - `capture fw45-code-06`: every tick was logged as played, but the owner heard none, while the longer `alert` was heard. The amplifier's settle went from 60 to 150 ms and the tick from 50 to 80 ms. `ota fw45-code-07` installed v112 (`bc5561ae`), which confirmed, and the owner then heard the ticks.
  - `capture fw45-code-04` and `-06`: windows that ran out before the sixth press refused with `unconfirmed` and `alert`. Presses after the window went to the locked panel (`ask action=feed result=refused`).
  - `capture fw45-code-09`: a second client connected while the first's code was pending, with no disconnect seen for the first. It got `busy`, and the owner's presses then counted towards an offer nobody held. That led to the stale-offer fix, host-tested only.
  - **CLI discovery:** BlueZ listed the advertising feeder with the setup UUID, but `wellfed setup`'s search missed it twice. `--address` after a `bluetoothctl scan le` connected every time.
  - Afterwards a signed clear emptied the Wi-Fi store (generation 22).
- **Observed** (October 24), setup mode with a Dispense press as the check, before the code replaced it (v108–v109, owner pressing, no motion):
  - `ota fw45-setup-01` installed `feeder` v108 (`46255eec`), which confirmed.
  - `capture fw45-setup-02`: the owner unlocked and held Lock and Mic: `pair setup state=opened`, `PLSET1 start`, `PLBLE1 ready heap_free=104416` (Wi-Fi already up, so lower than the boot-time figure), advertising, then a link at MTU 247. `wellfed setup` found the feeder and finished the handshake (`asked wifi=1`, `reply confirming seconds=60`). After the press: `pair admit path=setup role=owner`, `PLCFG1 keys change=enroll result=done enrolled=1`, `net saved generation=19 stored=1`, `reply done enrolled=new wifi=saved`, `reboot ended=done`. The feeder rebooted with no Bluetooth record, kept `enrolled=1`, and its `PLID1` fingerprint `8019237d8eba8b9d` matched the CLI's. The server saw it online with `source: stored` 23 s into the new boot.
  - `capture fw45-setup-03`: on a locked panel the chord was refused (`refusal=locked`). With no press, the feeder refused after 60 s (`reply refused reason=unconfirmed`), but it dropped the link right after the reply, the CLI never reported it, and the CLI then stalled in BlueZ's disconnect. Fixed in v109 (`fw45-setup-04`): the feeder waits 2 s for the client to hang up before dropping the link, and the CLI bounds its disconnect. In `capture fw45-setup-05` the CLI reported that nobody pressed in time, and setup mode stayed on, advertising again.
  - Afterwards a signed clear emptied the Wi-Fi store (generation 20). The enrolled CLI key stayed.
  - **Mains only** (`fw45-battery-01`–`03`, three D cells): in setup mode, cutting the bench's adapter relay gave `PLBAT1 sample adapter=0`, then `advertising on=0`, with the feeder still running. Restoring it gave `adapter=1`, then `advertising on=1`.
  - With no further session, setup mode rebooted on its own after its 15-minute idle limit (uptime 1,073 s back to 23 s), and the feeder came back online with `source: build`.
- **Observed** (October 23), a retired `ble` bring-up firmware with an earlier, HMAC-signed exchange (`fw34-setup-01`–`03`, OTA v70): with the hotspot down, the feeder advertised about 124 s after boot (`ready heap_free=131072`). The host sent the hotspot's credentials; the UART showed `PLCFG1 net saved generation=16 stored=1`, and the link stayed up through the NOR write. When the hotspot came up, join attempt 14 connected with source `stored`, after 13 failed attempts on the build's. `bluetoothd` 5.87 on the host segfaulted twice around a filtered discovery, so the client checks the service UUID itself.
- **Observed** (October 23), an earlier speaker build with Bluetooth: the stack came up beside Wi-Fi (`PLBLE1 ready heap_free=…`, `BT INIT success`), and BlueZ saw it advertising. Links settled at MTU 247, data length 251, 2M PHY and a 7.5–15 ms interval. A signed 10 s audio stream played clean, and with Bluetooth running, the integration suite's smoke tier passed: OTA install and confirmation, signed requests, refusals, and two settings saves to NOR. That audio demo is removed.
- **Recovered:** the research unit's system-data word at `0x3028`, from the October 22 full dump. The first-boot image with Bluetooth was 832 KB, well within a bank.
- **Modeled** (October 24):
  - `wellfed-session/tests/setup_code.rs`: the proof press by press. One code under one handshake proves both sides; a wrong press fails at that press, before the checking side opens its own; a relay between two handshakes and a reflected message fail at the first opening; turns and message shapes.
  - `wellfed-client/tests/setup.rs`: the client against the feeder's responder through the BLE framing (keys exchanged, the code proven, the Wi-Fi only after); a mistyped code stopping the client before its Wi-Fi; a `Done` without the proof rejected; refusals; a tampered handshake and a foreign prologue failing closed.
  - `feeder/tests/setup.rs`: setup mode in the loop against the fake peripheral and a scripted writer (code keyed in, proof, enroll, Wi-Fi save, `Done`, reboot); a wrong code enrolling nothing and timing out; a tick per press then `coin`, or `alert` for a wrong code, and silence with `sound` off; an unanswered ask; a client that leaves; a foreign first message; mains-only advertising and the idle reboot.
  - `wellfed-hal/tests/ble.rs`: advertising on request only, framed messages held until consumed, oversized frames and long sessions dropped, the radio failure.
  - `wellfed-feeder-runtime/tests/network.rs`: a mid-boot save joined with a fresh count.
