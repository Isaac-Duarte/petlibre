# Roadmap

This page tracks what works today and what comes next. It covers the replacement firmware, the tools around it and the optional server. Everything here was tested on one PLAF103 on our bench. It is not a promise that it works on yours.

| Word | Meaning |
| --- | --- |
| **Works** | Done and observed on a real feeder. |
| **Partial** | Works on a real feeder, with a known gap. |
| **Experimental** | Built and tested on the host or in simulation; not yet confirmed on hardware. |
| **Not started** | Planned, nothing built yet. |

## Getting Wellfed on and off a feeder

| Capability | Status | Notes |
| --- | --- | --- |
| Install over stock's own updater | Works | No soldering or UART needed. See [consumer installation](firmware/consumer-installation.md). |
| Go back to stock over the air | Works | Using a stock image we serve. |
| Guided installer (`wellfed install`) | Partial | Run from bank 0 with Bluetooth setup; not yet from stock |
| Roll back to your feeder's own stock image | Partial | Run from the CLI and the app; a vendor app setup afterward has not been completed |
| UART backup and recovery | Works | For developers. See [recovery](devices/plaf103/recovery.md). |
| Updates over the air, with automatic rollback | Works | A new image that fails to start or hangs gives way to the previous one. |

## Feeding

| Capability | Status | Notes |
| --- | --- | --- |
| Feeding with motor supervision | Works | Stops on time, survives a power cut without feeding twice. |
| Dispense button | Works | Respects the child lock. |
| Feed from the CLI | Works | |
| Portion calibration | Works | Weighed once with a kitchen scale (`wellfed calibrate`); the weight survives reboots, installs and the Reset button. |
| Jam and sensor fault handling | Works | A jam, a blocked outlet or a sensor fault ends the feed by name, never retried, and shows in the status and history. Simulated jams on the feeder end as `jammed` and `mark_stuck`, and a blocked outlet stops the motor within about a second; kibble pressed into the rotor by hand did not jam it. |
| Schedules | Works | Up to ten plans, run on the feeder with no server. |
| Feeding history | Partial | `wellfed feeds` lists each feed the feeder keeps, across reboots; a long history needs the server. |
| Food level | Partial | The status and the low-food light show the hopper's latest reading. A feed reads it, and on the adapter so does a short window soon after a boot and every 10 minutes, so a refill shows with no feed. A feed on low food still runs, as stock's does, and its history entry says the food read low. Checked on the feeder with and without food during feeds; the window at rest is tested in simulation only, and its emitter current is unmeasured. The food beam is a low warning, reading empty while about five portions are still left. |

## The feeder itself

| Capability | Status | Notes |
| --- | --- | --- |
| Wi-Fi join and reconnect | Works | |
| Bluetooth setup and pairing | Works | |
| Encrypted local API | Works | See [device API](firmware/device-api.md). |
| Child lock | Works | |
| Reset button | Works | Clears clients, Wi-Fi, settings, plans, history and statistics, and keeps the calibration, the feed journal and the flash wear counts. Checked on the feeder: a hold on an unlocked panel wipes it, and the owner pairs again over Bluetooth. |
| Buttons | Works | |
| Indicator lights | Works | Which states show is not settled. |
| Speaker | Works | Plays sounds; which sounds play when is not settled. |
| Clock and time zones | Partial | Needs a time source after each power-up. Offline time is experimental. |
| Battery and brownout | Partial | Stock's battery mode works on the feeder: Wi-Fi off 5 s after the adapter goes, the Dispense button still feeds, and Wi-Fi back on when it returns, 20 times in a row. The charge on stock's curve, the floor and the brownout hold are tested only in simulation. |
| Statistics and per-feed traces | Experimental | |
| Lights and sound settings | Works | Lights and sound windows, volume-0 mute and a beep on every button press, checked on the bench. |
| Desiccant reminder | Works | Counted on the feeder from the last replacement; the period and the date survive a reboot. |
| Meal reminder | Works | An event on the event stream before each scheduled meal, repeated once after a reboot inside the lead. |
| Meal call | Works | Your own clip, uploaded from the CLI, played before each meal whose plan asks for it, ending as the meal is due, and muted with the sound off. Checked by ear on the feeder at low, middle and full volume. |

## Apps and server

| Capability | Status | Notes |
| --- | --- | --- |
| Command-line client (`wellfed`) | Partial | Every owner setting and action: pairing, status, settings, plans, the meal call, feeds, history, the desiccant reminder, lock, Wi-Fi, time, invites and the installer, with text for people and `--json` for scripts. The meal call, the desiccant reminder and the newer status fields are tested in simulation only. See [the command overview](../src/tools/README.md#the-wellfed-command-line). |
| Self-hosted server | Partial | Optional: the app alone is the complete product, and the server adds long history, remote access, accounts, pets and food, and firmware releases. It has no web page of its own, and you use it through the CLI until the app connects to it. Adopts a real feeder and keeps its history whole across a server restart, a power cut and a Wi-Fi drop. A phone talking to the feeder directly gets no events while the server is connected. Feeds, plans with the next meal, accounts, releases and installs are tested in simulation only. |
| Android app | Experimental | Installs from stock through the feeder's own Wi-Fi after a Reset hold, sets up a feeder over Bluetooth or takes it back over the LAN, and has a screen for everything the device API offers: meals, history, settings, people and invites, updates and diagnostics. Its install has run once on the research unit to the end; the other screens have run only against fakes and the simulator so far. It can put the vendor's firmware back from its backup, with a Dispense press at the feeder; that is simulated only so far. See [the app](../src/app/README.md). |
| Desktop app (macOS and Linux) | Not started | The same app as Android, including the install. |
| iPhone app | Not started | The same app, for setup and daily use. Whether it can install is an open question, so you would install from Android or a computer. |
| Windows app | Not started | Waits for a machine to test it on. |
| Home Assistant | Not started | Paused. Planned as an integration over the server's API. |

## Development hardware

| Capability | Status | Notes |
| --- | --- | --- |
| Wellfed Bench board | Not started | Designed in KiCad with its firmware written; no board built yet. See [the bench board](../src/hardware/bench/README.md). |

## What's next

1. A long unattended run with injected faults, to prove the feeder stays safe over hours.
2. Checking calibration, jams, lights and the battery on the bench.
3. Schedules, then a finished manual feed with stock's portion rules.
4. Finishing the Android app, with the first run of its new screens on the research unit.
5. Proving the server on a real feeder, and connecting the app to it.
6. A first research release.
7. The desktop app on macOS and Linux, then iPhone, then Windows.
