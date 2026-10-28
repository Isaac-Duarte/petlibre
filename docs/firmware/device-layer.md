# Device layer

This page lists what in the firmware is specific to the PLAF103, and where each fact lives. It is for contributors who want to bring Wellfed to a second feeder model. Wellfed does not support a second model yet; this is the map of what one would have to supply.

Each fact is one of three kinds:

- **Board:** another feeder would differ.
- **Chip:** any AmebaD board shares it.
- **Unit or stock:** tied to our research unit, or to stock 1.1.15.

## Where the facts live

| What | Where | Kind |
| --- | --- | --- |
| Every GPIO the ports use (motor, emitters, sensor inputs, buttons, indicators, amplifier) and the board's ADC channels | `wellfed_board_plaf103::pins` | Board |
| Flash layout: OTA banks, stock's bank table, preserve ranges | `src/firmware/devices/plaf103/layout.json`, read by the builder and its audits | Unit and stock |
| The replacement's storage region and who owns each sector | `STORAGE_OFFSET`, `STORAGE_SECTORS` and `storage` in `wellfed-board-plaf103` | Unit |
| Build inputs: stock image, C entry and BLE ports, first-boot mains | `device.json` | Board |

**One source for pins.** The pin numbers used to sit in up to six places: the HAL's `board.rs`, `stopped.rs`, the feedback IRQ, `power.rs`, `buttons.rs`, `leds.rs`, `audio.rs`, three probes and the fake. Each now reads `wellfed-board-plaf103`'s `pins`. The bit masks (`MOTOR_MASK`, `IR_PA_BIT`, `IR_PB_BIT`) are derived from it. `wellfed-amebad-sys` keeps only the SoC's bindings and its fake, which read the board crate for the panic and RAM-park output writes and the storage bounds.

A second board would start as a second crate of the same shape, chosen at build time. Nothing reads the board at run time.

**Why not `device.json`.** The pins are compile-time constants inside `no_std` firmware, and several feed audited code: the stopped-output check, the panic and RAM-park output writes, and the KM0 pad-table audit. Generating them from JSON would put a code generator in front of audited binaries, for no gain while there is one board. `device.json` stays the description of what the builder needs.

## Known limits

These facts stay where they are, and are listed here instead. A second model would revisit each one.

### Board facts outside the port layer

- **Panel layout.** `wellfed_core::buttons::Buttons::from_port_b` maps PB4–PB7 to reset, mic, dispense and lock, active LOW. Core is hardware-independent and cannot depend on the SDK bindings. `pins::BUTTONS` is in the same order.
- **Motor gate.** The `wellfed_core::feed::motor_gate::MotorPins` trait names its arguments `pa13_high` and `pa14_high`. It is a Rust trait, so a board-neutral rename is a Rust-only change.
- **Rust interfaces between Rust crates.** The motor pins (`MotorPins`), a store's flash (`store::slot::SlotIo`) and the owner's journal (`feed::storage::OwnerStorage`) are traits the caller's types implement. Each call is direct: the build's stack audit follows it, and no context pointer crosses it. Only what C or the SDK calls stays `extern "C"`: the flash-lock hook provider the RAM interposer installs, the interrupt handlers, and the `wellfed_*` exports.
- **Motor driver.** The TC118AH's truth table is in `wellfed_hal::board::motor_pair`: both inputs HIGH is brake, and both LOW is refused. The dispensing direction is clockwise, `Drive0`, with PA13 HIGH ([motor pulse](motor-pulse.md)).
- **Beams.** `wellfed_core::feed::sensor_inputs` holds their polarity, and which is food and which is outlet.
- **Stopped outputs.** `wellfed_hal::stopped` defines stopped as motor HIGH, emitters LOW and SWD released. The raw-register copies in `target.rs` (`panic_outputs_safe`, `ram_outputs_safe`) use the derived masks. The RAM-park audit checks `ram_outputs_safe` as leaf code.
- **Raw outputs are `unsafe`.** In `wellfed-amebad-sys` these are `unsafe fn`s whose contract is ownership:
  - the GPIO writers (`gpio_port_write`, `gpio_write_bit`, `gpio_init`): the motor pins belong to `board` and `stopped`;
  - the watchdog (`wdg_init`, `wdg_cmd`, `wdg_refresh`): it belongs to the runtime's supervision;
  - the flash writers (`storage_erase`, `storage_program`, `bank1_erase`, `bank1_program`, `bank1_give_way`): a write needs the armed hooks' lease.

  Product firmware reaches them only through those owners. Bring-up firmwares and their probes take the GPIO, watchdog and storage writers from `wellfed_hal::bringup`, inside an `unsafe` block that states why the contract holds. The bank-1 writers stay the updater's and are not re-exported there. `cargo xtask check-repository` refuses a raw writer or `bringup` in product code (the `feeder` firmware's sources and `src/product/`). It also refuses a raw writer named other than as `bringup::…` in bring-up code (the other firmwares and the probes).
