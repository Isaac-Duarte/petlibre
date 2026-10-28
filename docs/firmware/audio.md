# Speaker

The PLAF103 has a real speaker, not a piezo buzzer. Stock uses it to play a meal call the owner records in the app. Wellfed drives the same audio path with a small set of built-in tunes, played one at a time and always bounded, so the speaker can never get in the way of a feed.

Status: **Works** for playback. Which sounds play when is product policy and not settled. A feed plays no tune.

## What stock does

Recovered from the stock KM4 image (`DL_AUDIO`, `audio_play_http_file`):

- **Audio path.** Stock decodes AAC (a downloaded meal call, or one of up to 11 named prompts) and streams PCM through SPORT by GDMA into the codec.
- **Codec setup.** `CODEC_Init(rate, 16-bit, mono flag, 8)`, where application 8 is `APP_LINE_OUT`. The I2S PLL and divider are chosen per sample rate; 8, 16, 32, 44.1 and 48 kHz are supported.
- **Amplifier.** PB23 (`0x37`) is written HIGH while it plays and LOW afterwards. This is the amplifier enable.
- **PA25.** A separate GPIO buzzer line, pulsed HIGH by a one-shot 200-tick timer. It drives an active buzzer: a HIGH alone beeps ([evidence](#evidence)). The `feeder` firmware beeps it for 200 ms on every button press, as stock does, when sound may play: `sound` on, a volume above 0 and inside the sound window. The buzzer has one pitch and loudness, so volume only mutes it at 0.

Stock also keeps clips in flash (recovered from a full-flash dump):

- `0x400000` holds a one-word count (3), and `0x4B0000` a 2 MiB FAT12 filesystem with 4 KiB clusters.
- It holds `TEST1.AAC`–`TEST11.AAC` (no `TEST8`) and `COME_T~1.AAC`, 17–78 KB each, as ADTS AAC. The first stream is 44.1 kHz stereo.
- `COME_T~1` is probably the meal call.
- These clips stay on the device and are never copied into the repository.

## How Wellfed plays a tune

Three pieces work together: a C shim that owns the codec, a tune renderer, and a speaker task.

**The C shim** (`src/firmware/vendor/ports/amebad/example_entry.c`):

- Opens the codec as stock does, at 16 kHz, 16-bit mono, line out, with SPORT and an 8-page GDMA ring of 16 ms pages.
- When no filled page is ready, the DMA plays a zero page and counts an underrun, so the stream never stops by itself.
- Close stops the DMA, frees its channel, and stops SPORT and the codec output.
- Following the SDK's line-out path, `CODEC_Init` rewrites the audio LDO trim (the same value KM0 already writes) and disables the PB28–PB31 pads. It touches no pin this firmware uses.

**Tunes** (`wellfed-core/src/tone.rs`):

- Fixed note lists rendered as sines, with a 4 ms attack and a 10 ms release so notes don't click.
- The tunes are `chime` (startup), `meal`, `coin`, `alert`, `test` (1 kHz for 1 s), `twinkle`, and `key` (one 80 ms tick, BLE setup's press feedback). Each lasts under 6 s.
- **Loudness** follows the `volume` setting on a decibel scale: 0 is silent, then 0.3 dB per step up to a peak of 16000 (about −6 dBFS) at 100. A linear scale made 40 and 80 sound almost alike.

**The speaker task** (`wellfed-hal/src/audio.rs`, task `wellfed-aud`, priority 2, normal role only):

- A one-slot mailbox holds the tune. The slot stays taken until playback ends, so a second request is `busy` rather than queued.
- For each tune it opens the codec, sets PB23 HIGH, and plays 150 ms of silence while the amplifier settles. It then streams the tune, waits at most 400 ms for the ring to drain, sets PB23 LOW and closes.
- A ring that stops draining still ends playback at the tune's length plus 1 s, so the amplifier is never left on.
- The QA firmware and the bring-up `speaker` variant play the startup chime once the settings are loaded, unless sound is muted as above. The `feeder` firmware starts its speaker quiet and boots silently.

Audio is independent of feeding. The task touches only the codec, its DMA and PB23, never the motor, the emitters or flash.

## Asking for a tune

A tune is queued only through `audio::request`. It answers:

- `Started`;
- `Muted`, when the `sound` setting is off, the volume is 0, or the local time is outside the sound window (every window is open while the clock is invalid);
- `Busy`, when another tune plays or the settings are not loaded yet.

The device API's `Play` request carries a tune. The `feeder` firmware does not serve it yet, since it steps the speaker only in setup mode.

To build the speaker firmware, run `wellfed-build build --profile firstboot --firmware bringup --variant speaker`, or `wellfed-bench ota NAME --build --firmware speaker` to install it. It runs the tune task and plays the startup chime. It does not touch motion, and OTA, recovery and confirmation work as in every firmware.

## Records

The task logs `PLAUD1` lines:

- `start created=0|1`
- `play tune=… volume=…`
- `done tune=… ms=… underruns=… drained=0|1`
- `muted tune=chime`
- `error stage=open`

## Not done yet

- **Stored clips.** Playing stock's clips needs a read-only FAT reader, the SDK's Helix AAC decoder and a 44.1 kHz codec setup.
- **A meal call** before a scheduled meal, when its plan enables one ([device API](device-api.md)).

## Evidence

- **The buzzer.** Observed (October 28, owner listening, `feeder` `buzzer` variant v334): with the amplifier and codec off, a 200 ms HIGH on PA25 gave a clear beep, and 512 Hz and 2,048 Hz square waves on it sounded rough, as an active buzzer's own oscillator chopped by its supply would. The pad read back HIGH while driven HIGH and LOW while driven LOW. The normal build went back on afterwards.
- **Playback.** Observed (October 22, OTA v20 and v21, owner listening). The startup chime played on the first boot (`done tune=chime ms=624 underruns=0 drained=1`) and was heard clearly, as were `coin` and `twinkle`. A `coin` sent during `twinkle` got `busy`. With the decibel scale, volumes 40, 80 and 100 were clearly stepped and clean. With `sound` off, a play request was refused with `muted`. Settings were then restored (sound on, volume 40, generation 2985). These requests went over the since-deleted MQTT transport, which reached the same `audio::request`.
- **Amplifier settle.** Observed (October 24): a 60 ms settle swallowed a 50 ms tick whole, so the settle is 150 ms.
- **Linear volume.** Observed: on a linear scale, 40 and 80 sounded almost alike.
- **During feeds.** Observed (October 26): a tune played during feeds under load with `underruns=0`, and feeds still ended Done. Feeds play no tune since the owner's decision that day.
- **Host.** Modeled: pitch, the volume scale, silence at volume 0, quiet note edges and tune bounds. The speaker task against a fake codec ring: the chime waits for the settings, mute is honored, the amplifier is on only while the codec is open, a failed open frees the slot, and a stalled ring still closes. `play` parsing.
