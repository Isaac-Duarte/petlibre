# Supported devices

Wellfed runs on one feeder model today: the PETLIBRO Granary PLAF103. This page lists it and the related models we know of, and explains how a new model gets added.

A model counts as supported only when a unit of that model has been through our bench. Sharing a chip is not enough. Boards, pins, flash layouts and stock firmware all differ between models.

| Model | Product | SoC | Status | Notes |
| --- | --- | --- | --- | --- |
| [PLAF103](plaf103/README.md) | Granary 5 L | [RTL8721DM](../chips/rtl8721dm/README.md) | **Partial** | Our research device. Feeding, updates and the device API work; see [its status table](plaf103/README.md#status). |
| PLAF108 | Feeder | ESP8266/ESP32 family (External) | Not supported | No unit inspected. A different SoC family. An ESPHome replacement exists outside this project ([references](../references.md#peer-projects)). |
| PLAF203 | Camera feeder | Embedded Linux with Tuya IPC (External) | Not supported | No unit inspected. Root access and filesystem notes exist outside this project ([references](../references.md#peer-projects)). |
| AF301 (mainboard PLNF301) | RFID feeder | RTL8721DM (External) | Not supported | Same SoC, reported by a peer project. Its pin map is a lead for our [pin table](../chips/rtl8721dm/pins.md), not a PLAF103 fact. |

**Not supported** means we have done no work on that model. Anything listed for it comes from other projects.

## Adding a device

A new model starts as **Not supported**. It moves up once its own unit passes the read-only steps: identification, a receive-only UART capture and matching full backups.

1. Copy [`_template/`](_template/README.md) to `docs/devices/<model>/` and fill in every section, giving each finding its [evidence level](../evidence.md).
2. Add the model's column to the [chip pin table](../chips/rtl8721dm/pins.md).
3. Put its machine-readable layout and profile in `src/firmware/devices/<model>/`, next to [`plaf103`](../../src/firmware/devices/plaf103/). The [device layer](../firmware/device-layer.md) explains how firmware picks them up.
4. Add a row to the table above.
