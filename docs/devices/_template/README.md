# MODEL (product name)

Copy this folder to `docs/devices/<model>/`. Keep every section, and write **Unknown** rather than leaving one out. Label each finding with an [evidence level](../../evidence.md). Peer-project claims stay External until your unit shows them. See [adding a device](../README.md#adding-a-device).

## Identification

Model, capacity or variant, FCC ID, PCB marking and revision, SoC and flash part numbers, flash ID as read over UART. Link the public filings in [references](../../references.md).

## Hardware

SoC (link its page under `docs/chips/`), flash, motor driver, sensors, connectors and test points. Put the signal map in `board.md` and add the model's column to the chip's pin table.

## Console and loader entry

The UART header, levels and baud rate, plus how ROM download mode was entered on this unit (`uart.md`).

## Flash layout and boot

Bank offsets, preserve ranges and what is still unknown (`flash-layout.md`), backed by `src/firmware/devices/<model>/layout.json`. Unknown gaps are not free space.

## Backup and recovery

Matching full backups first. What a restore has been shown to do on this unit, and what is unverified (`recovery.md`).

## Stock firmware

The stock version, its protocol and behavior (`stock/README.md`), and its update path.

## Support status

A feature-support table like [PLAF103's](../plaf103/feature-support.md), linking board cards.

## Known variants

Other revisions or models that look similar, and why they are or aren't covered.
