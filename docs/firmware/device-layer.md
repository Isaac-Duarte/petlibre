# Device layer (FW50)

Status: static analysis (October 23, 2026); nothing ran on the feeder. This audits the SDK, the HAL and the runtime for PLAF103 assumptions, so that a second feeder model knows what it has to supply. It does not support a second model. Each fact is one of:
- **board:** another feeder would differ;
- **chip:** any AmebaD board shares it;
- **unit or stock:** tied to the research unit or to stock 1.1.15.

## Where the facts live now

| What | Where | Kind |
| --- | --- | --- |
| Every GPIO the ports use (motor, emitters, sensor inputs, buttons, indicators, amplifier) and the board's ADC channels | `wellfed_amebad_sys::board` | board |
| Flash layout: OTA banks, stock's bank table, preserve ranges | `src/firmware/devices/plaf103/layout.json`, read by the builder and its audits | unit and stock |
| The replacement's storage region | `STORAGE_OFFSET` and `STORAGE_SECTORS` in `wellfed-amebad-sys` | unit |
| Build inputs: stock image, C entry and BLE ports, first-boot mains | `device.json` | board |

**Single source:** before FW50 the same pin numbers sat in up to six places (the HAL's `board.rs`, `stopped.rs`, the feedback IRQ, `power.rs`, `buttons.rs`, `leds.rs`, `audio.rs`, three probes and the fake). Each now reads `board`, and the bit masks (`MOTOR_MASK`, `IR_PA_BIT`, `IR_PB_BIT`) are derived from it. A second board would start as a second module of the same shape, chosen at build time; nothing reads `board` at run time.

**Why not `device.json`:** the pins are compile-time constants inside `no_std` firmware, and several of them feed audited code: the stopped-output check, the panic and RAM-park output writes, and the KM0 pad-table audit. Generating them from JSON would put a code generator in front of audited binaries for no gain while there is one board. `device.json` stays the description of what the builder needs.

## Known limits

These stay where they are and are listed here instead. A second model would revisit each one.

**Board, in code that is not the port layer:**
- **Panel layout:** `wellfed_core::buttons::Buttons::from_port_b` maps PB4–PB7 to reset, mic, dispense and lock, active LOW. Core is hardware-independent and cannot depend on the SDK bindings; `board::BUTTONS` is the same order.
- **Motor gate:** the `wellfed_core::feed::motor_gate::MotorPins` trait names its arguments `pa13_high` and `pa14_high`. It is a Rust trait, so a board-neutral rename is a Rust-only change.
- **Rust interfaces between Rust crates.** The motor pins (`MotorPins`), a store's flash (`store::slot::SlotIo`) and the owner's journal (`feed::storage::OwnerStorage`) are traits the caller's types implement, so each call is direct: the build's stack audit follows it, and no context pointer crosses it. Only what C or the SDK calls stays `extern "C"`: the flash-lock hook provider the RAM interposer installs, the interrupt handlers, and the `wellfed_*` exports.
- **Motor driver:** the TC118AH's truth table (both inputs HIGH is brake, both LOW is refused) is in `wellfed_hal::board::motor_pair`. Which drive direction dispenses is still open (FW16).
- **Beams:** `wellfed_core::feed::sensor_inputs` holds their polarity and which is food and which is outlet.
- **Stopped outputs:** `wellfed_hal::stopped` defines stopped as motor HIGH, emitters LOW and SWD released. The raw-register copies in `target.rs` (`panic_outputs_safe`, `ram_outputs_safe`) use the derived masks, and the RAM-park audit checks the latter as leaf code.
- **Raw outputs are `unsafe`:** in `wellfed-amebad-sys` the GPIO writers (`gpio_port_write`, `gpio_write_bit`, `gpio_init`), the watchdog (`wdg_init`, `wdg_cmd`, `wdg_refresh`) and the flash writers (`storage_erase`, `storage_program`, `bank1_erase`, `bank1_program`, `bank1_give_way`) are `unsafe fn`s whose contract is ownership: the motor pins belong to `board` and `stopped`, the watchdog to the runtime's supervision, a flash write needs the armed hooks' lease. Product firmware reaches them only through those owners. Bring-up firmwares and their probes take the GPIO, watchdog and storage writers from `wellfed_hal::bringup`, inside an `unsafe` block that states why the contract holds; the bank-1 writers stay the updater's and are not re-exported there. `cargo xtask check-repository` refuses a raw writer or `bringup` in product code (the `feeder` firmware's sources and `src/product/`), and a raw writer named other than as `bringup::…` in bring-up code (the other firmwares and the probes).
- **Audio pages:** a `sdk::Audio` handle (one at a time, from `Audio::take`) lends one ring page at a time as a `Page` guard, which `submit` consumes, so no two references to a ring page exist and none outlives the handle.
- **Shared audio pads:** `firstboot_km0.c` powers the audio-shared pads so PA0–PA7 work as GPIO, and the stock pad table is checked by `audit/pinmap.rs`.
- **Limits:** the probes' and simulator's feed and sensor limits (300 mA, battery 1650 scaled, settle times) are bench values for this board.

**Supervision, after the October 25 split:**
- **Shared handles:** `Board` and `Events` are `&self` handles over interior cells, borrowed only inside `IrqGuard` for one call, so the TIM2 and feedback interrupts never alias a task's borrow. A `Board` call nested inside another (from code the first call runs) is refused with `Reentered` instead of being lent the state a second time; on one core, waiting for it would never end. The owner keeps them beside its task state, never inside it. The probes still hold them inside the state their task borrows `&mut` through its `Slot`; they are bench tools, but moving them out the same way is still open.
- **Flash leases during a feed:** the lease provider checks stopped outputs (`stopped::observe`) when it quiesces, and a lease during a feed trips it: the hook halts and the watchdog resets the board, and the owner faults a feed that sees a lease. So no lease starts while a feed may move the outputs. The feed journal writes only at the owner's rest boundaries (the outputs stopped), under the loop's own claim, and every other store write (the settings writer's stores, the confirmation, the give-way) first takes the write turn in `wellfed_hal::interlock`, which waits while the loop holds the outputs ([device API](device-api.md#in-the-firmware)). The provider's request and quiesce run under the same serialization as the board.
- **One watchdog owner:** the runtime feeds the hardware watchdog; the feeding owner only publishes a beat (`supervision::beat`). A fault outside initialization stops the beat, so it resets the board, as the owner's own watchdog did before. The beat promises two gaps: 1 s while outputs may move, and the runtime's 10 s liveness limit at rest, because the Wi-Fi driver's WPA3 handshake held the loop off for over a second on hardware (October 25) and an idle owner then reset the board for nothing.

**Unit or stock:**
- **Battery divider:** `DIVIDER_PER_MILLE` (2063) was measured on the research unit.
- **Indicator readback:** PA0–PA7 read back LOW on the tested unit, so only PB22 has input readback.
- **Flash ID:** `c84017` (GigaDevice, 8 MiB), in the flash probe.
- **Settings binding:** `SETTINGS_BINDING` names the research unit's installation.
- **Bank addresses:**
  - **Rust:** `wellfed-amebad-sys`, `core::ota` (const-asserted equal), `FlashMmuEntry::bank`, `wellfed-flash`, `wellfed-emu` and `review.rs`.
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

Everything in the known limits above then needs a decision. The panel mapping and the motor gate's names are the first two, since core carries them.
