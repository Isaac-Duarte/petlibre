# UART and loader entry

This page shows how to connect a serial adapter to the PLAF103's console and how we entered the ROM's download mode on our unit. It is for developers who want backups or UART recovery. Most owners never need it: Wellfed installs over the air.

## The console header

The board has a three-hole header, `CON5`, labelled **RX, TX, GND**.

| Setting | Value |
| --- | --- |
| Logic level | 3.3 V (idle TX measured at about 3.29 V) |
| Baud rate | 115200, 8N1 |
| Flow control | None. Leave DTR and RTS false and unwired. |
| Pad positions on our unit | RX is the square pad, TX the middle round pad, GND the round pad nearest the mounting screw |

Pad positions can differ. Go by the silkscreen and a measured ground on your own board.

| Feeder | Receive-only | Two-way |
| --- | --- | --- |
| GND | Adapter GND | Adapter GND |
| TX | Adapter RXD | Adapter RXD |
| RX | Not connected | Adapter TXD |
| Power | Not connected | Not connected |

## Connect and watch the console

You need a 3.3 V USB serial adapter and the [Rust flash client](../../development/rust-flash.md), which owns the serial port.

1. Unplug external power, take out the batteries and disconnect USB.

   > [!WARNING]
   > Change wiring only with every power source removed. Leave the adapter's power pin unconnected: the feeder runs from its own supply. Support the opened halves so you don't pull on the wire bundle or the antenna lead.

2. Wire GND and the feeder's TX to the adapter, as in the receive-only column above.
3. Build the client and read its options:

   ```sh
   cargo build --locked --release -p wellfed-flash
   target/release/wellfed-flash read --help
   ```

4. Close any other program that has the serial port open.
5. Start `read observe` with your adapter's stable device path and a new output directory under `private/`. This mode never writes to the serial port. It records the raw bytes and their metadata.
6. Power the feeder from its normal supply. You should see readable boot output from stock.

The other `read` modes send commands or upload a RAM helper. They are separate, active operations.

## Enter download mode

We entered the ROM's download mode with a button, independent of the stock application:

1. Keep feeder TX wired to adapter RXD.
2. Add a branch from feeder TX through a **4.7 kΩ resistor and a normally-open momentary button to GND**.
3. Hold the button and power the feeder on from cold. You should see the ROM download banner followed by repeated NAK bytes.

   > [!WARNING]
   > Never short TX directly to ground. The resistor is part of the setup.

The [relay bench](bench.md) does the same with a relay in place of the button, and another relay to switch feeder power.

This is the arrangement we tested, not a universal strap recipe. Before any active operation, confirm your board, its voltage levels, your wiring and the download banner, and keep your original backups. A working stock application's console loader does not prove you can enter download mode without it.

## Which pin selects download mode

Manufacturer and SDK analysis says the download strap is active-low **PA7**, the LOGUART TX line ([chip page](../../chips/rtl8721dm/README.md#boot-and-download-mode)). An SDK comment naming PA30 conflicts with that. We have not traced continuity from the header all the way to PA7. The route from the header through R30 to the module has about 100 Ω in circuit.

## What a backup covers

- The pinned read helper runs in RAM and changes controller state, including the flash decryption (OTF) configuration.
- An external-flash read does not include the separate OTP, eFuse and security registers.
- Matching dumps prove the reads are consistent. They do not prove that writing back works, or that recovery survives a power cut.

Keep independent copies of your backups before you change anything. The [recovery page](recovery.md) explains why the write tools stay locked to one tested profile.

## Evidence

- **Observed** on the research unit: the `CON5` labels and pad positions in the inspection orientation, idle TX at about 3.29 V, and readable boot output at 115200 8N1 without flow control.
- **Observed:** a button-held cold power-on produced the ROM download banner and repeated NAKs without a stock application command. Later bounded helper reads succeeded through this entry.
- **Observed:** about 100 Ω in circuit on the header, R30 and module route.
- **Recovered:** PA7 as the download strap, from manufacturer and SDK analysis.
