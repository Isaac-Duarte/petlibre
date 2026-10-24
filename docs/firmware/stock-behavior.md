# Stock behavior and local protocol

Findings apply to the inspected PLAF103 stock **1.1.15** image and research unit. Other models/releases may differ. Instruction-level behavior is distinguished from physical acceptance in the [hardware guide](../hardware/README.md).

## Network protocol

The observed feeder uses plaintext MQTT on TCP 1883 with device-specific credentials. Its initial CONNECT uses legacy MQTT 3.1 (`MQIsdp`); the server normalizes that CONNECT for an MQTT 3.1.1 broker and forces a clean session. It preserves the credentials, client identifier, keepalive, and later packets. Topic access stays within the configured device namespace.

Topics have the form `dl/PLAF103/<device-id>/<group>/<event>/post` or `/sub`. The local server handles startup, attributes, heartbeat, clock, binding, captured plan replies, and manual feeding. Consult the Rust protocol tests for complete synthetic wire examples. The API is described in [local-server setup](../development/local-server.md).

The broker is authenticated, has no upstream bridge, and uses nonpersistent sessions. Manual feed commands have durable caller UUIDs, QoS 0, and no retained messages. Replies must correlate to the request ID. An uncertain operation is not automatically republished. Stock firmware's limited raw-payload/result caching is not a substitute for durable no-replay handling.

## Feeding and device features

Recovered feeding code tracks feedback events, logical direction, single/double-bowl progress, direction pauses, and reverse recovery. The replacement controller uses bounded deadlines and fault stops; its policy is not a byte-for-byte reimplementation of every stock branch. Physical quantities and mechanical mapping remain uncalibrated.

Food/outlet optical filtering, power-dependent emitter duty, current estimates, battery lookup logic, four button roles, indicator selection, and audio completion paths have been investigated. A recovered branch does not establish physical signal polarity, actual timing, or complete feature support in replacement firmware.

## Panel buttons and child lock

Static analysis and offline emulation of the stock 1.1.15 panel handlers (October 18, 2026; nothing run on the feeder). Stock's event task runs its button handler once per loop with a 50-tick delay at 1 kHz, and every boundary below is a count of those visits, so the durations are nominal: handler and scheduling time add to them, and none was measured on the feeder.

| Behavior | Stock boundary | Nominal |
| --- | --- | --- |
| Lock (PB7) held toggles the panel lock, once per hold | 100th pressed visit | About 5 s |
| An unlocked panel relocks | 600th idle visit | About 30 s |
| Reset (PB4) held on an unlocked panel starts the factory restore | 100th pressed visit | About 5 s |
| Dispense (PB6) asks for one feed, once per press | The pressed visit | — |

- **Locked:** Dispense, Mic (PB5) and the Reset hold do nothing. Lock still toggles, so the panel can always be unlocked.
- **Idle:** activity on Mic, Reset or Dispense restarts the relock count.
- **Indicator:** PA4 is driven HIGH while locked and LOW while unlocked. The `lights` setting off drives every indicator LOW.
- **App setting:** the PLAF103 has no `childLock*` fields; the vendor app reports `childLockSwitch` and both lock durations as `null` for it. Its app-side setting is `disableHardwareButton` (configuration `+0x91`), which disables the panel except for a recovery sequence: Lock held about 3 s, then Reset held about 3 s, starts the factory restore.
- **Unknown:** the lock state at boot. Its flag's address falls inside the RAM image's code, so its initial value was not recovered.

**Replacement (FP11, physical on October 23 with the owner pressing; host-tested):** `wellfed-feeder`'s `panel` keeps these rules over the SDK button service's gestures, with one 5 s hold threshold, and the `feeder` firmware shows the lock on PA4. Choices where stock is unknown or differs:
- The panel starts locked at every boot, so an unattended reboot never leaves Dispense live.
- Any gesture, Lock included, restarts the relock, and a button still held holds it off until its release. Stock counts only Mic, Reset and Dispense.
- A signed `lock` request locks or unlocks from the network ([local transport](local-transport.md#child-lock-fp11)). A remote unlock relocks on the same 30 s timer.
- `disableHardwareButton` and its recovery sequence are not implemented; FP15 owns resets.

## Bluetooth LE

Static analysis of the 1.1.15 KM4 image and the vendor app shows that stock links the full Realtek BLE stack, with its embedded controller patch and Wi-Fi coexistence. It runs as a GATT peripheral for two jobs only:
- **Setup:** the app writes Wi-Fi and MQTT settings and binds the device, then reads the result. It uses a 16-bit service `ffee` with characteristics `ffe1`–`ffe4`.
- **Offline diagnostics:** after about two minutes offline the feeder advertises and sends a compact report by notification, then stops advertising once it is back online.

Advertising runs only on mains power. Nothing found ties BLE to feeding, schedules or settings commands, which go over MQTT. Pairing appears only as SDK plumbing; the setup flow seems to rely on plain GATT with app-side encryption. Bond storage uses the SDK's FTL flash layer, whose sector was not located. The replacement's own BLE design is in [Bluetooth](bluetooth.md).

## Clock and schedules

The server responds with the captured authoritative plan snapshot; it does not expose schedule editing or automatically push plans. Nonempty plans remain unvalidated for this release, and clock calibration is held when plans exist. Responses use the configured IANA timezone, including DST transition information. The full stock timezone/timer activation and cloud plan-construction contracts remain unresolved.

Replacement schedules still need durable occurrence identities, timezone/DST policy, restart reconciliation, and explicit handling of interrupted meals. A successful manual feed does not establish scheduled or unattended reliability.

## Reliability limits

A stock-only server-restart experiment reproduced repeated startups and a KM4 watchdog reset indication. The exact stall trigger is unresolved. WAN isolation, sustained reconnect, cold boot, time recovery, battery behavior, and portion weight require independent physical acceptance. Home Assistant integration remains deferred.
