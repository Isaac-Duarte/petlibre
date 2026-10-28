# How the firmware works

This page explains how the Wellfed replacement firmware is put together and why. It is for owners who want to know what runs inside their feeder, and for contributors finding their way around. For what works today, see the [roadmap](../roadmap.md).

## The one idea

One task owns the feeder. A single loop task runs the network, the panel and feeding, and inside it the [feeding owner](../glossary.md) is the only code allowed to move the motor. Everything that wants food asks that owner: the Dispense button, the `wellfed` CLI, the app and the server all go through the same [device API](device-api.md) requests, and all of them land in the same loop.

That one rule is what makes the safety promises simple. Two clients and a button press can't race each other into a double feed, because there is nobody to race. A feed is written down before the motor turns, so a power cut can't make the feeder repeat it. And the motor's stop guarantees live in timers and the watchdog, below the loop, so a slow network can't keep the motor running.

## One feed, end to end

Here is a single one-portion feed, from your finger to the event your app shows.

1. **You ask.** You press Dispense, or run `wellfed feed --portions 1`. The button service task only notices the gesture and queues it. The CLI's request arrives as an encrypted Noise message on the feeder's TCP port.
2. **The loop picks it up.** On its next pass the loop reads the gesture or the request. A locked panel ignores the button; a client's request is checked against its role.
3. **Is a feed allowed now?** One decision, called readiness, answers that. A feed is refused while another runs, while the owner is still starting or faulted, while Wi-Fi is joining, in Bluetooth setup mode, during an update, or on a battery below its floor. A refusal is final: nothing moved, and trying again is a new feed.
4. **The owner writes it down.** The feeding owner reserves the feed in its [journal](../glossary.md) in flash, under the next feed number. Only then does the client's answer go out, naming that number.
5. **The motor turns, supervised.** The owner claims the outputs, turns on the beams and drives the motor. One portion is one compartment of the rotor, about 9 s of drive, counted by a mark on the feedback line ([motor pulse](motor-pulse.md)). A hardware timer, a short motor lease and the watchdog each stop the motor on their own if the owner stops checking in.
6. **The result is recorded.** With the outputs stopped, the owner writes the result to the journal. The loop then records a `Fed` event, which says who asked (`Panel` for the button) and how the feed ended.
7. **Clients hear about it.** The event goes into the event history, and every subscribed client receives it. The CLI that asked prints the result.

Feeds are silent: no tune plays.

**If the power goes out mid-feed**, the reservation is still in the journal. At the next boot the owner records that feed as *uncertain* under its own number, reports it once, and never runs it again. Clients never resend a feed whose answer they didn't get either. The worst case is a feed you have to check, never a feed served twice.

## The layers

The firmware is Rust on top of Realtek's AmebaD SDK. The vendor SDK still supplies startup, FreeRTOS, the radio code, linker scripts and image packaging. Minimal C hooks call the Rust entry point. The chip's two cores, KM0 and KM4 ([glossary](../glossary.md)), are built separately. KM4 builds use bindings generated from the SDK's headers; host tests use fake bindings.

From the bottom up:

| Layer | What it does | Read |
| --- | --- | --- |
| Board | Every pin, ADC channel and storage sector of the PLAF103, in one crate | [Device layer](device-layer.md) |
| SDK runtime | Startup with outputs stopped, the watchdog, Wi-Fi, flash coordination, updates and boot confirmation | [Startup diagnostics](startup-diagnostics.md), [OTA design](ota-design.md), [bank policy](bank-policy.md) |
| Feeding owner | The motor, beams and feedback under supervision, and the journal | [Motor pulse](motor-pulse.md), [jams and sensor faults](jams-and-sensor-faults.md), [calibration](calibration.md) |
| Product | The loop: the panel, the device API, pairing, events and settings | [Device API](device-api.md), [pairing](pairing.md), [Bluetooth](bluetooth.md) |
| Peripherals | Buttons, lights, speaker, clock and power | [Buttons](buttons.md), [indicators](indicators.md), [audio](audio.md), [clock](clock.md), [power](power.md), [optical inputs](optical-inputs.md) |
| Schedules | Feeding plans: when each runs, at most once | [Schedules](schedules.md) |
| Reminders | The desiccant reminder's count, from the last replacement | [Desiccant reminder](desiccant.md) |

The design behind all of this, from "one firmware, server optional" to the choice of Noise, is in [product architecture](product-architecture.md). Whether it fits the chip is measured in [resource budget](resource-budget.md). How a stock feeder moves to Wellfed, and back, is in [consumer installation](consumer-installation.md).

Wellfed has a second, separate path to the feeder. The `wellfed-stock` server talks to **stock 1.1.15** over stock's own MQTT protocol. It never runs inside the feeder, and the replacement firmware doesn't use it except to install itself. Stock 1.1.15 can install the replacement over its own updater, with no UART write.

## Runtime contracts

These are the promises the runtime keeps, whatever the rest of the firmware does.

