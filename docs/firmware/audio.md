# FW24 speaker and built-in tunes

The PLAF103 has a real speaker, not a piezo buzzer: the stock app lets owners record a meal call, and stock 1.1.15 plays it through the AmebaD audio codec. The replacement firmware now drives that path with built-in tunes.

## What stock does (static analysis)

From the stock KM4 image (`DL_AUDIO`, `audio_play_http_file`):
- **Audio path:** decodes AAC (a downloaded meal call, or one of up to 11 named prompts) and streams PCM through SPORT by GDMA into the codec.
- **Codec setup:**
  - `CODEC_Init(rate, 16-bit, mono flag, 8)`; application 8 is `APP_LINE_OUT`.
  - The I2S PLL and divider are chosen per sample rate: 8, 16, 32, 44.1 and 48 kHz are supported.
- **Amplifier:** PB23 (`0x37`) is written HIGH while it plays and LOW afterwards. This is the amplifier enable.
- **PA25:** a separate GPIO "buzzer" line, pulsed HIGH by a one-shot 200-tick timer. It is not exercised here.
- **Stored clips (static, from a full-flash dump):**
  - `0x400000` holds a one-word count (3). `0x4B0000` holds a 2 MiB FAT12 filesystem (4 KiB clusters).
  - In it are `TEST1.AAC`–`TEST11.AAC` (no `TEST8`) and `COME_T~1.AAC`, 17–78 KB each, as ADTS AAC (the first stream is 44.1 kHz stereo).
  - `COME_T~1` is probably the meal call.
  - These clips stay on the device and are not copied into the repository.

## Replacement design

- **C shim** (`src/firmware/vendor/ports/amebad/example_entry.c`):
  - Opens the codec as stock does, at 16 kHz, 16-bit mono, line out, plus SPORT and an 8-page GDMA ring (16 ms pages).
  - When no filled page is ready, the DMA plays a zero page and counts an underrun, so the stream never stops by itself.
  - Close stops the DMA, frees its channel, and stops SPORT and the codec output.
  - Following the SDK's line-out path, `CODEC_Init` rewrites the audio LDO trim (the same value KM0 already writes) and disables the PB28–PB31 pads. It touches no pin this firmware uses.
- **Tunes** (`petlibre-core/src/tone.rs`):
  - Fixed note lists rendered as sines, with a 4 ms attack and a 10 ms release so notes don't click.
  - Tunes: `chime` (startup), `meal`, `coin`, `alert`, `test` (1 kHz for 1 s) and `twinkle`. Each lasts under 6 s.
  - **Loudness** follows the FW15 `volume` on a decibel scale: 0 is silent, then 0.3 dB per step up to a peak of 16000 (about −6 dBFS) at 100.
  - A linear scale was tried first; 40 and 80 sounded almost alike on the unit.
- **Speaker task** (`petlibre-hal/src/audio.rs`, task `petlibre-aud`, priority 2, normal role only):
  - A one-slot mailbox holds the tune, and the slot stays taken until playback ends, so a second request is `busy` rather than queued.
  - **Per tune:** open the codec, set PB23 HIGH, play 60 ms of silence while the amplifier settles, stream the tune, wait for the ring to drain (at most 400 ms), then set PB23 LOW and close.
  - **Bounded:** a ring that stops draining still ends playback at the tune's length plus 1 s, so the amplifier is never left on.
  - **Startup:** the startup chime plays once the settings are loaded, unless `sound` is off.
- **Feeding:** audio is independent of it. The task touches only the codec, its DMA and PB23, never the motor, the emitters or flash.

## Transport

