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
| Guided installer (`wellfed install`) | Experimental | |
| Roll back to your feeder's own stock image | Experimental | |
| UART backup and recovery | Works | For developers. See [recovery](devices/plaf103/recovery.md). |
| Updates over the air, with automatic rollback | Works | A new image that fails to start or hangs gives way to the previous one. |

## Feeding

| Capability | Status | Notes |
| --- | --- | --- |
| Feeding with motor supervision | Works | Stops on time, survives a power cut without feeding twice. |
| Dispense button | Works | Respects the child lock. |
| Feed from the CLI | Works | |
| Portion calibration | Experimental | |
| Jam and sensor fault handling | Experimental | |
| Schedules | Works | Up to ten plans, run on the feeder with no server. |
| Feeding history | Partial | Each feed is recorded as an event; a long history needs the server. |
| Food level | Not started | The sensor is read; the feature isn't built. |
| Importing your stock schedule | Not started | Stock's plan records are decoded; the installer does not read them yet. |

## The feeder itself

| Capability | Status | Notes |
| --- | --- | --- |
| Wi-Fi join and reconnect | Works | |
| Bluetooth setup and pairing | Works | |
| Encrypted local API | Works | See [device API](firmware/device-api.md). |
| Child lock | Works | |
| Reset button | Partial | Forgets paired clients; settings are kept for now. |
| Buttons | Works | |
| Indicator lights | Works | Which states show is not settled. |
| Speaker | Works | Plays sounds; which sounds play when is not settled. |
| Clock and time zones | Partial | Needs a time source after each power-up. Offline time is experimental. |
| Battery and brownout | Experimental | Stock's battery mode (Wi-Fi off 5 s after the adapter goes, meals and buttons on, updates refused), the charge on stock's curve, a floor and a brownout hold, all in simulation. |
| Statistics and per-feed traces | Experimental | |
| Lights and sound settings | Works | Lights and sound windows, volume-0 mute and a beep on every button press, checked on the bench. |
| Desiccant reminder | Experimental | Counted on the feeder from the last replacement; tested in simulation. |
| Meal reminder | Experimental | An event on the event stream before each scheduled meal; tested in simulation. |

## Apps and server

| Capability | Status | Notes |
| --- | --- | --- |
| Command-line client (`wellfed`) | Partial | Pairing, status, settings, lock, Wi-Fi, events. |
| Self-hosted server | Partial | Adopts a real feeder and keeps its history whole across a server restart, a power cut and a Wi-Fi drop. A phone talking to the feeder directly gets no events while the server is connected. Feeds, accounts, releases and installs are tested in simulation only. |
| Server web UI | Not started | |
| Android app | Not started | |
| Home Assistant | Not started | Paused. |

## Development hardware

| Capability | Status | Notes |
| --- | --- | --- |
| Wellfed Bench board | Not started | Designed in KiCad with its firmware written; no board built yet. See [the bench board](../src/hardware/bench/README.md). |

## What's next

1. A long unattended run with injected faults, to prove the feeder stays safe over hours.
2. Checking calibration, jams, lights and the battery on the bench.
3. Schedules, then a finished manual feed with stock's portion rules.
4. The server's web UI and the Android app.
5. A first research release.
