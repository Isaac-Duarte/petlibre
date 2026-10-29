# The Android app

This is Wellfed's phone app. It sets up a feeder over Bluetooth, finds it on the LAN, shows its status and feeds. It is for owners; contributors start with the layout below.

So far it does the first milestone:

- **Setup:** BLE setup of a new or reset feeder. You hold Lock, then Mic, give the Wi-Fi and the names, pick the feeder from those nearby in setup mode, then key in the 6-press code.
- **Recovery:** taking a feeder already on your network as its owner again. You hold Lock, then Dispense, and key in the same kind of code.
- **Home:** a card per feeder (compact when there are several), with whether it can feed now, the food, power, panel lock, Wi-Fi signal and anything that needs attention.
- **Feed now:** a feed of 1 to 48 portions, followed to its result.

Installing on a stock feeder, invites, schedules, history, settings and servers come later ([roadmap](../../docs/roadmap.md)).

## How it is built

| Part | Where | Does |
| --- | --- | --- |
| Screens, state, platform IO | `lib/` (Dart) | BLE through `flutter_reactive_ble`, NSD through `nsd`, the client secret in `flutter_secure_storage` |
| `wellfed-app-bridge` | `rust/` (Rust) | The client key, the setup and recovery sessions (`wellfed_client::setup`), device API sessions (`wellfed_connect`), the app's file |
| Generated | `lib/src/rust/`, `rust/src/frb_generated.rs` | The Dart API and types, from `flutter_rust_bridge_codegen generate` |
| Theme | `lib/theme/tokens.g.dart` | Colors and sizes from [`assets/brand/tokens.json`](../../assets/brand/tokens.json) |

- **The Rust half checks what the feeder checks.** Dart moves bytes and shows what the session says. A feed goes out once; a feed whose answer never came is shown as "may have run" and never sent again.
- **Images** are links to the committed renders in `assets/renders/`, and the launcher icon links to `assets/brand/android/`.
- **The client secret never leaves the phone.** Android backup is off, so a new phone pairs again.
- **Every socket follows the Wi-Fi network.** Android keeps mobile data as the default network while the Wi-Fi has no internet, so `MainActivity` binds the app to Wi-Fi. A VPN that apps cannot bypass (Tailscale, for one) refuses that: exclude Wellfed from it.

## Toolchain

The versions this was built with:

- Flutter 3.47.8 (stable), for example in `~/.local/flutter`.
- The Android SDK command-line tools, with platform 36, build-tools 36.0.0 and NDK 27.0.12077973. Run `flutter config --android-sdk <path>`.
- The Rust targets: `rustup target add --toolchain 1.99.1 aarch64-linux-android x86_64-linux-android`.
- `cargo install flutter_rust_bridge_codegen --version 2.13.0 --locked`.

Then `make check-app` from the repository root regenerates the bindings and checks they are unchanged, formats, analyzes, runs the widget tests and builds a debug APK. The bridge's own tests run in `make check`, against the simulated feeder.

## Change the bridge

1. Edit `rust/src/api/`.
2. Run `flutter_rust_bridge_codegen generate` here.
3. Commit the regenerated `lib/src/rust/` and `rust/src/frb_generated.rs` with the Rust change.

After a change to `tokens.json`, run `UPDATE_BINDINGS=1 cargo test -p wellfed-app-bridge --test tokens` and commit `lib/theme/tokens.g.dart`.

## Run it against the simulator

The simulator has no Bluetooth, so use recovery:

1. Start it on the LAN, with your computer's LAN address:

   ```sh
   cargo run -p wellfed-feeder-sim -- --state /tmp/sim --listen 0.0.0.0:4747 --mdns 192.168.1.10
   ```

2. Install the app: `flutter run` with the phone on the same network.
3. In the app, choose **Recover a feeder**, name the phone, and pick the feeder.
4. At the simulator's prompt, open the window with `hold lock`, then `hold lock+dispense`.
5. When the app shows the code, type it at the prompt as positions: `press 1 3 2 …` (1 Lock, 2 Dispense, 3 Mic, 4 Reset).

Home then shows the simulated feeder, and **Feed now** runs a feed in the simulator. Stop the simulator with `quit` when you are done.