**The owner and its stops.**
- The owner serializes requests and drives the controller from qualified inputs and captured feedback.
- The output gate uses stop intervals, leases and fault latches.
- Timer supervision can stop the drive when the owner stalls. One timer interrupt without SYSTIMER advance is tolerated: it is a pending update coalesced after a masked window. A second in a row is a frozen-clock fault.
- Closing supervision first verifies motor STOP and infrared OFF.
- Infrared OFF always requires LOW output latches, not only LOW pad inputs.

**The watchdog.**
- A watchdog refresh requires progress.
- The owner refuses a flash authority whose total lease budget, plus two sample and two timer periods, does not fit inside the watchdog's progress gap.

**Interrupted feeds.** Interrupted feeding is reconciled conservatively, never resumed. At the next boot a pending reservation is recorded in the journal as an uncertain result under its own id. It is reported once as such and never retried, and the owner opens.

**Faults that stop.** After the watchdog is armed, some failures latch a stopped fault: a journal that does not decode, a resolution that cannot be recorded, or another initialization failure. A stopped fault keeps refreshing the watchdog only while every iteration re-verifies STOP and infrared OFF. So it neither retries nor reset-loops. A hang or a lost output readback still resets.

**Panics.** On KM4 a panic masks interrupts and requests motor STOP and infrared OFF with raw latch writes, then halts. Only an already armed watchdog resets it.

**The dual-slot store.** The journal, settings, seed, Wi-Fi, client and update stores are one dual-slot store (`wellfed_core::store::slot`).
- Each store alternates complete records, with a CRC and a final commit marker.
- Every write follows one order: erase the other unit, check it blank, program all but the marker, check, program the marker, check. A power cut therefore leaves the other unit untouched.
- That module documents the rule, and what each store's policy makes of a cut unit. The journal's policy is the strictest: it never falls back past a reservation.
- Golden-byte tests pin every record's layout.

**Interrupts.** Rust ports keep caller-owned objects shared with interrupt handlers behind critical sections. A fully interrupt-safe ownership model is future work.

## What is still open

- **Flash across both cores.** Flash coordination, the SDK hooks and RAM parking are tested, but complete exclusive flash ownership across both cores is unfinished. KM0's own SDK flash users are not yet mediated, which is safe only while KM0 never sleeps.
- **Electrical acceptance.** Voltage and transient acceptance of the stopped outputs, and Port A input readback, are unresolved ([startup diagnostics](startup-diagnostics.md)).
- **Schedules.** Running on the feeder ([schedules](schedules.md)). The sound before a meal is not built.
- **The rest** is on the [roadmap](../roadmap.md): calibration, jams, lights and battery checks on hardware, and a long unattended run.

## Crates

The full inventory, with how to write a firmware, is the [firmware guide](../../src/firmware/wellfed/README.md).

| Crate | In short |
| --- | --- |
| `wellfed-session` | Noise, HMAC, the setup code and framing, shared with clients |
| `wellfed-core` | Controller, portions, sensors, motor gate, clock and the stores |
| `wellfed-hal-common` | Code both cores link: RAM parking, flash hooks, the boot trial, panics |
| `wellfed-hal` | KM4 ports: GPIO, ADC, timers, watchdog, flash leases, the update stage |
| `wellfed-sdk` | The KM4 runtime every firmware runs inside |
| `wellfed-probes` | The bench probes |
| `wellfed-amebad-sys` | Bindings to the pinned AmebaD SDK, and its host fake |
| `wellfed-board-plaf103` | The PLAF103's pins, ADC channels and storage map |
| `wellfed-testkit`, `wellfed-sim` | Fake NOR storage and the virtual feeder |
| `wellfed-proto` | The device protocol's messages |
| `wellfed-device-api` | The feeder's side of the device API and pairing |
| `wellfed-feeder` | Product policy and the loop itself |
| `wellfed-feeder-runtime` | The product's stores on the SDK runtime |
| `wellfed-fw-feeder` | The product firmware |
| `wellfed-fw-qa`, `wellfed-fw-bringup`, `wellfed-fw-blink` | The self-test, bring-up and minimal firmwares |

## Evidence

- **Modeled:** the stores' torn writes and restart behavior are host-tested once against the slot layer and again per store. The fake NOR runs at 256-byte, 4 KiB and odd-tail unit sizes. It models erase to 0xFF, bit-clearing programs, byte-ordered program cuts, and erase cuts that leave old data with an arbitrary deterministic subset of bits raised. It does not model weakly programmed or erased cells, unstable reads, page wrap, or a real part's timing. This is simulation, not flash evidence.
- **Modeled:** the core diagnostic uses synthetic inputs and memory-only motor outputs. Separately, the default application enables GPIO clocks, requests physical motor STOP and infrared off, and runs a bounded LED probe. The minimal KM0 main restores the stock shared-pad supply prerequisite.
- **Observed:** feeds through the device API and the Dispense button, and power cuts during a feed resolving as uncertain, on the research unit. See the [roadmap](../roadmap.md) and the topic pages' Evidence sections.
- SDK build success and model coverage do not establish electrical safety or physical reliability.
