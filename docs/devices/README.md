# Supported devices

Petlibre is developed on one research unit. A model is listed as supported only when its own unit has been through the bench. Other models with the same SoC are not compatible by default: boards, pins, flash layouts and stock firmware differ.

| Model | Product | SoC | Status | Notes |
| --- | --- | --- | --- | --- |
| [PLAF103](plaf103/README.md) | Granary 5 L | [RTL8721DM](../chips/rtl8721dm/README.md) | **Experimental** | The research device. Local server for stock 1.1.15; replacement firmware in development ([feature support](plaf103/feature-support.md)) |
| PLAF108 | Feeder | ESP8266/ESP32 family (External) | Not supported | No unit inspected. A different SoC family; an ESPHome replacement exists outside this project ([references](../references.md#peer-projects)) |
| PLAF203 | Camera feeder | Embedded Linux with Tuya IPC (External) | Not supported | No unit inspected. Root access and filesystem notes exist outside this project ([references](../references.md#peer-projects)) |
| AF301 (mainboard PLNF301) | RFID feeder | RTL8721DM (External) | Not supported | Same SoC, reported by a peer project. Its pin map is a lead for [pins](../chips/rtl8721dm/pins.md), not a PLAF103 fact |

Statuses: **Experimental** means bench-tested on a research unit with known gaps. **Not supported** means no Petlibre work on that model; anything listed comes from outside.

## Adding a device

Copy [`_template/`](_template/README.md) to `devices/<model>/`, fill in each section with labelled findings, add the model's column to the [chip pin table](../chips/rtl8721dm/pins.md), and add a row here. Machine-readable layout and profile data goes in `src/firmware/devices/<model>/`, next to [`plaf103`](../../src/firmware/devices/plaf103/). Moving PLAF103 assumptions out of the HAL and runtime is [FW50](../../FIRMWARE_BOARD.md). A new model starts as **Not supported** until its own unit passes the read-only steps (identification, UART observe, matching backups).
