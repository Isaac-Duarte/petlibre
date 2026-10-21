# UART and loader entry

## Console connection

The inspected board has a three-hole `CON5` header labeled **RX, TX, GND**. In the recorded inspection orientation, RX is the square pad, TX the middle round pad, and GND the round pad nearest the mounting screw. Use the actual silkscreen and measured ground on your board rather than positional descriptions alone.

The research unit's idle TX was measured at approximately **3.29 V**. Readable normal boot output was captured at **115200 baud, 8N1**, without flow control. Use a compatible 3.3 V logic adapter. DTR/RTS remain false and unwired; the adapter's power pin remains disconnected. The feeder uses its normal power source. Disconnect external power, batteries, and USB before changing wiring. Support the opened halves without pulling the wire bundle or antenna lead.

| Feeder | Receive-only connection | Active UART connection |
| --- | --- | --- |
| GND | Adapter GND | Adapter GND |
| TX | Adapter RXD | Adapter RXD |
| RX | Disconnected | Adapter TXD |
| Power | No adapter connection | No adapter connection |

Optional serial support is installed explicitly:

```sh
.venv/bin/python -m pip install -r tools/requirements-hardware.txt
.venv/bin/python tools/uart-flash-read.py --help
```

The `observe` mode receives without serial writes and records raw bytes plus metadata. Choose your own stable device path and a new output directory under `private/`. Close other serial clients first. Other modes can execute commands or upload a RAM helper; they are separate operations.

## Independent download entry

The tested setup branched feeder TX through a **4.7 kΩ resistor and normally-open momentary button to GND**, retaining the TX-to-adapter-RXD connection. A button-held cold power-on produced the ROM download banner and repeated NAK bytes without a stock application command. Later bounded helper reads succeeded through this entry route.

Manufacturer/SDK analysis associates download selection with active-low **PA7**, the LOGUART TX route; an SDK comment naming PA30 conflicts with that analysis. Physical continuity from the accessible header all the way to PA7 is not established. The observed header/R30/module route includes approximately 100 Ω in circuit. No direct TX-to-ground short is part of the documented setup.

The [relay bench](bench.md) automates this entry: a relay replaces the button and another switches feeder power.

This describes the tested research arrangement, not a universal strap recipe. Establish your board identity, voltage levels, wiring, entry observation, and original backups before considering active operations. A working stock application's console loader does not prove application-independent entry.

## Backup boundaries

The pinned helper runs in RAM and changes controller state, including OTF decryption configuration. An external-flash read excludes separate OTP/eFuse/security registers. Matching dumps establish read consistency, not accepted write-back or power-cut recovery. Preserve independent backup copies before changes.

The [recovery reference](recovery.md) explains why retained write tools remain restricted to one tested profile.
