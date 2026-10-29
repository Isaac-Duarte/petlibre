# Glossary

These are the words the rest of the docs use without stopping to explain them.

**AmebaD.** Realtek's family of Wi-Fi and Bluetooth chips, and the vendor SDK we build on. The PLAF103 uses the RTL8721DM. See [the chip page](chips/rtl8721dm/README.md).

**KM0 and KM4.** The chip's two processor cores. KM4 is the fast one and runs the firmware. KM0 is the small one: it boots first, picks which bank to start, and stays in charge of low-level duties.

**SoC.** A system on a chip: processor cores, memory, a radio and the circuits behind each pin in one package. The PLAF103's is the RTL8721DM.

**Boot ROM.** Code built into the chip that can never change. It runs first at every power-on and holds the UART download mode, so the chip can always be recovered.

**Flash.** The memory chip that keeps the firmware and settings with the power off. It is erased in 4 KiB sectors before it is written. See [flash layout](devices/plaf103/flash-layout.md).

**GPIO.** A general-purpose input/output pin. Read, it reports high or low, such as a button or a light beam. Driven, it switches something on or off.

**ADC.** An analog-to-digital converter: a pin that measures a voltage, such as the battery's.

**H-bridge.** A motor driver chip with four switches, which lets two pins run a motor either way or brake it.

**UART.** A simple serial line. The feeder's console is one, and it is how the bench reads, writes and recovers the feeder.

**Bank 0 and bank 1.** The two places in flash where a firmware image can live. A new image goes into the other bank and is given a trial. If it fails, the feeder starts the old one again. See [bank policy](firmware/bank-policy.md).

**Trial and confirmation.** A freshly installed image runs on trial. It becomes the image the feeder boots by confirming that it started cleanly. One that crashes or hangs during its trial is abandoned.

**Stock.** The firmware the feeder shipped with, version 1.1.15 on our unit. See [stock firmware](devices/plaf103/stock/README.md).

**Feeding owner.** The one piece of firmware allowed to turn the motor. Every feed, from a button or an app, is a request to it. It stops the motor on time and records every feed before it starts.

**Journal.** The feeder's record of feeds, kept in flash. Writing a feed down before turning the motor means a power cut can never make the feeder repeat a feed. Instead the feed is reported as *uncertain*.

**Device API.** The encrypted interface the feeder serves on your network. The CLI, the app and the server all speak it. See [device API](firmware/device-api.md).

**Noise.** The [Noise Protocol Framework](https://noiseprotocol.org/), which the device API uses for encryption and to check who is talking.

**Owner and member.** The roles a paired client can have. An owner can do everything, including updates and adding others. A member is invited by an owner and can do less.

**Pairing.** How a phone or computer first gets its key onto the feeder, over Bluetooth, while you are standing next to it. See [pairing](firmware/pairing.md).

**The server.** The optional self-hosted Wellfed server. It keeps long history and serves several feeders and people. The feeder works without it. See [the server](server/README.md).

**SDK.** Here, usually the Wellfed SDK: the Rust runtime every Wellfed firmware runs inside. "Vendor SDK" means Realtek's AmebaD SDK underneath it.

**Evidence levels.** Every finding is labelled by how we know it: *observed* on a real feeder, *recovered* from firmware analysis, or *modeled* in tests and simulation. See [evidence levels](evidence.md).