- **Audio pages.** An `sdk::Audio` handle (one at a time, from `Audio::take`) lends one ring page at a time as a `Page` guard, which `submit` consumes. So no two references to a ring page exist, and none outlives the handle.
- **Shared audio pads.** `firstboot_km0.c` powers the audio-shared pads so PA0–PA7 work as GPIO. `audit/pinmap.rs` checks the stock pad table.
- **Limits.** The probes' and simulator's feed and sensor limits (300 mA, battery 1650 scaled, settle times) are bench values for this board.

### Supervision

- **Shared handles.** `Board` and `Events` are `&self` handles over interior cells. They are borrowed only inside `IrqGuard`, for one call, so the TIM2 and feedback interrupts never alias a task's borrow. A `Board` call nested inside another (from code the first call runs) is refused with `Reentered`, rather than lent the state a second time: on one core, waiting for it would never end. The owner keeps the handles beside its task state, never inside it. The probes still hold them inside the state their task borrows `&mut` through its `Slot`. They are bench tools, but moving them out the same way is still open.
- **Flash leases during a feed.** The lease provider checks stopped outputs (`stopped::observe`) when it quiesces. A lease during a feed trips it: the hook halts, the watchdog resets the board, and the owner faults a feed that sees a lease. So no lease may start while a feed may move the outputs.
  - The feed journal writes only at the owner's rest boundaries, with the outputs stopped, under the loop's own claim.
  - Every other store write (the settings writer's stores, the confirmation, the update stage) first takes the write turn in `wellfed_hal::interlock`. The turn waits while the loop holds the outputs ([device API](device-api.md#in-the-firmware)).
  - The provider's request and quiesce run under the same serialization as the board.
- **One watchdog owner.** The runtime feeds the hardware watchdog; the feeding owner only publishes a beat (`supervision::beat`). A fault outside initialization stops the beat, so it resets the board. The beat promises two gaps: 1 s while outputs may move, and the runtime's 10 s liveness limit at rest. The second exists because the Wi-Fi driver's WPA3 handshake held the loop off for over a second, and an idle owner then reset the board for nothing.

### Unit and stock facts

- **Battery divider.** `DIVIDER_PER_MILLE` (2063) was measured on the research unit.
- **Indicator readback.** PA0–PA7 read back LOW on the tested unit, so only PB22 has input readback.
- **Flash ID.** `c84017` (GigaDevice, 8 MiB), in the flash probe.
- **Settings binding.** `SETTINGS_BINDING` names the research unit's installation.
- **Bank addresses** live in three places:
  - Rust: `wellfed-amebad-sys`, `core::ota` (const-asserted equal), `FlashMmuEntry::bank`, `wellfed-flash`, `wellfed-emu` and `review.rs`;
  - C: `firstboot_km0.c`, which also holds the confirmation sector;
  - the layout file: `layout.json`, the only data-driven copy, which the builder patches into the SDK's OTA addresses.
- **BLE system-data patch.** The builder pins `check_sw()` to an erased sector's value, as the research unit's is ([Bluetooth](bluetooth.md#build)).
- **Stock image constants.** The SHA-256, and the bank-table and pad-table addresses, in the builder's audits.

### Chip facts

ADC channel 10 (the internal supply), the ADC eFuse calibration, SWD pad muxing, the audio codec rates, and the `81958711` bank marker.

## For a second model

A new feeder needs:

1. **Board:** a `board` module of its own, with pins, ADC channels and polarities.
2. **Layout:** a `layout.json` from its stock image.
3. **Storage:** a checked storage region.
4. **Description:** a `device.json` naming its ports.
5. **Firmware:** a thin firmware crate ([product architecture](product-architecture.md)).

Every item under Known limits then needs a decision. The panel mapping and the motor gate's names come first, since core carries them.

## Evidence

- **Recovered** (static analysis, October 23, 2026): the audit of the SDK, the HAL and the runtime for PLAF103 assumptions. Nothing ran on the feeder for it.
- **Observed** (October 25): the Wi-Fi driver's WPA3 handshake held the loop off for over a second on hardware, which set the 10 s liveness gap at rest.
- **Observed:** the dispensing direction, seen by the owner in the first owner feed ([motor pulse](motor-pulse.md)).
