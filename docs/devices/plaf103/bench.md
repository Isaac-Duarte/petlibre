# Relay bench

The relay bench lets one computer power the feeder, enter its ROM loader, flash it, update it over the air and run long integration tests, all without anyone at the table. This page is the guide and reference for `wellfed-bench`. It is for contributors who have the bench wired up.

`wellfed-bench` ([source](../../../src/tools/wellfed-bench)) adds power and strap control only. Every serial read, inspection, plan and write runs as a separate `wellfed-flash` process, so the [flash client's](../../development/rust-flash.md) pinned checks, plan hashes and no-retry stops apply unchanged.

The bench is one server. `wellfed-bench serve` runs in a terminal of its own and holds the bench for as long as it runs. Every other command is its client: it asks the server to run the command as a job, prints the job's output and exits with its code.

Running any bench command is a physical operation on the feeder. Build and check targets never call it: `cargo test` and `make check` never reach the bench.

## Safety rules

These hold for every command on this page. The sections below give the detail.

- **Relay 1 is feeder power, and the kill switch.** All four relays are normally open, so if the relay board loses USB the feeder goes off and the strap is released ([wiring](#wiring)).
- **One tool drives the bench at a time.** The bench server holds the [bench lease](#bench-lease) for its life and runs one job at a time. A second job is refused, never queued. A killed server leaves a lease the next one refuses to override.
- **Unattended motion follows the owner's rules:** hopper empty, the D cells out so relay 1 is the kill switch, nobody at the bench, the bench lease free, and at most 3 portions a feed. Only `feed-writes` and `sustained`, under `--feeds`, ask the feeder to feed ([sustained run](#sustained-run)).
- **Motion builds stay manual.** The bench never offers a startup-motion build, refuses the motor pulse under `--build`, and needs `--attended-motion` for any image whose manifest names a motion policy.
- **A halt cuts relay 1 and leaves it off.** A person looks before power returns.
- **The bench never changes routing or the firewall.** Redirecting stock's traffic (`wellfed-stock network apply`) is the operator's change, made and removed by hand.
- **Don't rebuild `wellfed-flash` between prepare and execute.** A plan binds the SHA-256 of the binary that prepared it.
- **Keep heavy host work away from updates.** Loader NAKs came while `make check` ran beside an install.

## Set up

### Wiring

A DSD TECH SH-UR04A USB relay board (CP2102 serial, 9600 8N1) adds four switches to the [UART wiring](uart.md). The [wiring diagram](../../../src/tools/wellfed-bench/wiring.svg) carries reference designators and net names for a bench PCB. That PCB is the [Wellfed Bench board](../../../src/hardware/bench/README.md), which puts the adapter and the four switches on one USB cable. It is designed but not built, so this page still describes the relay board.

| Relay | Contacts | Connection | Role |
| --- | --- | --- | --- |
| 1 | COM1, NO1 | In series with USB 5V between two screw-terminal USB-A adapters; GND, D+ and D− pass straight through | Feeder power |
| 2 | COM2, NO2 | Feeder TX side of the 4.7 kΩ strap resistor, and GND | Replaces the loader-entry button |
| 3 | COM3, NO3 | In series with adapter TXD → feeder RX | UART into the feeder |
| 4 | COM4, NO4 | In series with feeder TX → adapter RXD, on the adapter side of the strap | UART out of the feeder |

- **Normally-open contacts** leave the feeder off and the strap released when the relay board loses USB.
- **Relays 3 and 4 isolate the UART.** A powered adapter holds its TXD and RXD lines high, which can back-power a feeder that is off through its UART pins. So every power cut opens relays 3 and 4 after the supply, and every power-on closes them just before it. GND stays connected.
- `capture` connects the UART only on a powered feeder. `--uart-when-off connected` leaves relays 3 and 4 closed through the cuts, as the bench was before they were wired, for comparison.
- `ota`, and `itest` without `--power`, leave the relay board alone. They need a feeder the bench powered, which connected the UART.
- The board answers `AT` with `OK`. `AT+CHn=1`, `AT+CHn=0` and `AT+CHn=?` set and query channel *n*. Its front buttons toggle the same state, which queries report.

### Software

1. Build the binaries together, from the repository root:

   ```sh
   cargo build --locked --release -p wellfed-bench -p wellfed-flash -p wellfed-stock -p wellfed
   ```

   The server runs the `wellfed-flash` beside itself unless `serve --flash-bin` names another. `itest`, `stock-ota` and `install` run the `wellfed-stock` beside it unless `serve --stock-bin` names another ([stock server](../../development/stock-server.md)), and `install` runs the `wellfed` beside it unless `serve --wellfed-bin` names another.

2. Write the ignored `private/bench.json`. The relay board also enumerates as a USB serial port and can take the name the UART adapter used to have, so use `/dev/serial/by-id/` paths:

   ```json
   {
     "relay_port": "/dev/serial/by-id/<relay board>",
     "uart_port": "/dev/serial/by-id/<UART adapter>",
     "baseline": "private/<pinned full read>/flash-read.bin",
     "rom_report": "private/<ROM inspection>/report.json"
   }
   ```

3. Check the bench without a server:

   ```sh
   target/release/wellfed-bench status
   ```

   You should see the lease (free), no server, the four relay states and both ports present.

4. Start the server in a terminal of its own:

   ```sh
   target/release/wellfed-bench serve
   ```

   You should see the socket, the lease and the four relays. It runs until `q` or Ctrl+C in its [console](#the-console). Run every other command from a second terminal.

Run the bench from the repository root, or pass `--root`. A job reads the paths you give it, such as `--target`, from the server's directory, so start the server at the repository root and give paths relative to it. Because the bench never links the flash client in, rebuilding the bench alone does not affect a plan.

### The crates

The bench is six crates with one-way dependencies:

| Crate | What it does |
| --- | --- |
| `wellfed-relay` | The relay board driver, behind a `Board` trait the Wellfed Bench board can implement |
| `wellfed-rig` | The operator sequences, and the steps the suite builds on |
| `wellfed-itest` | The integration suite, on the rig |
| `wellfed-bench-server` | The server: the lease, the board, the socket, the feeder session and the UART capture, the sequences and the suite as its jobs, and the bank ledger |
| `wellfed-dashboard` | The read-only progress page. It links neither the relay driver nor the flash client. |
| `wellfed-bench` | The command line over all of them, as one binary: `serve` and its clients |

The files and shapes they share (the bench config, a command's progress record, the server's live state, the run records and the suite's catalog) are typed in `wellfed-formats`. The rig signs offers through `wellfed-signing`. Every build runs as a `wellfed-build` process, and every flash read and write as a `wellfed-flash` process.

## Commands

| Command | Effect |
| --- | --- |
| `serve` | The bench server, in the foreground until `q`, Ctrl+C, SIGTERM or a hangup ([below](#the-bench-server)), drawn as a [full-screen console](#the-console). `--ui` serves the [progress page](#progress-page) beside it. |
| `status` | The [bench lease](#bench-lease), then the server's job, its children, the UART, the feeder session, the bank ledger and the relays. With no server, it reads the relay board itself. A server that does not answer within 5 s is reported as such, after the lease. `--break-lease` first frees a lease a killed server left. |
| `attach ID` | Follow job `ID` of the running server again, from its first kept line |
| `answer JOB SEQ done\|yes\|no` | Answer question `SEQ` of job `JOB` ([install as a job](#install-as-a-job)), once |
| `banks` | The [bank ledger](#the-bank-ledger). `--seed DUMP_SESSION` starts it, from that dump's inspection or, when the inspection names no build, from `--bank N --build HASH`. |
| `note` | The [operator's notes](#the-operators-notes): the step under way, the next steps, timers and evidence by card |
| `mcp` | The bench's tools for an agent, over the [Model Context Protocol](#mcp) on stdio, as a client of the server |
| `on`, `off` | Power the feeder with the strap released, or release the strap and cut power |
| `capture NAME` | Receive-only recording of the running feeder. Power is untouched. |
| `press CHORD` | Holds panel buttons together on a running `virtual-panel` build (`dispense`, or several joined by `+`: `lock+dispense`), for `--hold-ms` (150 by default, at most 15000), typed on its UART ([virtual panel](#virtual-panel)). Power is untouched; refused unless the build echoes the press and reports its release. |
| `boot NAME` | Power off, start a receive-only capture, confirm it holds the port, then power on |
| `loader NAME` | The same, with the strap held for 2 s across power-on. Requires the download banner and a NAK run, otherwise powers off. |
| `transients NAME` | Repeated logic-analyzer captures (`sigrok-cli`, fx2lafw) of relay power-ons or, with `--mode loader`, loader entries. Each cycle arms the analyzer before power-on and ends powered off. No UART, no flash. See [motor driver](motor-driver.md#capture-startup-transients). |
| `dump NAME` | `loader`, then `read small` and `read full` in one helper session |
| `flash-prepare NAME` | `loader`, `read small`, `update inspect` and `update prepare`. Prints the plan summary and the exact `flash-execute` line, then stops. |
| `flash-execute NAME` | Checks relay 1 stayed on and the strap is released, runs `update execute` with the reviewed hash, then `boot` |
| `give-way-prepare NAME`, `give-way-execute NAME` | The same pair for `give-way`. `--fallback` names bank 0's frozen review. |
| `stock-prepare NAME`, `stock-execute NAME` | UART return to stock ([below](#to-stock-over-uart)) |
| `ota NAME` | One over-the-air update of the running feeder ([below](#over-the-air-updates)). Never touches power or opens the relay board. |
| `stock-ota NAME` | Stock installs a Wellfed build through its own updater ([below](#stock-to-a-replacement-stock-ota)) |
| `install` | `wellfed install` as a job: its questions show on the page and the console, and the page, `answer` and the agent answer them, and it restarts the feeder itself with relay 1 ([below](#install-as-a-job)) |
| `itest NAME` | The real-device [integration suite](#integration-suite). Runs its own hotspot, `serve` and receive-only UART capture, installs over the air, and restores the bench at the end. Opens the relay board only with `--power`, for cold boots. |

`wellfed-bench <command> --help` lists the flags. `flash-prepare` and `flash-execute` take the updater's `--bank`, `--source`, `--original-read`, `--target`, `--interrupted` and `--restore-marker`, and `--attended-motion` for a target whose manifest names a motion policy.

Every command from `on` down is a job: it needs the server running, and refuses otherwise, naming `wellfed-bench serve`.

### Virtual panel

The feeder's `virtual-panel` variant lets the bench press the panel's buttons with nobody there: Bluetooth setup codes, the recovery window and the Dispense press that enrolls an update key. `press CHORD` runs `wellfed-flash read press`, which types one line, `press <chord> hold <ms>`, on the feeder's UART, one byte every 2 ms, after a line end that ends anything a stray byte started. A chord is one button or several held together, spelled as the gesture records spell it: `press lock+dispense hold 5200` opens the recovery window, as two fingers would, once `press lock --hold-ms 5200` has unlocked the panel (it starts locked at every boot and relocks after 30 s idle). The build holds the chord's buttons down for the hold, merged into the levels its button task reads every 5 ms, so the press goes through the same debounce, holds and gestures as fingers. The tool requires the build's exact echo, `PLKEY1 virtual press=<chord> hold_ms=<ms>`, then the chord's `PLKEY1 up` record. It returns as soon as both have arrived as whole lines, and otherwise reads until 1.5 s past the hold, then judges everything it got; both are in the press's report and capture under `private/captures/press-<ms>`.

- **Only that variant reads the line.** No other Wellfed image links a UART reader (the build links no vendor shell), so a line typed to any other Wellfed build does nothing, and `press` stops on the missing echo. Stock has a console of its own and the ROM loader reads its download protocol, and `press` checks only that relay 1 is on, so boot the variant first.
- **Each wrong answer stops the job, and the line is never sent again.** No echo: not the variant, or not running. Only `virtual refused`: the line lost bytes (a FIFO overrun refuses it whole). Another press echoed: bytes were lost into a different valid line, which the build pressed, so check the capture. A `virtual refused` before the exact echo is a stray byte's line, and the press still counts. No release: the gesture did not finish in the window.
- **Not shipping it is a convention today.** The variant is chosen only with `--variant`, and the build identity and manifest name it. Nothing yet refuses one: the builder takes `--provisioning consumer --variant virtual-panel`, which an unattended consumer install needs, and no release signer exists yet. The release signer, when it is written, must refuse any manifest that names a variant.
- **A Dispense press is a feed** outside a setup or enrollment window, exactly as a finger's would be. Keep the hopper empty.
- **The bench, not the client under test, presses.** A pairing that passes with a press here is one that would pass with a person there.

**Names and evidence.** Session evidence goes to new directories under `private/captures/`, `private/flash-backups/` and `private/verification/`, named after `NAME`. It must be a single plain name. An existing path is refused before any relay changes.

**Stopping.** Ctrl+C in the client's terminal asks the server to cancel the job, and the client keeps printing its cleanup. The server sends SIGINT to the `wellfed-flash` child, which stops itself terminally and records it, and SIGTERM to the job's other children with everything they started (a build's `cargo` and `make`). A flash session that writes is only ever sent SIGINT. A receive-only capture is killed after it, as it always was when a wait was interrupted or ran out. The sequence then runs the same cleanup as for any stop: the loader and transient sequences power off, and the demo turns the spares off. While the job writes flash over UART, the server refuses the first Ctrl+C's cancel and the client says so: only a second Ctrl+C within 5 s cancels the write, which then stops for good. Once the cancel is taken, another Ctrl+C leaves the job running and exits; `attach ID` follows it again.

### The bench server

`serve` holds four things for its whole life:

- the [bench lease](#bench-lease), which the jobs' children run on;
- the relay board, opened and read at the start, after every job, and every 30 s while no job runs (never during one), so its front buttons show;
- the control socket `private/bench.sock`, mode 0600 whatever the umask, which only the checkout's owner can reach;
- the owner's one device API session with the feeder ([below](#the-feeder-session));
- a receive-only UART capture while no job runs and relay 1 is on ([below](#the-uart-capture));
- its [console](#the-console): every job's output, as it comes, and one line as each job starts and ends.

It runs one job at a time. A job is the command line a client posts (`POST /jobs` with its words), parsed by the server with the same checks as the command line, so a bad flag or name is refused before anything runs. The client sends its checkout beside the words, and the server refuses another checkout's: a worktree whose `private/` links to this one's reaches this server, and must not run this checkout's sources under its session names. A job runs the same sequence the command always ran, in the server's process. A second job while one runs is refused with the running job named:

```
Stopped: The bench is busy with job 3 (dump fw-01); wait for it, or cancel it first
```

A client that disconnects, a closed terminal say, only stops reading. The job runs on, and `attach ID` prints its output again from the start. The server keeps the output of its last 8 jobs in memory, and every job's in `private/bench-status/jobs/ID-NAME.log`. That file is written through a buffer and never synced, so a UART write never waits on the disk; it is complete once the job ends. Job numbers carry on from the highest file there, so a new server never reuses one.

`ota`, and `itest` or `stock-ota` without `--power`, get a board that refuses every switch, as when they ran without opening it.

Ctrl+C, SIGTERM or a hangup of the server's terminal stops the server. It refuses new jobs, cancels the running one as a client's Ctrl+C does, waits for its cleanup, then removes the socket and releases the lease. Every child of a job runs in a process group of its own, so the terminal's Ctrl+C or hangup reaches only the server, never a `wellfed-flash` that is writing; the server forwards the cancel itself.

A cancelled write stops terminally and leaves its bank half-written until another UART write, so while the flash client holds the UART nothing stops the server by accident:

- **Ctrl+C or `q`** during a write or a loader session only says "A UART write is under way; press again within 5 s to cancel it". A second press within 5 s stops the server, cancelling the write.
- **SIGTERM or a hangup** during a write is deferred, however many arrive. The server says it is stopping once the write ends, shows the stop it holds in its state (`server.stopping`, on the page, the console's header and `bench_status`), refuses every new job meanwhile, lets the job finish, then stops as usual. A signal outside a write stops it at once.
- **To stop a deferred server sooner**, send SIGINT twice within 5 s (`kill -INT PID` twice, which needs no terminal): that cancels the write, which then stops for good. SIGKILL remains the last resort: the [lease](#bench-lease) and the orphan checks handle a killed server.
- **The full-screen console ending on its own** (a terminal error, a panic) stops nothing. The server prints its lines on the terminal again, says the console ended, and Ctrl+C or SIGTERM stops it from then on.

The socket's routes:

| Route | Answer |
| --- | --- |
| `GET /state` | The live state: the job and its next steps, every child it runs, the UART (idle, capturing, a loader session, or writing with sectors verified), the relays, the feeder session, the capture's tail and records, and the bank ledger |
| `GET /events` | The live state again on every change, as server-sent events |
| `POST /jobs` | `{"root": ..., "argv": [...]}`, the client's checkout and the command's words: 201 with the job's number, 400 for words that are no job, 409 while a job runs |
| `GET /jobs/{id}/stream` | The job's output lines, then its end, as newline-delimited JSON |
| `GET /jobs/{id}/output?after=N` | A page of at most 500 of the job's kept lines after line `N`, the next line number, and the job's end once that is all |
| `POST /jobs/{id}/cancel` | `{"during_write": "refuse"\|"cancel"}`: 202; 409 for a job that already ended, or one writing flash over UART unless `"cancel"` says to stop the write; 503 while the server stops |
| `POST /jobs/{id}/answer` | `{"seq": N, "answer": "done"\|"yes"\|"no"}`, the answer to the question the running job asks (the state's `prompt`): 202; 409 `{"answer": {"error": {"refused": ...}}}` when the job is not running, asks nothing, or asks another question; 400 for an answer of the other kind |
| `POST /device/ask` | `{"id": [...], "op": {...}}`, one device API request on the server's session: 200 with `answered` and the answer, or `uncertain`; 503 `{"reason": ...}` when nothing was sent |
| `GET /device/pushed?stream=events\|logs&from=N&wait=MS` | The history (events and gaps) or the log lines the session was pushed, from number `N` on, waiting up to `MS` (at most 30 s) for the first; with the event the held history starts after |
| `GET /device/session` | Whom the session speaks as and with: the checkout, the key file and its public key, the feeder, and once open its build, role and capabilities; 503 while paused or unconfigured |
| `GET /banks`, `POST /banks/seed` | The bank ledger, and `banks --seed` |
| `GET /wait?for=...&timeout_ms=MS` | Holds until a condition holds, at most 10 minutes: `for=job&id=N` (it ended), `for=line&text=T` (a UART, device log or job output line containing `T` after the wait began), `for=event&kind=K`, `for=status&build=B&bank=N` (a status read after the wait began, so one from before a reboot never counts; the answer names the boot count), `for=timer&label=L` (due, seen or missed), `for=idle` (no job, no loader or write) or `for=prompt` (a job asks a question). 200 with `met` and how, or `timed-out`; 400 `{"wait": {"error": {"refused": ..., ...}}}` for a condition missing a field, or one nothing could meet: a timer nobody set, or a job past the next one to be given. It only reads. |
| `POST /notes` | One [operator's note](#the-operators-notes), the body of a line of `notes.jsonl`: 201 with the note as kept, 400 `{"note": {"error": {"refused": ..., ...}}}` for one refused, such as `{"refused": "outside-private", "path": "src"}` |

#### The feeder session

The server opens one session with the feeder, as the owner key the bench already uses (`private/wellfed-owner.json`), and finds the feeder by its identity over mDNS or by its MAC in the hotspot's neighbour table, as the rig does. `serve --feeder-address` names the address instead (the simulator, say). The session:

- subscribes after the newest event it took, which it keeps in `private/bench-status/device-cursor.json`, so a new server does not replay the history;
- follows the feeder's debug log into the state and into `private/bench-status/device-log/YYYY-MM-DD.log`, one file a UTC day, each line stamped with the host's time, through a buffer that is never synced;
- reads the status every 10 s, and sets the clock as `wellfed time` does (this host's clock and zone) when it is invalid or more than 2 s off. After the feeder refuses a sync, for any reason, it waits 5 min before the next. A job's clock sync through the session uses this host's zone too, so the two never disagree. `serve --no-clock-sync` leaves the clock alone, for a check that needs it invalid;
- reads whether the hotspot's connection is up every 30 s, with `nmcli` listing the active connections only, and never while a job runs or the UART is in a loader session or a write.

It closes:

- while a job that holds the UART runs (`capture`, `boot`, `loader`, `transients`, `dump` and every prepare and execute);
- while an install push runs (`ota`'s, or one of `itest`'s), which opens owner sessions of its own; it reopens after its cursor once the push ends;
- for the whole of `stock-ota`, which meets stock (no device API) and then runs its own sessions with the build on bank 0;
- for the whole of `install`, which meets stock, then pairs with the replacement and promotes it with sessions of its own;
- while relay 1 is off, and while the server stops.

Otherwise it reopens after a loss, after 1 s and then twice as long each time up to 30 s. `ota` and `itest` ask through it: their status checks, cancels and the suite's requests are the server's session's. Only `itest`'s deliberate second and intruding sessions, the pushes of an install, `stock-ota` and `install` open sessions of their own.

A read (status, settings, plans, statistics and the like) that got no answer, or whose session dropped under it, goes once more under the same id. Nothing else is sent again: a write that got no answer is reported uncertain, since it may have run, and it is not sent again on any path. A request while the session is paused or unconfigured is refused with the reason, and nothing is sent; one while it is opening waits for it up to 20 s. A second request under an id still out is refused (409).

**Finding the server.** Once it listens, the server writes a per-user pointer to itself, naming its socket, checkout and process, in both `$XDG_RUNTIME_DIR/wellfed-bench.json` (when XDG is set) and `~/.local/state/wellfed/wellfed-bench.json`, so a client whose environment lacks XDG (cron, an ssh session) still finds it. It removes its own when it quits. A pointer whose process is gone, or whose pid now belongs to another process, names no server. One whose process lives but whose socket is missing or refuses fails closed: every device command is refused with that server's pid. While another checkout's server answers, `serve` refuses to start: one server holds the bench.

When the session opens and the feeder counts more sessions than the server's own, the state warns of them (`status` prints it): something reached the feeder without the server.

**The `wellfed` CLI.** Every device command reads that pointer first, whatever its working directory or key file. While the server it names answers, a command whose key file holds the server's key (a copy counts) goes through it, and a direct session is never opened beside it or used as a fallback. Everything else is refused, naming the server: another key, `--address`, and `setup`, `install`, `recover`, `join` and `restore-stock`, which open sessions of their own. The one exception is the server's own job. A command that descends from the process the pointer names passes only while the server's state shows its session paused for that running job's own sessions (`stock-ota`'s or `install`'s pause). Paused for a UART job, for power-off or for the server's own install push, or not paused at all, the job is refused too. A server that does not answer refuses its jobs like any other command.

`events` and `feeds` read the history the server's session kept, and `logs` its log lines, so neither moves the server's own subscription or tail; `--samples` is refused. The server keeps the newest 4096 events and gaps, and 1024 log batches, apart. The held history is one epoch's: after a store reset (a new epoch) it starts again, empty, and numbers from two epochs are never compared.

History older than the server holds is refused when the command subscribes, before it sends anything else: `feed` subscribes first, so a feed never runs ahead of a refusal of its own events. The refusal names the event the server's copy starts after: pass `--after` that number, or stop the server to read further back. `logs --after` below what the server holds is refused the same way; plain `logs` shows what it holds, with a line saying older lines were dropped.

A command takes what it wants of the server's ring when it starts and reads on from the ring's end, so the messages it waits for (a feed's own `Fed` among them) never overrun its place, however full the ring. Only a follower that more than a whole ring of new messages outruns while it waits stops with an error, rather than skip them.

#### The UART capture

While no job runs and relay 1 is on, the server holds the UART with back-to-back receive-only captures, `private/captures/bench-MILLIS-uart-NN`, through the same capture chain the suite uses. Its lines feed the state's UART tail as they arrive. `PLFW1 start` checks the bank ledger, and the last `PLOTA1` record is shown.

Before every job it stops the capture, killing its observe (which only receives), and waits for it to let go of the port; it starts a fresh one after the job, and when the relays read relay 1 on again. The decision to start one is made under the same lock that accepts a job, so a capture never starts under a job that is starting. A capture that stops on its own, a missing port say, stays stopped with its reason until then.

`wellfed-flash` caps one observe at 180 s, so an idle bench writes about 480 segment directories a day, each with its `serial-rx.bin`, `serial-tx.bin`, events and report. Clear old `bench-*-uart-*` directories from `private/captures/` now and then.

#### The bank ledger

`private/bench-status/banks.json` records which build the bench believes each bank holds, from the jobs that wrote it:

| Job | Records |
| --- | --- |
| `flash-execute` | The plan's bank: its target review's startup identity, or the stock image, or else the review's OTA image hash, which no report names and so is never checked |
| `give-way-execute` | Bank 1 holds no valid image |
| `flash-execute` or `give-way-execute` that stops or is cancelled once the flash client ran | That bank is unknown, until a write or a seed settles it. Any non-zero exit counts, 2 as well: `wellfed-flash` exits 2 for any error its command returns, not only one before serial access. |
| `stock-execute` | Bank 1 holds stock |
| `ota` | Confirmed: bank 1 holds the offered build (or stock). Fell back: bank 1 holds no valid image. `--fault give-way-only`: bank 1 gave way |
| `stock-ota` | Stock's install: bank 0 holds the build and bank 1 no valid image. The promotion: bank 1 holds the build |
| `itest` | Each install the suite watched: confirmed, bank 1 holds the build; fell back, no valid image |
| `install` | Once the installer started, by its exit alone. Exit 0: bank 1 holds the review's build, and bank 0 too unless `--from-bank0`. Any other end, a cancel included: those banks are unknown |

Each entry is then checked against what the feeder says it runs: the `PLFW1 start build= bank=` banner on the capture, and the build and bank in the session's status. A report that names another build marks the entry disputed rather than replacing it, since either side may be wrong; the operator decides. `banks --seed DUMP_SESSION` starts the ledger. Today's flash inspections pin region hashes and name no build, so the seed takes `--bank` and `--build`, and says so. A seed is refused while a job runs. Like every file the server keeps, it is replaced whole and never synced, and nothing writes it while a job holds the UART. A `banks.json` that does not read refuses `serve` before the relay board opens, and is left for the operator to repair or remove.

#### The console

On a terminal, `serve` draws its console full screen on `/dev/tty` with the same panels as the page: the UART-write banner with the sectors verified, the job with its step and next steps, Activity, the relays and the lease, the feeder (banks, session, status, clock, journal high water, hotspot, capture and the device log's last lines), the operator's step, timers and evidence by card, and a scrolling log of every job's output. PgUp and PgDn scroll the log (Up and Down a line), and End follows it again.

While a job asks a question, it shows in a panel above the log, with its number and the `wellfed-bench answer` line that answers it. The console only shows it: answer on the page, with that line, or through the agent.

`q` or Ctrl+C stops the server the same way SIGINT does: cancel the job, wait for its cleanup, then release the bench. During a UART write or a loader session the first press only shows the confirmation in the footer, and a second within 5 s stops ([above](#the-bench-server)). While the flash client writes, the screen redraws at most twice a second; otherwise four times. It draws from the live state and a channel only, so a slow terminal never holds the output pump: it loses console lines instead, which the job's stream and log file keep. When the server stops, the screen closes and the log's last 30 lines are printed on the terminal.

Without a terminal (a service, a pipe), or with `--plain`, the console is plain lines on stdout, as before.

#### The operator's notes

`wellfed-bench note` posts to the server, which appends each note to `private/bench-status/notes.jsonl` (through a buffer, never synced, and not flushed while the flash client holds the UART) and shows it on the console and the page:

```
wellfed-bench note step "Install the candidate over UART"
wellfed-bench note next "Boot and watch the banner" "Run the smoke tier"     # replaces the list
wellfed-bench note timer fire --in 5m --expect "PLSCH1 fire"                 # or --at 18:00
wellfed-bench note timer reboot --in 2m --expect-event boot --window 5m
wellfed-bench note timer fire --clear
wellfed-bench note evidence --card schedule private/captures/boot-01 --pass --text "banner seen"
```

A card is the operator's own name for the work, free text. Evidence must name a file or directory inside `private/`, given from the working directory; the server keeps it root-relative and refuses anything else.

**Timers.** `--at HH:MM` is the next time the bench host's clock reads it; `--in` takes lengths such as `90s`, `5m` or `1h30m`. A timer with `--expect TEXT` is marked seen by the first UART line, device log line or job output line containing `TEXT` from its start until its window ends; `--expect-event KIND` does the same for a device event of that kind (`boot`, `fed`, `clock_sync`, as the device API names them). The window ends 2 minutes after the due time unless `--window` says otherwise. A timer still unseen then is marked missed. Both are notes the server appends, so a restarted server shows the same. Lines reach the timers from the capture while no job runs, the session's device log, and the running job's output, so a `boot` job's own banner counts.

#### MCP

`wellfed-bench mcp` serves the bench's tools to an agent over the Model Context Protocol on stdin and stdout. It is a client of the running server like every other command, holding nothing of its own, so it needs `serve` running in the same checkout. Register it with the agent from the checkout's root, for example in `.mcp.json`:

```json
{
  "mcpServers": {
    "wellfed-bench": {
      "command": "wellfed-bench",
      "args": ["--root", "/path/to/wellfed", "mcp"]
    }
  }
}
```

Every tool's description says what it does physically.

- **Reads:** `bench_status` (the live state, with its tails cut to 20 lines), `job_output {id, after}`, `logs_tail {source: device|uart|job, lines}`, `banks`, and `wait_for {for, …, timeout}` over `GET /wait`, with the timeout in seconds; `for: prompt` waits for a job's question.
- **Notes:** `note_step`, `note_next`, `note_timer` and `note_evidence`, the same notes as `wellfed-bench note`, built straight from the arguments, so text may start with `-`. On the command line such text goes after `--`: `wellfed-bench note step -- "- flash bank 1"`.
- **The feeder:** `device_read {op}` asks the server's session for one read: status, settings, time, plans, clients, update state, calibration, the meal call or a stats section. `clock_sync` sets the clock from this host. Nothing else goes to the feeder's device API from here: never a feed, a cancel, a plan, a setting or an update. The panel is another matter: `press` on a `virtual-panel` build has a finger's effects, so any press with Dispense needs `confirm_motion: true` (it can feed; the hopper must be empty) and a Reset hold over 1000 ms needs `confirm_destructive: true` (one of 5 s on an unlocked panel erases the enrolled keys, and a stall in the button task can carry a hold just short of 5 s past it). An agent that opens the recovery window with `lock+dispense` still cannot answer an install's `open-recovery-window` question: a person does. A read that comes back uncertain is reported as such.
- **Hardware:** `job_cancel`, which the server refuses during a UART write unless `during_write: true` (the write then stops for good), and one tool a command: `power {state: on|off}`, `boot`, `capture`, `loader`, `dump`, `flash_prepare`, `flash_execute` (`plan_sha256` required), `give_way_prepare`, `give_way_execute`, `stock_prepare`, `stock_execute`, `ota`, `itest`, `transients`, `install` and `press`. `install`'s `virtual_panel` needs `confirm_motion: true`, as a Dispense press does, since the job presses Dispense itself. Each builds the command's words, with the session name after `--`, and posts them as a job, so the server's parse and every check apply as they do to a person's command line; it answers with the job's number, to follow with `wait_for {for: job, id}` and `job_output`.

- **Questions:** `job_answer {seq, answer, id}` answers the question `bench_status` shows (`prompt`), once. It reads the question first and refuses two that only the owner may answer, at the feeder: whether this is their feeder (`recognize-feeder`) and the recovery window at the panel (`open-recovery-window`). Those are answered on the page or with `wellfed-bench answer`, or the feeder check ahead with `install`'s `expect_device`, which the owner gives. An agent may answer `key-setup-code` and `press-dispense`: the feeder's own check of the presses decides, and the answer only lets the installer ask its next question.

Refused over MCP, before anything is posted, as they need a person at the bench: `attended_motion`, the suite's `feeds`, `break_lease`, a `destructive` suite run without `confirm_destructive: true`, and those two presses without theirs. Arguments a tool does not name are refused too, and so is any string a hardware tool is given that starts with `-`, so none reads as a flag. A session name never starts with `-` anywhere. Stdout carries the protocol alone; nothing else prints there.

### Flash over UART

1. With the server running, prepare the update:

   ```sh
   target/release/wellfed-bench flash-prepare NAME
   ```

   Add the updater's flags (`--bank`, `--source`, `--original-read`, `--target`) as the update needs. You should see the plan summary and the exact `flash-execute` line to run next.

2. Review the plan: bank, sectors, erases, image hashes and the plan SHA-256. This is the check before anything is written.

   > [!WARNING]
   > The next step writes flash. Keep the feeder powered between the two commands, and don't rebuild `wellfed-flash`.

3. Run the printed `flash-execute` line. You should see the update complete and the feeder boot.

`flash-prepare` and `flash-execute` are separate on purpose, so you review the plan between them. A failed step stops without retry and leaves the feeder powered for inspection. A stopped update is never followed by a boot. Finish it with a fresh `flash-prepare … --interrupted` session, as the [development updater](../../development/uart-development.md) describes.

Relay state shows that the supply was not switched off between the two commands. It does not rule out a brownout.

Stock recovery keeps its own commands. Use `loader` to enter the ROM, then run `wellfed-flash stock-recovery` directly, or use [`stock-prepare`](#to-stock-over-uart).

## Bench lease

One tool drives the bench at a time. Before they touch anything, these take the lease `private/bench.lease`, and remove it when they end, on a stop or Ctrl+C as on success:

- `wellfed-bench serve`, for as long as it runs. The commands that are its jobs take no lease of their own, and `status` only reads it;
- every `wellfed-flash` command that opens the serial port (`read`, `update`, `give-way`, `stock-recovery`);
- `wellfed-stock setup-send`;
- `wellfed-stock serve --enable-ota-trial`, for as long as it runs;
- `wellfed install`, for as long as it runs.

The server never stops for an install: its `install` job runs `wellfed install` on its lease. Run from another terminal while a server holds the bench, `wellfed install` is refused with the server named.

The lease names its holder: its pid, its start time and boot (so a reused pid is not mistaken for it), its host, its command line and when it started. A second tool is refused with that holder named, before it opens a port or switches a relay:

```
Stopped: The bench is held by pid 81234 (`wellfed-bench serve`, since 2026-10-26 12:00:00 UTC on bench); wait for it to finish or stop it
```

**Chained tools.** A process descended from the holder runs on the holder's lease and leaves it in place. So the children the server's jobs start (`wellfed-flash` captures, reads and updates; the `serve --enable-ota-trial` of `itest` and `stock-ota`; `wellfed install` and its stock server) never wait on their own parent. That is checked once, when the child starts, and a job waits for or stops its children before it ends, as the server waits for its job before it releases. Run by hand, `wellfed-flash` takes the lease itself, so a hand-run read cannot open the UART under a running server.

**Tools that take no lease.** The progress page, alone or as `serve --ui`, only reads; under the server, the server holds the lease. A client only asks the server. The offline `wellfed-flash` commands (`plan-stock-recovery`, `decode-status`, `inspect-journal`) and an emulated helper (`WELLFED_FLASH_EMULATOR`, the host tests) take none.

**The stock server.** `wellfed-stock` keeps the lease beside its `--config`, which on the bench is `private/`.

- A trial-enabled `serve` holds the bench for its whole life, because the install it dispatches runs for minutes after the request. A `wellfed-bench serve` from another terminal is refused until it stops.
- `ota-trial` takes no lease of its own. It only asks that server, which already holds the bench, so it works from any shell.
- A plain `serve`, `broker` and `ota-images` take none. They only answer the feeder, beside the bench command that drives it (`ota` beside an operator's `serve`).
- `network apply` and `network remove` take none either. They change the host's routing, not the bench, and they run under sudo, where a lease would leave a root-owned `private/bench.lease.lock` that later runs as the operator could not open. If one ever does (`Permission denied` on that file), remove it with sudo; it holds nothing.

### After a killed run

A server killed outright (SIGKILL, a closed terminal) cannot remove its lease or run its job's cleanup. It may have left relays on, a capture running or an update half done. The next server refuses rather than take the bench over:

```
Stopped: The bench lease of pid 81234 (…) outlived its process: that run was killed and may have left relays on or an update half done. Check `wellfed-bench status`, then free the bench with `wellfed-bench status --break-lease`
```

With no server, only `status` runs, and that is the way back:

1. Run `target/release/wellfed-bench status`. It shows the lease first (free, held, or left by a process that is gone), finds no server, and reads the relays itself, without taking the lease.
2. Stop what the killed server's job left running. Under a lease whose holder is gone, `status` lists every `wellfed-flash`, `wellfed-stock`, `wellfed-build` and `sigrok-cli` running on this bench, with its pid and command line. They no longer count as the bench's. Let a `wellfed-flash` finish, or stop it with SIGINT (`kill -INT PID`) so it records its stop. Stop the others by hand.
3. Run `target/release/wellfed-bench status --break-lease`. It refuses while a `wellfed-flash` still runs on this bench, naming it, since it may be writing. Otherwise it removes the lease, only when its holder is gone, then shows the relays.
4. Start the server again.

The tools are found by process name and by the `private/` of their working directory: every job's child runs from its server's checkout, and the lease lives in that `private/`. So a worktree whose `private/` links to the main checkout's shares its lease and finds the main server's tools, a checkout with a `private/` of its own never counts them, and the killed server needed to record nothing. Against a live holder, `--break-lease` refuses for the live lease, whatever its children.

`--break-lease` never breaks a live holder's lease, one written on another host, or a file that is not a lease. Remove the last two by hand once the bench is idle. Against a file that is not a lease, `--break-lease` stops before the relays; plain `status` still shows them.

While a server runs, `status` shows the relays as the server last read them, since the server holds the board's port. While another holder keeps the port open (a hand-run tool), `status` shows the lease and then stops with "Serial port: Device or resource busy" instead of the relays.

A server killed outright also leaves `private/bench.sock` behind. The next server replaces it once it holds the lease, and until then a client finds nothing answering and says to start one.

Taking, breaking and releasing run under an advisory lock on `private/bench.lease.lock`, which the kernel drops with the process, so two tools racing for a free or stale lease cannot both win. The lease is replaced atomically, so `status` never reads it torn. The lease and its rules are `wellfed_formats::bench::lease`.

## Over-the-air updates

`ota` runs one [over-the-air update](../../firmware/ota-design.md) of the feeder that is already running: build, checks, push over the device API, reboot, then build identity and status. The hotspot must already be up. The bench never starts it, and a push that cannot start stops before any capture. A bank-0 change still needs the UART updater.

```sh
target/release/wellfed-bench ota unit-01 --build                     # build, review, offer, verify
target/release/wellfed-bench ota unit-02 --review private/verification/<review>
target/release/wellfed-bench ota stock-01 --review private/verification/<stock review>   # back to stock
```

What a run does:

1. **Build** (`--build` only). Runs two children from the root, their output streamed to the progress log: `wellfed-build build --profile firstboot` (beside the bench binary, or `--build-bin`), then the review `wellfed-build review build --build private/firmware-build/plaf103/firstboot --output private/verification/NAME-review`. A nonzero exit from either stops without signing or offering. The run then continues as with `--review` on that directory.
2. **Sign.** The bench signs the review in process with `wellfed-build`'s signing library and `private/ota-key.json`. It verifies the review as `wellfed-flash` does, refusing a directory with no `review.json`, an image or manifest that differs from it, a non-pinned "stock" image, or an unattended motion policy. It signs a version 2 manifest.
3. **Push** over the device API as `--keys`: the offer, bank 1's give-way, the offer again to bank 0, the chunks, and a resume after any drop.
4. **Watch.** Receive-only UART captures run until the new build confirms, falls back, or `--minutes` (20) pass.
5. **Check status.** The device API is polled until it shows the build on bank 1.

**Choosing a build.** `--firmware qa|optics|buttons|adc|speaker|blink|trial|feeder` picks the build (default `qa`), and `--variant` picks a variant of that firmware:

- `stall|fail` with `--firmware trial`;
- `entropy|owner-stall|held-stall|gap-probe|feed-stall|stop-fail|owner-fail|twin|virtual-panel` with `--firmware feeder`.

`feed-stall` and `stop-fail` move the motor only for a feed request, like the feeder, and inject a fault into it ([motor pulse](../../firmware/motor-pulse.md)). `owner-fail` never opens its feeding owner, so its trial must fall back. `owner-stall` resets about 3 minutes into every boot once confirmed, a reset storm ([OTA design](../../firmware/ota-design.md)).

The bring-up areas and the trial images are variants of one `bringup` firmware. So `--firmware optics` builds `wellfed-build --firmware bringup --variant optics` (likewise `buttons`, `adc` and `speaker`), `--firmware trial --variant stall` builds `--firmware bringup --variant stall`, and `--firmware trial` alone builds its normal image. `qa`, `blink` and `feeder` pass through unchanged. The motor pulse (`--firmware motor`, `--variant pulse`) is refused: motion builds stay manual, then go through `--review` with `--attended-motion`.

**Refused before anything runs:** an existing `NAME-review`, `--firmware` or `--variant` without `--build`, `--variant` without the firmware that declares it, and `--build` with `--review`.

**Other options:**

- **Version:** `--version`, or one more than the highest version among the signed manifests in `private/verification/`. The manifest is written to `private/verification/NAME-vN.manifest`.
- **Build ID:** the review's `startup_identity`. A review whose `startup_motion_policy` is set needs `--attended-motion`, as for `flash-prepare`.
- **Downgrades:** stock gets a downgrade statement for the feeder's identity and floor. `--downgrade` signs one for a version at or below the floor.
- **Faults:** `--fault corrupt@N`, `unwanted@N`, `stall@N` or `drop@N` spoils the bench's own transfer. `--fault give-way-only` stops once bank 1 gave way, to offer bank 0 another manifest next.
- **Cancel:** `ota NAME --cancel` sends only a `CancelUpdate`. It forgets the candidate a give-way or a stopped install left recorded; without it, bank 0 refuses any other manifest (`Pending`). Run it between failure drills.
- **Tools:** no server runs. The bench signs by linking `wellfed-build` and offers over the device API.
- **Captures:** 60 s segments `private/captures/NAME-01`, `-02`, … A name with any numbered segment is refused before building or signing.

**Outcomes.** The `PL…` lines are read as each segment ends. Progress shows on stdout and the page: the give-way, the manifest, written sectors as a bar, activation, the new build's start and trial boots.

| Outcome | Lines | Exit |
| --- | --- | --- |
| Passed | `PLFW1 start build=<ID> bank=1`, then `PLCNF1 confirmed=1`, then the status check below | 0 |
| Health missing | Confirmed on the UART, but the status never showed the build within 90 s | 1 |
| Fell back | `PLTRL1 give_way=1` after activation or the new build's start; reports the last `PLTRL1 confirmed=0 boot=N` | 1 |
| Update error | `PLOTA1 error stage=…` | 1 |
| Refused | The push ended before any UART outcome: the feeder refused the offer (a downgrade without a statement, a manifest bank 0 does not install, another signer) or reported a stop over the API | 1 |
| Timeout | None of these within `--minutes` | 1 |

**Status check.** After the confirmation, the device API's `Status` is read as `--keys` every 5 s for up to 90 s. It requires the offered build on bank 1, and reports the uptime, the boot count, the Wi-Fi source and whether the clock is valid. A changed reason for waiting (an older build, another bank, the request failing) is logged once.

**Timing.** Once any child has run, the command ends with a timing summary: build, review, sign and offer, give-way to activation (the download), activation to confirmation, the status check, and the total. The bench measures build, review and offer itself. The two feeder phases come from each capture's `serial-events.jsonl` chunk times; without that file they fall back to the end of the 60 s segment and say so.

After a fallback, bank 0 refuses the manifest that failed its trial, so sign a new version. A cancel stops the child, the capture or the poll, and the job.

To install stock instead, see [to stock over the air](#to-stock-over-the-air).

## Integration suite

`itest` runs named real-device scenarios against the feeder on the bench. It is a bench command, not a test target. Builds go in over the air. The UART stays attached, receive-only, for the whole run: it is what the scenarios assert on, it is the evidence, and it is the way back when the feeder stops answering.

The suite is the `wellfed-itest` crate, which the bench server links. `itest` and `stock-ota` are jobs: the server runs the scenarios on the rig in its own process, so the run, its cancel and its exit status are the job's, and the client exits with that status. `--list` answers in the client, with no server.

```sh
cargo build --release --bins --examples
target/release/wellfed-bench itest --list
target/release/wellfed-bench itest itest-YYYYMMDD-01 --build --tier smoke    # a few minutes
target/release/wellfed-bench itest itest-YYYYMMDD-02 --build --power         # standard (default tier)
target/release/wellfed-bench itest itest-YYYYMMDD-03 --build --power --destructive --tier soak --soak-minutes 120
target/release/wellfed-bench itest itest-YYYYMMDD-04 --review private/verification/<review> --only requests,request-auth
target/release/wellfed-bench itest itest-YYYYMMDD-05 --review private/verification/<review> --only feed-writes --feeds
target/release/wellfed-bench itest itest-YYYYMMDD-06 --build --power --destructive --feeds --only sustained   # the 8-hour run
```

**Tiers.** `--tier smoke` runs `ota-update`, `requests` and `request-auth`, which takes minutes once the build under test is running. `standard`, the default, runs the whole functional suite. `soak` adds the long runs, `soak` and `sustained`. `--only` picks scenarios whatever their tier. The [sustained run](#sustained-run) is meant to run alone.

**Builds.** `--build` first asks `wellfed-build source-id --image` for the identity a build of these sources would embed. That identity covers every source input, the firmware, the variant and the update key. If a complete review with that `startup_identity` is already in `private/verification`, the suite signs it rather than building again. `wellfed-bench ota --build` does the same, and `--rebuild` builds anyway. So after the first run on unchanged sources, neither the build under test nor the trial firmware's `fail` image is rebuilt.

### Scenarios

| Scenario | Checks | Needs |
| --- | --- | --- |
| `ota-update` | The build under test (`--build`, which builds the `feeder` firmware, or `--review`) installs over the air and confirms, unless the device API already shows it on bank 1. It always runs first, then syncs the feeder's clock, and every other scenario needs it. | |
| `requests` | Over the device API: status, with `PLAPI1 request … op=status`; a clock sync, after which the feeder's clock is within 5 s of the host's; a lights change against the generation just read saves a newer one (`PLCFG1 saved`), reads back, and is restored. | |
| `request-auth` | A session with an unenrolled key is refused (`OpenRefusal::Unknown`, `reason=unknown-key`); a handshake with its last byte flipped is dropped (`reason=noise`); the owner's session still works. | |
| `settings-persist` | Lights, sound and volume changed against the base generation; a write from the stale generation must be refused `Stale` and change nothing; a cold boot, then read back; the originals are restored. | `--power` |
| `wifi-fallback` | Credentials stored for an absent SSID (`PLCFG1 net saved stored=1`). After a cold boot, `Status.wifi` shows `Fallback`. A clear (against the Wi-Fi generation) and another cold boot return it to `Build`. It refuses to start while credentials are stored. | `--power` |
| `network-bounce` | The hotspot goes down for 20 s and comes back. The device API answers again with the same boot count and more Wi-Fi losses (association, lease or gateway). | |
| `feed-writes` | Writes from a second session during a feed, then a cancel ([details](#feed-writes)). | `--feeds` |
| `ota-recovery` | The trial firmware's `fail` image installs and fails its trial, and KM0 gives way to bank 0, which answers the device API. The suite then pushes the re-signed baseline to it with one fault each, in its own transfer: a flipped byte → `stage=hash`, a chunk ahead of the wanted ones → `order`, 75 s of silence → `timeout`, none of them activating. Bank 0 keeps answering after each stop. A clean push then confirms and restores the baseline, so the fallback needs no restore install of its own. | `--build --power` |
| `ota-power-cuts` | Installs the feeder's `twin` variant: the same firmware under another build identity, so it still serves the device API. It has to be a build the feeder has not confirmed, because a confirmed one skips the trial. Two cuts follow. **Mid-download** (48 sectors in), the next boot must be bank 0, and when the push offers again its first progress record must show the download starting again. **10 s into trial boot 1**, before the 30 s confirmation, KM0 must count boot 1 again (backup registers clear on power loss), and the build must still confirm. A cut during activation itself is not attempted, since the marker write and reset take milliseconds. The re-signed baseline then goes back on. | `--build --power --destructive` |
| `soak` | A device API status every 30 s for `--soak-minutes` (default 60). It fails on any restart, any Wi-Fi loss or any missed answer, and reports the slowest. | soak tier |
| `sustained` | `--sustained-cycles` (default 16) half-hour cycles of feeds every 10 minutes, each cycle opening with one fault from a fixed rotation. Any failed check cuts relay 1 and halts. Defined [below](#sustained-run). | soak tier, `--build --power --destructive --feeds` |
| `stock-roundtrip` | To stock over the air and back through stock's updater ([below](#itest-scenario-stock-roundtrip)). | `--power --stock` |

A scenario whose flag is missing is reported as **skipped**, never as passed. `--destructive` requires `--power`. The design recovers from both power cuts over the air; if it ever does not, recovery is the UART flash the suite prints.

#### feed-writes

The scenario holds two owner sessions: the suite's own, and a second held open from before the feeds, since the feeder admits no handshake while a feed holds its outputs.

1. A 2-portion feed on the first session. Once the motor drives (`PLOWN1 state=4`), the held session sends a lights change against the current generation (it must be refused `Busy`), a lock change and a clock sync, and their answers are logged.
2. No settings-writer save (`PLCFG1 saved`, `net saved`, `keys change`) or confirmation may appear between `PLOWN1 feed id=N` and `PLOWN1 fed id=N`. The feed must end `outcome=1` (Done) with `completed` = `requested`, and the refused change must have changed nothing.
3. After the feed the same change saves, and is restored with the lock.
4. A second feed is cancelled from the held session 2 s into its drive. The cancel answers `Done`, the owner logs its outputs stopped, and the feed ends `outcome=2 reason=2` (Cancelled).
5. The lock change and the clock sync must answer `Done`, and the lock must read the new value after the feed. When the feeder logs a `PLOWN1 interlock` record after the feed began, its `writes_deferred` must have grown.

Throughout there must be no `PLFW1 start` or panic and no boot count change. The event ring logs nothing per append, so a ring write during the feed shows as the reset its failed flash lease causes.

Feeds and cancels are sent once, never resent on a fresh session: the feeder remembers a write's outcome only once its answer resolves, and a failed one is treated as uncertain. After any failure the suite waits for a feed it started to end (`fed id=N`, then `state=1`) before restoring anything. If it does not end, nothing is restored and the run stops, whatever `--keep-going` says. The feeds run unattended, so the hopper may be empty.

### How a run behaves

**Bench state.** Before changing anything, the suite refuses if the UART port is missing, `wellfed-stock` is not built, an evidence name is in use, or anything already answers on the server's `http_listen` (stop that `serve`; the suite runs its own). It then:

1. records whether the hotspot connection named in `private/hotspot.json` is active, and brings it up;
2. with `--power`, records relay 1 and switches the supply on;
3. starts the UART capture;
4. starts `wellfed-stock serve --config private/server.json` as a child.

Updates go over the device API, pushed on a thread of their own while the scenario reads the UART. `serve` stays up for stock's broker. The suite talks to the feeder as the key in `--keys` (default `private/wellfed-owner.json`). It finds the feeder over mDNS or, while the bench host's firewall drops UDP 5353, through the hotspot's neighbour table and `target_mac`. All server output goes to `private/verification/NAME-servers.log`.

At the end, or on an error or Ctrl+C, the suite stops its children and puts the hotspot and the supply back as it found them, except after a `sustained` halt, which leaves the supply off. Without `--power` the relay board is never opened. The suite never flashes over UART and never changes the firewall or routing. Startup-motion builds are not offered. Only `feed-writes` and `sustained`, under `--feeds`, ask the feeder to feed, within the owner's limits: hopper empty, D cells out, at most 3 portions a feed. `sustained` also installs the `feed-jam`, `sensor-fault` and `feed-stall` variants, which move the motor only for a feed request.

**Device state.** Once the baseline confirms, the suite reads the settings, whether Wi-Fi credentials are stored, and the build and bank. It reads them again after every later scenario. A scenario that leaves any of them changed fails, even if its own checks passed. The suite puts back what a request can: settings at once, stored credentials from the next boot.

**Failures.** A failed scenario stops the rest, which are recorded as skipped, unless `--keep-going` is given. Scenarios that change device state restore it on failure too, while the feeder still answers.

If there is no `PL…` record, no device API answer and no fresh bank-0 health for 180 s, the feeder counts as silent. Every wait checks this, so an unpowered feeder stops an install watch within 3 minutes rather than at its 15-minute deadline. Only the wrong-credential wait in `wifi-fallback`, where the feeder is unreachable on purpose, is exempt. The suite then stops whatever `--keep-going` says, prints the last confirmed review and the `flash-prepare` command to recover it over UART, and exits 2. A scenario failure exits 1.

**Cold boots.** After a supply cut the server keeps reporting the old session's health as online and fresh until the broker's keepalive lapses, and a request sent then is lost. So after a cold boot the suite waits for `PLMQT1 ready` and accepts only health received after it.

**Evidence.**

- **Captures:** receive-only, in back-to-back 180 s segments `private/captures/NAME-uart-01`, `-02`, … (the flash client's limit for one observe). A thread starts each segment as the last exits, so the port stays held while the suite blocks on a build or a child. Segments are read while they record, so a record arrives within about a second.
- **Reviews and manifests:** `NAME-good`, `NAME-fail`, `NAME-restore`, `NAME-cut-speaker` and `NAME-cut-restore`, in `private/verification/`. Each signs the next version, so a run uses up several. A reused review keeps its original name.
- **Record:** `private/verification/NAME-itest.json` holds each scenario's verdict, reason and duration, any silence, and the capture list, marked `"evidence": "physical"`.

### Sustained run

The `sustained` scenario (`src/tools/wellfed-itest/src/sustained.rs`) is the long unattended fault run.

```sh
target/release/wellfed-bench itest itest-YYYYMMDD-NN --build --power --destructive --feeds --only sustained \
  | tee private/verification/itest-YYYYMMDD-NN-console.log
# a shakedown of every fault once (4 h): add --sustained-cycles 8
```

**Before it starts (the owner's unattended motion rules):**

1. The hopper is empty.
2. The D cells are out, so relay 1 is the kill switch.
3. Nobody is at the bench.
4. The bench lease is free.

The build under test is this checkout's `feeder` (`--build`). The run also builds the `feed-jam`, `sensor-fault`, `feed-stall` and `twin` variants once and re-signs them later. Each install and each restore signs one manifest version: 8 per rotation of eight cycles, about 17 for the full run with the baseline's. The logic analyzer may stay clipped on; it plays no part.

**Length and cadence.** 16 cycles of at least 30 minutes, about 8 hours. A step that outlasts its half hour (the update cuts can) stretches its cycle, and each cycle's note gives its real length. Each cycle first runs its fault step, then feeds the baseline at once and again every 10 minutes while the cycle lasts (at least one feed a cycle), with a device API status every 30 s in between. Baseline feeds ask for 1, 2, 3, 1, … portions; **no feed in the run asks for more than 3**. By the simulation's timings that is about 70 feeds (36 in eight cycles), fewer on hardware, where an install takes about 4 minutes rather than 2.5. At about 9 s a portion, that is under 25 minutes of drive in 8 hours.

**The rotation.** Cycle n takes step n mod 8, so the full run takes each step twice:

| Cycle | Step | What it injects | How it must end |
| --- | --- | --- | --- |
| 1, 9 | `feed-writes` | The [scenario](#feed-writes) as it stands: settings, lock and clock writes from a held session during a 2-portion feed, then a cancel 2 s into the next | Its own checks; its feeds join the run's journal |
| 2, 10 | Reboot | A `Reboot` request at rest | One restart into the baseline; reset cause `watchdog` (the firmware restarts by letting its watchdog lapse); the clock still valid (carried across a warm reset); no drive after the boot |
| 3, 11 | Cut while reserved | A 3-portion feed, relay 1 cut as soon as the `Feeding` answer arrives, which the feeder sends only once the owner has reserved the feed in its journal, before the drive starts | The next boot logs `fed id=N outcome=5` (uncertain) once; reset cause `power_on`; no drive after the boot; the run syncs the clock again |
| 4, 12 | `feed-jam` | Installs the variant (a trial, then confirmation), feeds 1, 2 and 3 portions, then puts the baseline back | 1: Done with 1 portion (one reversal); 2: Failed, `Jammed`; 3: Failed, `MarkStuck`; the owner Idle after each |
| 5, 13 | Cut mid-drive | A 3-portion feed, relay 1 cut 4 s into its drive | As the reserved cut |
| 6, 14 | `sensor-fault` | Installs the variant, feeds 1, 2 and 3 portions, asks for one more, reboots, puts the baseline back | 1: Failed, `FeedbackTimeout`; 2: Failed, `OutletBlocked`, no drive; 3: Failed, `Sensor`, the owner then in `Fault`; the extra feed refused `Fault`; the reboot clears the hold |
| 7, 15 | Outage mid-feed | The hotspot down for 60 s from 4 s into a 3-portion drive | The feed ends Done with 3 portions on the UART; no restart; the link losses grew; the device API answers again within 180 s |
| 8, 16 | `feed-stall`, then `ota-power-cuts` | Installs the variant: a 1-portion feed (600 ms loop stall) and a 2-portion feed (8 s stall); then the two update power cuts, mid-download and early in the twin's trial; the baseline back | 1: Failed, `Output`, then one restart; 2: one watchdog restart and `outcome=5` at the next boot; then `ota-power-cuts`' own checks |

**Pass criteria.** The run passes only if all 16 cycles pass every check. Each check reads evidence an existing tool or record already produces:

| Criterion | Evidence | Check |
| --- | --- | --- |
| No feed sent twice, none run twice | The suite's own requests (`Ask::Feed` is never resent) and the `PLOWN1 feed id=` and `fed id=` records | Each feed id the run was given has exactly one owner record and exactly one result, before or after a restart; no result names an id the run never asked for; ids only grow |
| Every feed ends as its step says | `PLOWN1 fed id=N … outcome= reason= completed=` | The outcomes in the table above; baseline feeds Done with every portion |
| Uncertain feeds are resolved, not retried | `fed id=N outcome=5` after the restart, and no `state=4` | A cut or a stall's reset leaves exactly one uncertain result and no drive in the boot that follows |
| Counters agree with the journal | `Stats { Counters }` | This boot's `feeds`, `feeds_failed` and `portions` equal the sum of this boot's `fed` records on the UART. Lifetime counts never go back within a boot, and across a reset their stored base (lifetime less the boot's own) never does: a restart refused its save once the early-save budget is spent loses only what the last boot counted |
| No unexpected restart, panic or fault | `PLFW1 start`, `PLPNC1`, `PLTRL1 storm=` and `give_way=1` on the UART; the status `boot_count` | None at rest or during a feed; each planned restart shows exactly one start record, and a warm one moves the boot count by one (it lives in a backup register a power cut clears); `Stats { Counters }.reset` names the planned cause; no `PLPNC1` anywhere in the run |
| No motion at rest | `PLOWN1 state=4` | None outside a feed the run asked for |
| Memory | `Stats { Memory }`, after each baseline feed | Least free heap at least 64 KiB (provisional: half the 141,792 B measured for the product image); free heap within 8 KiB of the first reading in the same boot of the same build |
| Clock | `GetTime`, after each feed | When valid: error bound at most 120 s, and the host's time within that bound plus 2 s; valid after a reboot; an invalid clock (after a cut, or an expiry) is synced and counted |
| Updates install and confirm during the run | The install watch (`PLOTA1`, `PLTRL1`, `PLCNF1`) and the status build | Each variant and each restore of the baseline ends `Passed`; `ota-power-cuts` passes |
| The feeder stays reachable and as it was | Status every 30 s; the device state check | Every status answered, no Wi-Fi loss at rest, and after every step the baseline on bank 1 with the run's settings and Wi-Fi source |

Each cycle ends with a note of the figures so far: feeds, polls and the slowest answer, least free heap, the largest clock skew and bound, and clock syncs. With the UART captures and `NAME-itest.json`, they are the run's evidence.

**Stop conditions.** Any failed check, a feed whose result does not arrive within 100 s, a silent feeder (180 s), an install that does not confirm, and the operator's Ctrl+C all halt the run the same way:

- the suite **cuts relay 1 and leaves it off**;
- it runs no further cycle or scenario;
- it puts the hotspot back as it found it, and records why.

A host panic does the same: until the run ends cleanly, tearing the rig down cuts relay 1. Nothing is restored on the feeder, so a halt during a `feed-jam`, `sensor-fault` or `feed-stall` step may leave that variant on bank 1 (the halt note says so). Install the baseline before any other run. **A person looks before power returns.** The suite's UART recovery applies if the feeder then does not boot.

**Kept out of the unattended run (the owner's rules):**

- Real food: portion weights, food through the outlet beam, and the weighing session.
- Real jams with food in the rotor, taped or blocked sensors, a full hopper.
- Batteries: the D cells, adapter changes, brownout and the low-voltage floor, so the run has no low-power step.
- Button presses (Dispense during a feed) and LED observation.
- The `stop-fail` variant's 8 s held stop, which leaves the motor driving until the watchdog, and `owner-stall`. Both have run attended.
- DHCP-server loss without an access-point loss, and lease expiry. They need a host network change, which the authorization does not cover.

## Stock and back

Three bench routes cover stock 1.1.15:

- to stock over the air (`ota` with a stock review);
- to stock over UART (`stock-prepare` and `stock-execute`);
- from stock to Wellfed over stock's own updater (`stock-ota`).

`itest`'s `stock-roundtrip` scenario chains the two over-the-air routes. The design for doing all of this without a bench is in [consumer installation](../../firmware/consumer-installation.md).

### To stock over the air

```sh
target/release/wellfed-bench ota NAME --review <frozen stock review>
```

A review whose `review.json` says `image_kind: stock` installs through the same signed update as any build. The bench signs the pinned stock payload with its `payload_sha256` as the build ID, plus a downgrade statement for the feeder's identity and floor. The running build gives way, and bank 0's updater writes and verifies stock in bank 1 before the marker.

The command passes on stock's `app_main:version:` banner after activation and skips the status step, since stock does not report to this server. Stock's own KM0 then runs, so our trial counter never counts it. The integration suite refuses stock reviews.

> [!WARNING]
> Stock has no trial. If stock failed to boot from bank 1, the only way back would be UART [stock recovery](recovery.md).

### To stock over UART

Two reviewed commands, shaped like `flash-prepare` and `flash-execute`:

```sh
# Bank 1 still holds a valid image: give way to bank 0 first.
target/release/wellfed-bench stock-prepare NAME --fallback <bank-0 review>
target/release/wellfed-bench stock-execute NAME --fallback <bank-0 review> --expected-plan-sha256 <sha>
# Bank 1 invalid (a give-way, or just after a stock-ota install): recover.
target/release/wellfed-bench stock-prepare NAME
target/release/wellfed-bench stock-execute NAME --expected-plan-sha256 <sha>
```

`stock-prepare NAME`:

1. enters the loader;
2. runs `read small`, then `read full`, in one helper session;
3. refuses if bank 1's marker is still valid, since `stock-recovery` refuses a valid target marker;
4. runs `wellfed-flash stock-recovery prepare`, which writes the plan from the pinned baseline backup.

It prints the next command with the plan's hash. With `--fallback`, both commands instead run the give-way to that bank-0 review, under the name `NAME-giveway`, and point back at `stock-prepare`.

`stock-execute NAME`:

- is single-use, and requires unbroken power since the prepare;
- runs `stock-recovery execute` with the plan hash, which reads all 8 MiB three times, about an hour;
- boots, and must see stock's banner.

The boot capture holds the unit's serial and cloud keys, so keep it private. The bench reads the plan through `wellfed-formats`' `StockRecoveryReviewed`.

### Stock to a replacement (`stock-ota`)

`stock-ota` has a feeder running stock 1.1.15 install a Wellfed build through stock's own MQTT-triggered HTTP updater, with no UART write ([stock OTA requirements](stock/ota-requirements.md)). It assumes stock is installed, and proves it first: the server must see stock report 1.1.15 on its adapter, which is also the server's own gate for the request.

```sh
cargo build --release --bins --examples          # from a clean commit: --enable-ota-trial refuses a dirty build
target/release/wellfed-bench stock-ota NAME --power                     # default firmware (qa)
sudo target/release/wellfed-stock network apply --config private/server.json   # once it reaches stock-ready
sudo target/release/wellfed-stock network remove                               # afterwards
```

**Routing is the operator's.** Stock connects to its vendor broker on TCP 1883. Only `network apply`, a host firewall change, redirects the feeder's traffic to this server, and only the operator runs it and removes it.

- **Image port:** `network apply` also accepts the image server's TCP 8088 from the feeder's MAC. Without that rule, stock connects for about 25 s and then reports `OTA_INFORM` error 2003.
- **Timing:** `network apply` refuses until a broker listens, so run it once the run reaches `stock-ready`. Without it stock never reports, and the first stage fails after 5 minutes with nothing sent.

**Stages:**

1. **Build.** Before the bench is touched, the firmware is staged as `ota --build` does: build, review, sign the next version. Stock ignores the manifest; the promotion uses it.
2. **`stock-ready`.** The bench comes up as in `itest`, with `serve --enable-ota-trial`. The stage then waits for fresh stock telemetry with `softwareVersion` 1.1.15 and `powerType` 1.
3. **`stock-settle`.** An earlier request that failed or is uncertain blocks a new one. The run asks the server every 30 s to resolve it, for up to 25 minutes.
   - **The server's evidence:** 20 minutes since that request, 10 quiet connected minutes, and a fresh 1.1.15 version report.
   - **Version report:** idle stock reports its version only around a connect, so a refused resolve makes the server send stock a read-only `ATTR_GET_SERVICE`.
   - **Heartbeat:** stock heartbeats about every 61 s, which the server's 90 s telemetry bound tolerates.
4. **`stock-request`.** `wellfed-stock ota-images` serves the review's `image.bin`, bound to its `ota_sha256`, to the feeder found by its MAC. Then one `POST /api/ota/trial`. The server persists the request, then sends stock one `OTA_UPGRADE` with:
   - target version `2.0.0`;
   - the image's SHA-256;
   - a URL of about 80 characters, under stock's 128-byte parser buffer.

   It is never resent.
5. **`stock-install`.** Stock's `OTA_PROGRESS` and `OTA_UPGRADE` reports are printed as they arrive, and a reported error fails the stage. Stock writes bank 0 and invalidates bank 1, so the stage passes only when the first `PLFW1 start` afterwards is the build, from **bank 0**.
6. **`bank0-health`.** The build answers the device API from bank 0 while it waits for an update. Stock reports nothing after activating it and the build speaks no MQTT, so the run tells the server what it saw (`POST /api/ota/trial/answered`: the build identity and bank 0 the device API reported). The server checks that against the build it served and closes the request as `replacement_healthy` once stock's telemetry has gone stale (90 s), so the run asks every 10 s for 120 s. The next request then needs no 20-minute resolution first. A server that will not close it (stock still reporting, or its version reported after the request) is noted, not failed; the next request then waits for the resolution instead.
7. **`promote`.** The same build is pushed, signed, over the device API. Bank 0 installs it into bank 1, and the trial boot confirms. The feeder ends in the normal two-bank state, with bank 0 holding the same build. `--no-promote` stops after `bank0-health`.

Unknown on hardware: whether stock posts its `softwareVersion` between taking the command and rebooting into the build. If it does, every install fails the `bank0-health` fast path closed and waits for the resolution.

**After it:**

- The run restores the hotspot and the supply as it found them.
- It writes `private/verification/NAME-itest.json` with the stage verdicts, so the page's run history shows it. Server output goes to `NAME-servers.log`.
- If anything failed after the request was sent, the run prints the UART recovery pointer: bank 1 may be invalid.

### Install as a job

`install` runs `wellfed install` ([consumer installation](../../firmware/consumer-installation.md)) as the server's job, so the server never stops for it. It takes the installer's bench flags, `--review`, `--stock-config`, and `--broker-name` (the DNS route) or `--from-bank0`, with `--manifest` or `--update-key`, `--home-network`, `--ble-address`, `--expect-device`, `--enroll-minutes`, `--calibrate-clock`, `--without-stock-plans`, `--keys` and `--address` passed through. `--virtual-panel` is the server's own: the installer never sees it. The setup-frame route (`--ssid`) and `--home-ssid` read a passphrase at a terminal and are no job; the bench uses the DNS route, and a consumer image's network comes from `--home-network private/hotspot.json`.

```sh
target/release/wellfed-bench install --review private/verification/<review> --manifest private/<manifest>.json \
  --stock-config private/installer.json --broker-name <stock's broker name>
```

- The server closes its feeder session for the whole job (`install`'s pause) and starts the installer once its state shows that, since the installer opens sessions of its own only then.
- The installer writes each question as one line on its standard output (`--prompts lines`) and waits for its answer. When it asks for the feeder to restart, the job does it with relay 1: off for 10 s, then on, whatever came of the cut. That shows as the job's step (`restart feeder`) and a line in its output, not as an operator's note. Every other question goes into the live state (`prompt`: the job, its number, its words and the answers it takes) and waits for an answer from the page, `wellfed-bench answer JOB SEQ done|yes|no` or `job_answer`; the console shows it too.
- An answer is taken only for the question asked now, by its number, and only of its kind (done for an instruction, yes or no for a question). Anything else is refused and never held for later: the installer reads its answers in order, with no number of their own. The job takes the question away before it stops the installer, so an answer the server took (202) is written to the installer unless it has already exited.
- With `--expect-device`, the owner's pin answers the feeder check: the pinned device passes with no question, and any other stops the installer before anything is sent to stock.
- With `--virtual-panel`, the job answers the panel's prompts itself on a running [virtual panel](#virtual-panel) build, from its own thread, as it does the restart. It needs a consumer review, and refuses a bench one before the installer starts: on the recovery route a person unlocked the panel, where a press left over from a code cut short could feed. For `key-setup-code` it presses the code's six buttons in order, 150 ms each; for `press-dispense`, Dispense once. Each press is a `wellfed-flash read press` child under the server's lease, and the job's output records each one (`press 3 of 6, mic`, then the press's own line). Nothing unlocks the panel first: a code being keyed in takes every gesture whatever the lock says, so a Lock hold would count as one of its presses, and a waiting enrollment takes Dispense on a locked panel too. The server's UART capture is stopped for every job and starts again after it, so the presses have the port. The job answers done only once every press echoed and released. A press that did not stops the installer, and the job ends naming it; nothing is pressed again. Before each press the job looks again, and presses nothing more once it is stopping or the installer has exited. The presses add no step rows of their own. Without the flag these prompts go into the live state for a person at the panel, who answers done once the presses are in. The installer goes on without waiting for that answer, since the feeder times its own window.
- Its standard error is the job's output, so what goes with a question (the recovery code beside the recovery window) shows right below it. A line on its standard output that is no question stops the installer. When the installer exits with an error, the error it printed last is the job's.
- A cancel stops the installer through its own SIGTERM handler, which stops its stock server, and takes the question away. The job's end takes it away too, however it ended.

### `itest` scenario `stock-roundtrip`

```sh
target/release/wellfed-bench itest NAME --review <baseline review> --only stock-roundtrip --power --stock <frozen stock review>
```

From the confirmed baseline:

1. **To stock:** the stock review, offered over the air, installs into bank 1, and its banner must appear.
2. **On stock:** the server switches to `serve --enable-ota-trial`. The scenario waits for stock to report, so run `network apply` then, and resolves any earlier request.
3. **Back:** stock is asked to install the baseline, which starts on bank 0 and answers the device API there (closing the server's request as `replacement_healthy`), then is promoted into bank 1. It confirms at once, since it was confirmed before.
4. **Restore:** back to plain `serve`, with the baseline's device-API status fresh.

`--stock` requires `--power`. A failure on stock stops the suite with a stock-aware recovery hint. With the owner's `network apply` rules already in place, nobody has to touch the bench.

## Progress page

Each job records its steps, the flash client's progress lines (bytes read, sectors verified), relay changes and the active capture. The record lives in the server's memory, as its live state (`GET /state`). Files in `private/bench-status/` outlive the server:

| File | Contents |
| --- | --- |
| `jobs/ID-NAME.log` | Each job's output, complete once it ends |
| `history.jsonl` | One line per job, including the reviewed next command after a prepare |

Lines in `history.jsonl` from before the bench server may not parse, and the page skips them. To clear them, delete the file; the next job starts a new one.

### Serve it

`serve --ui` serves the page on `http://127.0.0.1:8780/`, beside the server. It only reads, but for one thing: the answer to a job's question, a card above the job's output with the buttons the question takes (`POST /api/jobs/{id}/answer`, checked by the server as any client's answer is). Answering needs the page's link as the console prints it at start, `Bench page on http://127.0.0.1:8780/#token=…`; the tab keeps it in its `sessionStorage`, so a reload still answers. Opened any other way, the card shows the question with a line saying to open that link. One event stream (`/api/events`) carries the server's live state on every change, the running job's output from the server's memory, and the history and latest capture when they change. `/api/state` answers the state once. The page has no server of its own: with no `wellfed-bench serve`, there is no page.

The page is optional. It is a Svelte app in `src/web/apps/bench`, which the binary embeds only when built with the `ui` feature:

```
make ui                                   # pnpm in src/web/: builds src/web/apps/bench/dist
cargo build --locked --release -p wellfed-bench --features ui
```

A binary built without the feature refuses `--ui` before the server takes the bench. Node and pnpm are needed only for `make ui` and `make check-web`, never for `make check`. The page's TypeScript types (`src/web/packages/api/src/bench.gen.ts`) are generated from the Rust views, and a test in `make check` fails when they are stale (`UPDATE_BINDINGS=1 cargo test -p wellfed-dashboard bindings` rewrites them).

**What it reads.** Only the bench server's live state and job output, the history, the evidence directories and the stock server's status. It links no relay or flash code and never opens a serial port, so it can run beside any job, and nothing it does writes but a question's answer. While the flash client holds the UART (writing flash, or in a loader session) the page reads no file and asks the stock server nothing, for any browser, including one that opens or reconnects then: the history, capture tail, health, sessions, tests and stock server views answer 503 with `Retry-After`, the event stream sends only the live state and the job's output, and the page keeps what it showed until the flash client lets go. The stock server panel asks `wellfed-stock serve` for `GET /api/status` on the address and token in `private/server.json`, every 5 seconds. That read changes nothing on the server or the feeder. When the server is down or not configured, the panel says so.

**Network.** The page binds to loopback by default because it shows private UART output and job output. It answers only requests whose `Host` is `localhost` or an IP address of its own (any address under a wildcard bind) with its port, and refuses others with 421, so a site that points its own name at this host cannot read it from a browser. Names are refused too, a Tailscale MagicDNS name among them, so open the page by its IP address (`http://100.x.y.z:8780/`). `--listen ADDRESS` serves another interface, for example the Tailscale address to reach it from a phone. Reads need no token: anyone who can reach that address can read the page, so prefer a private interface such as Tailscale. An answer needs the token the server makes at every start (16 random bytes), which only the console prints, after `#` in the page's link. A fragment is never sent, so the server and any proxy never see it in a request line; the page takes it out of the address bar and keeps it in the tab's `sessionStorage`. It goes back in an `x-bench-token` header, compared in constant time; an answer without it, or with an old server's, is refused with 403. The header and the JSON body each need a CORS preflight, which this server never grants. `--listen 0.0.0.0` (or `::`) serves every interface, including the feeder hotspot and any container bridges; the owner chose to allow it. Allowing that port through the host firewall is the owner's change. `--ui-port` changes the port.

### Views

The page has four views (keys `g o`, `g t`, `g s`, `g l`; `/` filters the log). A strip on every view shows the relay board as the server holds it, whether the server sees the UART adapter, the relay states as the server last read or set them, the `wellfed-flash` hash, the build and bank from the newest replacement-firmware start banner in a capture, and the running job. While a flash client writes over the UART, a red banner on every view says not to touch the bench, with the sectors verified so far and how long the write has run; the tab title starts with `WRITING`.

- **Overview**
  - **Guided QA** (when the latest capture holds `PLQA1` records) coaches the newest [QA](../../firmware/qa.md) run from its records. It shows what to do now in plain words ("Look at the Wi-Fi light. Lit: press Dispense. Dark: press Lock."), the time left on a button prompt, each check's state with the answers so far, and the result. **Enable sound** beeps at each new prompt. It follows the records only; `wellfed-audit qa` still grades the run. The records are parsed by `wellfed_formats::qa`, which the audit shares.
  - **Now:** the server's job, its outcome and elapsed time, the step it is on and the next few. Steps show their own durations. While the job runs, bars show straight-line ETAs for the tool's own counts and the active capture's time. The next command to run after review is shown with a copy button.
  - **Operator:** the step, the next steps, timers counting down to their due time (with what each expects, and whether the server saw it or marked it missed) and evidence by card with pass or fail, from the server's [notes](#the-operators-notes).
  - **Activity:** the job and each child it runs (flash client, build, stock, analyzer), with when it started. One that clears stays a minute, fading.
  - **Relays:** channels 1 to 4 as the server last read or set them, and when, and the server holding the lease.
  - **Feeder:** the build the bank ledger (`private/bench-status/banks.json`) puts in each bank, how it got there, and whether the feeder's banner or status confirmed or disputed it. Then the server's [feeder session](#the-feeder-session) (open, paused and why, or connecting), the last status, the clock's skew and last sync, the journal high water, the hotspot, the capture's segment, last start banner and update record, and the device log and UART tails. What the server has not read yet says "not known yet".
  - **Stock server:** stock as `wellfed-stock` last heard it: online or not, its reported version, and whether the server holds its broker session. The replacement firmware speaks no MQTT, so it shows nowhere here.
  - **Recent commands.**
- **Tests**
  - **Run in progress:** each scenario with what it checks and its time. The running one shows its current wait and its notes from the log.
  - **Run history:** every `NAME-itest.json` against every scenario, newest on the left. A live run fills the first column. Each row has the pass rate over runs that reached a verdict, the median passing time and the last failure. **Flaky** marks two or more pass/fail changes in the last 20 runs. Selecting a run opens its record: verdicts and reasons, the UART recovery commands if the feeder went silent, and the tails of its captures and `NAME-servers.log`. A run's link (`#/tests/NAME`) can be shared.
  - **Start a run:** composes a `wellfed-bench itest` command from a tier or chosen scenarios and the allowed flags, and warns which scenarios would be skipped. The page only writes the command out; running it is a physical bench operation.
- **Sessions:** session names with evidence parts, report state and age. Selecting one shows each part's report or plan (bank, sectors, erases, image hashes, plan SHA-256) and a copyable execute line. A report still marked `running` whose command is no longer live reads as `ended`: the process stopped without recording an outcome.
- **Log:** the job's output as the server holds it, line by line as it comes, kept to the newest 20,000 lines. It can be filtered, followed and copied, and only the visible lines are drawn. Stops, refusals, faults and NAKs are highlighted, and scenario starts mark sections. The flash client's JSON summaries are coloured by their `error` and `status` fields. Healthy health lines (`outputs stopped, faults none`) and file names such as `-fault-v52.manifest` are not taken for failures. The latest capture's tail sits beside the log.

Light and dark follow the system; the button in the header cycles auto, light and dark for this browser.

**Timed operator scripts.** An optional `private/bench-status/script.json` lists steps (`{"title", "capture", "anchor", "steps": [{"at", "text"}]}`). The page shows the current step large with a countdown, starting when the anchor bytes (for example `PLOPT1 open`) arrive in that capture. After **Enable sound** (browsers need a click), each step beeps (a double beep to move a probe, a long tone at the end), and **Speak steps** reads it aloud.

## Failure modes

| Failure | Effect | Handling |
| --- | --- | --- |
| Relay board loses USB or the host reboots | Normally-open contacts drop: feeder off, strap released | A running update stops like any other power cut; finish it with `--interrupted` |
| Brownout or supply dip while powered | Relay state still reads on | Not detectable here; the updater's readback and status checks are the guard |
| Strap contact fails at its small current (under 1 mA) | Normal boot instead of the loader | `loader` requires the banner and NAKs, otherwise powers off; nothing is retried |
| UART adapter back-powers the feeder through its TX line | Power-off may not fully reset the chip, so the strap is ignored | Seen intermittently with the manual button (cured by replugging the adapter), not in 32 relay trials. Relays 3 and 4 now isolate the UART while off; holding a UART break while off is the fallback. |
| Relay port disappears mid-session | The current command fails; power stays as last set | Cut power by hand if needed, then check `status` |
| Board power also feeds the motor harness | The motor driver is powered whenever the chip is | Only the ROM and flash helper run while flashing, and the driver sits in standby in the loader ([motor driver](motor-driver.md)) |
| UART adapter stops answering control requests | A run stops before its first step | A USB reset of that adapter alone recovered it |

## Evidence

- **Observed, October 22, 2026 (Rust bench):** `status`, `on`/`off`, `boot`, `loader` (99 NAKs) and a no-op `flash-prepare`/`flash-execute` with a captured boot, all with `--ui` serving the page; then a 158-sector bank-0 install with no loader NAK, and a UART give-way.
- **Observed, October 2026 (former Python bench, whose sequences the Rust bench ports line for line):** every command except `transients` ran on the research unit, including a full dump, installs on both banks, the give-way sequence and five refusals that changed no relay and wrote no flash. Relay boots and strap-held loader entries succeeded 16 of 16 times each with the UART adapter connected throughout. Two loader NAKs came in one session, one while `make check` ran beside an install; none came over three installs and four dumps with the host otherwise idle.
- **Observed, October 22, 2026:** `ota NAME --build` ran the whole loop in one command, 5 min 57 s end to end, over three consecutive changed builds with no manual step and no UART write. The classifier, status check and sequence are also host-tested with real log lines, status documents and fake tools.
- **Observed, October 22, 2026:** the integration suite passed the standard tier in 13.6 minutes (`itest-20261022-18`), with each download taking 18–19 s. `ota-power-cuts` passed (`itest-20261022-10`): a cut at 48 of 171 sectors booted bank 0 and the download began again, and a cut 10 s into trial boot 1 was counted as boot 1 again before the build confirmed.
- **Observed, October 23, 2026:** to stock over the air, then back through stock's updater and promoted (`fw41-roundtrip-01`, 6 min 31 s; 336 sectors written by the recovery role). `stock-ota` installed a `qa` build in about 40 s, booted from bank 0, reported health from bank 0, and promoted to bank 1 where it confirmed (`fw40-stock-ota-05` to `-07`). The stock UART route took about 65 minutes; `stock-prepare` and `stock-execute` are host-tested, and the same `wellfed-flash` steps ran physically.
- **Observed, October 23, 2026:** the progress page's feeder panel followed a live stock server, and its run record matched the suite's.
- **Observed, October 26, 2026:** the bench lease refused a second command and a hand-run read while a capture held it; after the holder was killed, `status` reported the stale lease until `--break-lease`. While a capture holds the relay port, `status` shows the lease but not the relays.
- **Observed, October 27, 2026:** the sustained run's first hardware attempts. Cycles 1–5 passed in one run. Halts in other runs came from two checks that were wrong (since fixed) and an intermittent bank-0 download fault, which stopped at a 50 MHz flash clock ([flash layout](flash-layout.md#flash-clock)). A full 16-cycle run has not yet passed.
- **Modeled, October 28, 2026:** the bench server over a socket in a temporary checkout, with a fake relay board and a stand-in flash client: a job streams its output and ends with its code; a second job is refused while one runs; a cancel sends the flash client SIGINT, and the loader's cleanup cuts power and isolates the UART; a client detaches and another follows the job from its first line; quitting cancels the job, waits for its cleanup and frees the lease; with no server, every hardware command refuses. Also: a cancel mid-write (`update execute`) sends only SIGINT and the job ends after the writer exits, with the UART shown as writing; a cancelled build takes its grandchildren with it; the server refuses what the command line refuses (`itest --list`, a missing name, out-of-range values); a 100 KiB burst of output never blocks; a panicking job leaves the server serving. No hardware ran.
- **Modeled:** the bench with a fake relay board and flash client, the page's handlers in process, and the real binary against synthetic status files. The web page with unit tests and Playwright against the real binary serving a synthetic `private/` tree, on desktop and phone sizes in both themes. The suite's scenarios, including `sustained`, against a scripted feeder that emits the firmware's record formats.
