# PLAF103 feature support

What works on the PLAF103 by hardware block, after [Asahi's feature-support pages](https://github.com/AsahiLinux/docs/tree/main/docs/platform/feature-support). This page is a summary: each cell links the card on the [firmware board](../../../FIRMWARE_BOARD.md) (FW) or the [feature board](../../../FEATURE_BOARD.md) (FP), and the card holds the evidence and scope. If this page and a card disagree, the card is right and this page is stale.

## Legend

- **Hardware:** done and accepted on the research unit.
- **Hardware (scope):** done on hardware within the stated scope; the card says what is left out.
- **In progress / Ready / Planned:** the card's status, unchanged.
- **Stock:** what stock 1.1.15 does, from its [behavior notes](stock/README.md). The local server on stock is described in the [root README](../../../README.md#current-capabilities).

## Blocks

| Block | Stock 1.1.15 | Replacement firmware |
| --- | --- | --- |
| Console, loader, backups | ROM loader over [UART](uart.md) | Hardware: FW01, FW31–FW33 |
| Build identity and startup | — | Hardware: FW02; Hardware (revised scope): FW03 |
| Flash coordination, journal, settings | Factory loader picks the bank ([flash layout](flash-layout.md)) | Hardware (mediated-path scope): FW06; Hardware (storage scope): FW15 |
| Clock, stop timer, watchdog | — | Hardware (motion-disabled scope): FW05 |
| Wi-Fi | Cloud MQTT ([stock](stock/README.md#network-protocol)) | Hardware (no-renewal scope): FW07 |
| Local transport | — | Hardware (read-only scope): FW08; Planned: FW47 device-hosted API |
| OTA and boot recovery | Stock's own updater ([requirements](stock/ota-requirements.md)) | Hardware: FW09–FW11, FW40, FW41; Ready: FW42, FW43 |
| Food, outlet, feedback optics | Yes ([stock](stock/README.md#feeding-and-device-features)) | Hardware (motion-free scope): FW13 |
| Power inputs and battery | Yes | Hardware (no-load scope): FW14; Planned: FW26, FP14 |
| Motor and feeding | Yes | In progress: FW30 electrical checks; Planned: FW16–FW19, FP05 |
| Buttons and child lock | Yes ([stock](stock/README.md#panel-buttons-and-child-lock)) | Hardware: FW20, FP11; Planned: FP15 reset button |
| Indicators | Yes | Hardware (output-only scope): FW04; In progress: FW23 ([service](../../firmware/indicators.md), host done); Planned: FP12 |
| Speaker and audio | Yes | Hardware (playback scope): FW24; Planned: FP12, FP13 |
| Schedules and time | Yes ([stock](stock/README.md#clock-and-schedules)) | In progress: FW21; Planned: FW46, FP03, FP04 |
| Bluetooth LE setup | Wi-Fi/broker provisioning ([stock](stock/README.md#bluetooth-le)) | In progress: FW34 |
| Home Assistant | Through the vendor cloud only | Planned, paused: FW49, FP16 |

A Hardware entry is accepted for the research unit and the exact images named on the card. It is not a general reliability claim; see [evidence levels](../../evidence.md).
