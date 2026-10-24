# FW21 wall clock and missed-time policy

Status: implemented and host-tested (`petlibre-core/src/clock/wall.rs`, `petlibre-hal/src/clock.rs`, the server's `replacement/mod.rs`). Hardware evidence is recorded on the [board](../../FIRMWARE_BOARD.md).

## Source of time

The feeder has no route to the internet, and the bench host's firewall allows only DHCP, DNS and the broker from the hotspot, so NTP is not available. **Decision:** the local server sets the clock over the FW08 channel with a `time` request:

```json
{"id":"<uuid>","op":"time","unix_ms":1792674000000,"offset_s":-21600,"next_change_s":1793520000,"next_offset_s":-25200}
```

- **Time zone:** the server's configured IANA zone (`--timezone` at `init`). Each request carries the current UTC offset and the next offset change, found by searching the zone 400 days ahead. The firmware therefore needs no zone database and still crosses one DST change correctly between syncs.
- **When:** only to firmware whose health has a `clock` object (pre-FW21 builds never get one). The server sends it when health shows the clock unsynced (at most once a minute) and every 10 minutes after a sync.
- **Reply:** `{"id":…,"op":"time","ok":true,"jump_ms":N|null}`, or `ok:false` with `error` (`time`, `offset` or `next_change`) for a refused sync. The UART logs `PLTIM1 sync ok=1 kind=first|step|jump delta_ms=N offset_s=N next_change_s=N` or `PLTIM1 sync ok=0 error=…`.

The request parser accepts integers only for `op:"time"`, and only exactly the four fields, at most 15 digits each (plus `seq` on a signed request). Setting the clock is the one thing a request may change, and only with a valid signature. `cargo xtask check-repository` lets the KM4 transport name `clock::sync` and nothing else new.

## The clock

- **Invalid at boot.** There is no battery-backed RTC in use, so every boot starts unsynced.
- **Carried by uptime** between syncs. Against the server, the uptime clock ran a steady 735 ppm slow (+442 and +439 ms per 10-minute resync, physical, October 22). Each reading carries an uncertainty of 1 s plus 2000 ppm of the time since the sync, and the clock reads invalid past 60 s (about 8 hours without a sync).
- **Refused syncs** leave the clock unchanged: a time before 2026 or after 2099, an offset outside −12 h to +14 h, or a next change that is not in the future.
- **Jumps:** a sync within 2 s of the running estimate is a step; anything else is a jump, logged with its signed size.

Health reports `"clock":{"syncs":N,"unix_ms":…,"offset_s":…,"uncertainty_ms":…}` while valid and `"clock":{"syncs":N}` otherwise. The health payload limit went from 512 to 576 bytes on both sides to fit it (now 704 with the request nonce and FW14's `power`; see [health message](local-transport.md#health-message)).

## Missed-meal rule (for FP03)

`wall_clock::occurrence(due, evaluated, now)` decides each scheduled occurrence. `evaluated` is a persisted high-water mark of time already evaluated; after each evaluation it becomes `max(evaluated, now)`.

| Situation | Result |
| --- | --- |
| No valid clock | Nothing runs |
| `due` after `now` | Later |
| `due` at or before the mark | Evaluated: never runs twice, even after a backward jump |
| `due` in (mark, now], at most 15 min late | Runs once |
| `due` in (mark, now], more than 15 min late | Missed: reported, never run |

So a forward jump, a restart or a long unsynced spell runs only occurrences that are at most 15 minutes late, and a backward jump never repeats a meal.

## Trust

The channel is plaintext MQTT on the WPA2 hotspot (FW08). A `time` request must therefore be signed with the per-device request key and name the current session's nonce ([authenticated requests](local-transport.md#authenticated-requests)), and an unsigned one is refused (physical, October 22). The time itself is still visible to anyone on the hotspot, which reveals nothing.
