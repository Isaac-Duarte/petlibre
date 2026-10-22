# Relay bench control

`tools/bench/bench.py` powers the feeder, enters the ROM loader and chains the existing UART tools. It adds power and strap control only: serial reads, inspections, plans and writes still go through [uart-flash-read.py](../../tools/flash/uart-flash-read.py) and [uart-development.py](../../tools/flash/uart-development.py) with their pinned checks. Running it is a physical bench operation; build and check targets never call it.

## Wiring

A DSD TECH SH-UR04A USB relay board (CP2102 serial, 9600 8N1) adds two switches to the [UART wiring](uart.md):

| Relay | Contacts | Connection | Role |
| --- | --- | --- | --- |
| 1 | COM1, NO1 | In series with USB 5V between two screw-terminal USB-A adapters; GND, D+ and D− pass straight through | Feeder power |
| 2 | COM2, NO2 | Feeder TX side of the 4.7 kΩ strap resistor, and GND | Replaces the loader-entry button |
| 3, 4 | — | Unused | Spare |

Normally-open contacts leave the feeder off and the strap released when the relay board loses USB. The board answers `AT` with `OK`; `AT+CHn=1`, `AT+CHn=0` and `AT+CHn=?` set and query channel *n*. Its front buttons toggle the same state, which queries report.

The relay board also enumerates as a USB serial port and can take the name the UART adapter used to have. Use `/dev/serial/by-id/` paths. They live in the ignored `private/bench.json` with the pinned baseline and ROM report:

```json
{
  "relay_port": "/dev/serial/by-id/<relay board>",
  "uart_port": "/dev/serial/by-id/<UART adapter>",
  "baseline": "private/<pinned full read>/flash-read.bin",
  "rom_report": "private/<ROM inspection>/report.json"
}
```

## Commands

| Command | Effect |
| --- | --- |
| `status` | Relay states and port presence |
| `on`, `off` | Power the feeder with the strap released, or release the strap and cut power |
| `capture NAME` | Receive-only recording of the running feeder; power is untouched |
| `boot NAME` | Power off, start a receive-only capture, confirm it holds the port, then power on |
| `loader NAME` | Same, with the strap held for 2 s across power-on; requires the download banner and a NAK run, otherwise powers off |
| `transients NAME` | Repeated logic-analyzer captures (`sigrok-cli`, fx2lafw) of relay power-ons or, with `--mode loader`, loader entries; each cycle arms the analyzer before power-on and ends powered off. No UART, no flash |
| `dump NAME` | `loader`, then `read-small` and `read-full` in one helper session |
| `flash-prepare NAME` | `loader`, `read-small`, `inspect` and `prepare`; prints the plan summary and hash, then stops |
| `flash-execute NAME` | Checks relay 1 stayed on and the strap is released, runs `execute` with the reviewed hash, then `boot` |

Session evidence goes to new directories under `private/captures/`, `private/flash-backups/` and `private/verification/`, named after `NAME`. Existing paths are refused.

`flash-prepare` and `flash-execute` are separate on purpose: review the plan between them. A failed step stops without retry and leaves the feeder powered for inspection. A stopped update is never followed by a boot; finish it with a fresh `flash-prepare … --interrupted` session as the [development updater](../development/uart-development.md) describes. Relay state shows that the supply was not switched off between the two commands. It does not rule out a brownout.

## Progress dashboard

Each command except `status` records its steps, the tools' progress lines (bytes read, sectors verified), relay changes and the active capture in `private/bench-status/` (`current.json`, and one `history.jsonl` line per command). `tools/bench/bench-dashboard.py` serves that as a page on `http://127.0.0.1:8780/`, refreshed every second, with the tail of a running capture. It only reads those files and the capture's received bytes. It never opens a serial port, so it can run beside any command. It binds to loopback by default because the page shows private UART output. `--listen ADDRESS` serves another interface, for example the Tailscale address to reach it from a phone; any non-loopback address requires a random token, printed in the start URL, and wildcard addresses are refused. Allowing that port through the host firewall is the owner's change. Start it in its own terminal and stop it with Ctrl+C.

Bars show the overall steps, the tools' own counts (sectors verified, bytes read) and, for an update, an estimate from the bytes sent and read back against the reviewed plan. Each has an ETA by straight-line extrapolation; a loader NAK still stops the run outright. A latest-RX card shows the tail of the newest capture. For a timed operator run, an optional `private/bench-status/script.json` lists steps (`{"title", "capture", "anchor", "steps": [{"at", "text"}]}`); the page shows the current step large with a countdown, starting when the anchor bytes (for example `PLOPT1 open`) arrive in that capture. After **Enable sound** (browsers need a click), each step beeps (a double beep to move a probe, a long tone at the end) and **Speak steps** reads it aloud; the page polls four times a second while a script runs.

Stock recovery and standalone diagnostics keep their own pinned tools. Use `loader` to enter the ROM, then run them directly.

## Failure modes

| Failure | Effect | Handling |
| --- | --- | --- |
| Relay board loses USB or the host reboots | Normally-open contacts drop: feeder off, strap released | A running update stops like any other power cut; finish it with `--interrupted` |
| Brownout or supply dip while powered | Relay state still reads on | Not detectable here; the updater's readback and status checks are the guard |
| Strap contact fails at its small current (under 1 mA) | Normal boot instead of the loader | `loader` requires the banner and NAKs, otherwise powers off; nothing is retried |
| UART adapter back-powers the feeder through its TX line | Power-off may not fully reset the chip, so the strap is ignored | Seen intermittently with the manual button (cured by replugging the adapter), not in 32 relay trials; hold a UART break while off or switch adapter TXD with relay 3 if it returns |
| Relay port disappears mid-session | The current command fails; power stays as last set | Cut power by hand if needed, then check `status` |
| Board power also feeds the motor harness | The motor driver is powered whenever the chip is | Only the ROM and flash helper run while flashing; FW30 measures the driver inputs |

## Evidence

**Full live test (physical, October 22, 2026).** Every command was exercised on the research unit, except `transients`, which needs the analyzer:
- **Commands:** `status`, `on`, `capture` (power untouched), `off`, `boot`, `loader` and `dump`. The dump was a full 8 MiB read, and comparing it with the previous one showed exactly the expected changes.
- **Updates:**
  - `flash-prepare` and `flash-execute`: a changed-image install of 142 sectors, finished with `--interrupted` after a loader NAK, then a verified boot.
  - `--bank 0`: an install of 145 sectors.
  - `give-way-prepare/give-way-execute` and `--restore-marker`: the FW09 sequence. Each step booted the expected bank.
- **Refusals,** none of which changed a relay or wrote flash:
  - a reused capture name (fixed during the test; it used to power-cycle first)
  - a motion image without `--attended-motion`
  - `flash-execute` without a prepared plan
  - `flash-execute` after the feeder was powered off since prepare
  - a wrong plan hash (refused before any serial write; the feeder stayed powered and was not booted)
- **Dashboard:** it read the live install's steps, sector count and transfer estimate.

Two loader NAKs occurred in this session. One came while `make check` ran beside an install; none came over three installs and four dumps with the host otherwise idle. Keep heavy host work away from updates.


Physical, October 2026: with the UART adapter connected throughout, relay-1 boots reached the application banner and strap-held boots printed the download banner and NAKs in 16 of 16 trials each. Off times were 0.3–2 s, with and without a UART break held while off. A `flash-prepare`/`flash-execute` pair with the installed image as both source and target ran end to end: no-op plan, one verified acquisition, no writes, then a captured application boot. The earlier symptom of loader entry failing until the adapter was replugged did not recur in these trials.
