# MODEL (product name)

One or two sentences: what this feeder is, and whether Wellfed runs on it today.

> Copy this folder to `docs/devices/<model>/` and fill in every section below, in this order. Write **Unknown** rather than leaving a section out. Give each finding its [evidence level](../../evidence.md). A claim from another project stays **External** until your own unit shows it. Then delete this note. See [adding a device](../README.md#adding-a-device).

## Sold as

The brand, product name, capacity and any variants or regional names. Say which variant your unit is.

## How to identify it

What tells this model apart from look-alikes: the label on the base, the FCC ID, the PCB marking and revision, and the flash ID as read over UART. Link the public filings in [references](../../references.md). Name the models that look similar but are not covered, and why.

## SoC and flash

The SoC, linked to its page under [docs/chips/](../../chips/), and the flash part and size. List the other parts that matter: the motor driver, sensors, connectors and test points. The signal map goes in `board.md`, and the model gets a column in the chip's pin table.

## UART access

Where the console header is, its voltage and baud rate, and how ROM download mode was entered on this unit. The details go in `uart.md`.

## Recovery

Whether you can back up and restore this unit, and what a restore has been shown to do. Matching full backups come first. The details go in `recovery.md`, and the flash map in `flash-layout.md`, backed by `src/firmware/devices/<model>/layout.json`. Unknown gaps in the flash are not free space.

## Status

One row per hardware block. The Stock column says in a few words what the stock firmware does. The Wellfed column holds one status word: **Works**, **Partial**, **Experimental** or **Not started** ([what they mean](../../development/writing.md#status)). Keep each note short and put longer caveats in a list under the table.

| Block | Stock | Wellfed | Notes |
| --- | --- | --- | --- |
| Console and backups | | | |
| Flash and boot | | | |
| Wi-Fi | | | |
| Bluetooth LE | | | |
| Over-the-air updates | | | |
| Motor and feeding | | | |
| Food and outlet sensors | | | |
| Power and battery | | | |
| Buttons and child lock | | | |
| Indicators | | | |
| Speaker | | | |
| Clock and schedules | | | |

## Notes

Anything a reader must know before trusting the pages: what was never checked, which unit the findings come from, and warnings that apply to the whole model.

## Pages

| Topic | Page |
| --- | --- |
| Signal map and connectors | `board.md` |
| Console and loader entry | `uart.md` |
| Flash layout and boot | `flash-layout.md` |
| Backup and recovery | `recovery.md` |
| Stock firmware | `stock/README.md` |

## Evidence

How the claims above are known: each entry with its evidence level, the date, and the photo, capture or review that backs it.