A signed `{"op":"play","tune":"<name>"}` queues a tune (see [local transport](local-transport.md#speaker-fw24)). An unknown tune is malformed. Replies:
- `"ok":true,"tune":…` when the tune is queued;
- `error: muted` when the `sound` setting is off;
- `error: busy` while another tune plays or the settings are not loaded yet.

The transport reaches only `audio::request` (`cargo xtask check-repository` pins it).

## Speaker firmware (network audio)

`petlibre-build build --profile firstboot --firmware speaker` (or `petlibre-bench ota NAME --build --firmware speaker`) builds the `speaker` firmware: the tune task and a stream task (`petlibre-hal/src/stream.rs`, task `petlibre-spk`). Until the firmware split this was the reference firmware's `speaker` variant; the evidence below ran on that. Motion is untouched, and OTA, recovery and confirmation work as in every firmware.
- **Handshake:** the task listens on TCP 7700, one client at a time. It sends a `PLSPK1` hello with a fresh challenge, and the client answers with a header (sample rate 8, 16, 32 or 48 kHz) and an HMAC under the FW08 request key (`petlibre-core/src/speaker_stream.rs`). The feeder replies with one status byte: `ok`, `unauthenticated`, `bad_header`, `busy` or `muted`. An image without a request key refuses every stream.
- **Stream:**
  - After `ok`, the connection is raw 16-bit little-endian mono PCM.
  - A 64 KiB buffer (1 s at 32 kHz) fills to half before playback starts. TCP backpressure paces the sender.
  - The stream ends when the client closes, or after 3 s without data. The ring drains, then the amplifier and codec close.
- **Shared output and settings:** tunes and streams share one output slot, so either one gets `busy` while the other plays. `sound` off refuses streams, and `volume` scales them on the tunes' decibel curve.
- **Cost:** about 4 KiB of flash over the normal image, and 64 KiB of static RAM plus a 4 KiB stack.
- **Host (removed October 24, 2026 with FW51; in Git history):** `petlibre-local speak FILE [--host IP] [--rate 32000]` decoded any format `ffmpeg` reads (or `-` for stdin) to mono PCM and streamed it. It found the feeder by its MAC on the hotspot when `--host` was omitted.
- **Records:** `PLSPK1 start created=`, `listen port= ok=`, `open rate= volume=`, `refused status=`, `done bytes= ms= underruns= reason=closed|stall` and `error stage=open`.
- **Physical (October 22, OTA v22):**
  - A 2 s tone and a 15 s file streamed from mp3; the 15 s file was 960,000 bytes, played in 15.1 s.
  - That run reported `underruns=1`. It was counted after the last page, while the DMA played silence by design; source now counts only up to the last page.
- **Host tests** (`tests/stream.rs`):
  - sample-exact playback at the volume gain;
  - wrong-key, muted, busy and handshake-timeout refusals that never open the output;
  - a stalled client dropped with the amplifier off.

## Records

The task logs these `PLAUD1` lines:
- `start created=0|1`
- `play tune=… volume=…`
- `done tune=… ms=… underruns=… drained=0|1`
- `muted tune=chime`
- `error stage=open`

## Evidence

**Physical (October 22, OTA v20 and v21, owner listening):**
- **Startup chime:** played on the first boot (`done tune=chime ms=624 underruns=0 drained=1`) and was heard clearly. So were `coin` and `twinkle`.
- **One at a time:** a `coin` sent during `twinkle` got `busy`.
- **Volume:** with the decibel scale, 40, 80 and 100 were clearly stepped and clean.
- **Mute:** with `sound` off, `play` was refused with `muted`. Settings were then restored (sound on, volume 40, generation 2985).

**Host:**
- **Tones:** pitch, the volume scale, silence at volume 0, quiet note edges and tune bounds.
- **Speaker task:** against a fake codec ring, the chime waits for the settings, mute is honored, the amplifier is on only while the codec is open, a failed open frees the slot, and a stalled ring still closes.
- **Requests:** `play` parsing and signing on the device and on the server.

**Remaining:**
- **Stored clips:** playing them needs a read-only FAT reader and the SDK's Helix AAC decoder, and a 44.1 kHz codec setup;
- PA25's role;
- quiet windows (FW21/FW23);
- the feed-time call once FW17 can feed.
