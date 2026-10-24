# Device layer (FW50)

Status: static analysis (October 23, 2026); nothing ran on the feeder. This audits the SDK, the HAL and the runtime for PLAF103 assumptions, so that a second feeder model knows what it has to supply. It does not support a second model. Each fact is one of:
- **board:** another feeder would differ;
- **chip:** any AmebaD board shares it;
- **unit or stock:** tied to the research unit or to stock 1.1.15.

## Where the facts live now

| What | Where | Kind |
| --- | --- | --- |
| Every GPIO the ports use (motor, emitters, sensor inputs, buttons, indicators, amplifier) and the board's ADC channels | `petlibre_amebad_sys::board` | board |
| Flash layout: OTA banks, stock's bank table, preserve ranges | `src/firmware/devices/plaf103/layout.json`, read by the builder and its audits | unit and stock |
| The replacement's storage region | `STORAGE_OFFSET` and `STORAGE_SECTORS` in `petlibre-amebad-sys` | unit |
| Build inputs: stock image, C entry and BLE ports, first-boot mains | `device.json` | board |

**Single source:** before FW50 the same pin numbers sat in up to six places (the HAL's `board.rs`, `stopped.rs`, the feedback IRQ, `power.rs`, `buttons.rs`, `leds.rs`, `audio.rs`, three probes and the fake). Each now reads `board`, and the bit masks (`MOTOR_MASK`, `IR_PA_BIT`, `IR_PB_BIT`) are derived from it. A second board would start as a second module of the same shape, chosen at build time; nothing reads `board` at run time.

**Why not `device.json`:** the pins are compile-time constants inside `no_std` firmware, and several of them feed audited code: the stopped-output check, the panic and RAM-park output writes, and the KM0 pad-table audit. Generating them from JSON would put a code generator in front of audited binaries for no gain while there is one board. `device.json` stays the description of what the builder needs.

## Known limits

These stay where they are and are listed here instead. A second model would revisit each one.

**Board, in code that is not the port layer:**
- **Panel layout:** `petlibre_core::buttons::Buttons::from_port_b` maps PB4–PB7 to reset, mic, dispense and lock, active LOW. Core is hardware-independent and cannot depend on the SDK bindings; `board::BUTTONS` is the same order.
- **Motor gate:** `petlibre_core::feed::motor_gate::MotorWritePair` names its fields `pa13_high` and `pa14_high`. It is part of the C ABI, so renaming them is an ABI change.
- **Motor driver:** the TC118AH's truth table (both inputs HIGH is brake, both LOW is refused) is in `petlibre_hal::board::motor_pair`. Which drive direction dispenses is still open (FW16).
- **Beams:** `petlibre_core::feed::sensor_inputs` holds their polarity and which is food and which is outlet.
- **Stopped outputs:** `petlibre_hal::stopped` defines stopped as motor HIGH, emitters LOW and SWD released. The raw-register copies in `target.rs` (`panic_outputs_safe`, `ram_outputs_safe`) use the derived masks, and the RAM-park audit checks the latter as leaf code.
- **Shared audio pads:** `firstboot_km0.c` powers the audio-shared pads so PA0–PA7 work as GPIO, and the stock pad table is checked by `audit/pinmap.rs`.
- **Limits:** the probes' and simulator's feed and sensor limits (300 mA, battery 1650 scaled, settle times) are bench values for this board.

**Unit or stock:**
- **Battery divider:** `DIVIDER_PER_MILLE` (2063) was measured on the research unit.
- **Indicator readback:** PA0–PA7 read back LOW on the tested unit, so only PB22 has input readback.
- **Flash ID:** `c84017` (GigaDevice, 8 MiB), in the flash probe.
- **Settings binding:** `SETTINGS_BINDING` names the research unit's installation.
- **Bank addresses:**
  - **Rust:** `petlibre-amebad-sys`, `core::ota` (const-asserted equal), `FlashMmuEntry::bank`, `petlibre-flash`, `petlibre-emu` and `review.rs`.
  - **C:** `firstboot_km0.c`, which also holds the confirmation sector.
  - **Layout file:** `layout.json`, the only data-driven copy, which the builder patches into the SDK's OTA addresses.
- **BLE system-data patch:** the builder pins `check_sw()` to an erased sector's value, as the research unit's is.
- **Stock image constants:** the SHA-256 and the bank-table and pad-table addresses in the builder's audits.

**Chip, not board:** ADC channel 10 (the internal supply), the ADC eFuse calibration, SWD pad muxing, the audio codec rates, and the `81958711` bank marker.

## For a second model

A new feeder needs:
1. **Board:** a `board` module of its own: pins, ADC channels and polarities.
2. **Layout:** a `layout.json` from its stock image.
3. **Storage:** a checked storage region.
4. **Description:** a `device.json` naming its ports.
5. **Firmware:** a thin firmware crate ([product architecture](product-architecture.md)).

Everything in the known limits above then needs a decision. The panel mapping and the motor gate's names are the first two, since core and the C ABI carry them.
