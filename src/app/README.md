# The Android app

This is Wellfed's phone app. It installs Wellfed on a feeder that runs stock, sets up a feeder over Bluetooth, finds it on the LAN, shows its status and feeds. It is for owners; contributors start with the layout below.

So far it does:

- **Install:** from stock 1.1.15, through a DNS record on your router, from the same driver as `wellfed install`. It backs up stock, moves your meals across and makes Wellfed permanent ([consumer installation](../../docs/firmware/consumer-installation.md)). The route through the feeder's own Wi-Fi is drawn and comes later: it needs stock's Reset hold, which no one has yet seen on hardware.
- **Setup:** BLE setup of a new or reset feeder. You hold Lock, then Mic, give the Wi-Fi and the names, pick the feeder from those nearby in setup mode, then key in the 6-press code.
- **Recovery:** taking a feeder already on your network as its owner again. You hold Lock, then Dispense, and key in the same kind of code.
- **Home:** a card per feeder (compact when there are several), with whether it can feed now, the food, power, panel lock, Wi-Fi signal and anything that needs attention.
- **Feed now:** a feed of 1 to 48 portions, followed to its result.

Invites, schedules, history, settings and servers come later ([roadmap](../../docs/roadmap.md)).

## How it is built

| Part | Where | Does |
| --- | --- | --- |
| Screens, state, platform IO | `lib/` (Dart) | BLE through `flutter_reactive_ble`, NSD through `nsd`, the client secret in `flutter_secure_storage`, the install's notification through `flutter_foreground_task`, saving the backup through `share_plus` |
| `wellfed-app-bridge` | `rust/` (Rust) | The client key, the setup and recovery sessions (`wellfed_client::setup`), device API sessions (`wellfed_connect`), the install (`wellfed_install`), the app's file |
| Generated | `lib/src/rust/`, `rust/src/frb_generated.rs` | The Dart API and types, from `flutter_rust_bridge_codegen generate` |
| Theme | `lib/theme/tokens.g.dart` | Colors and sizes from [`assets/brand/tokens.json`](../../assets/brand/tokens.json) |

- **The Rust half checks what the feeder checks.** Dart moves bytes and shows what the session says. A feed goes out once; a feed whose answer never came is shown as "may have run" and never sent again.
- **Images** are links to the committed renders in `assets/renders/`, and the launcher icon links to `assets/brand/android/`.
- **The client secret never leaves the phone.** Android backup is off, so a new phone pairs again.
- **A session gives a feeder this phone's clock** right after pairing, and whenever the feeder's clock is not set, as `wellfed time` does.
- **Every socket follows the Wi-Fi network.** Android keeps mobile data as the default network while the Wi-Fi has no internet, so `MainActivity` binds the app to Wi-Fi. A VPN that apps cannot bypass (Tailscale, for one) refuses that: exclude Wellfed from it.

## The install

`wellfed_install::run` drives the whole run, the same as the CLI's. Its future is not `Send`, so `rust/src/install.rs` runs it on a thread of its own, with a current-thread tokio runtime. Stock's broker and image server are tasks on that runtime, in this process, on unprivileged ports. The `bench` feature stays off.

- **Dart moves things and draws them.** What the run needs from outside (a person's answer, a radio write, an NSD lookup) comes to Dart as an `Ask` on one stream, and Dart answers by id. Progress comes as generated `InstallEvent`s. `state/install.dart` follows them, and `screens/install_screen.dart` draws a screen per stage.
- **Tests fake the edge, not the flow.** `platform/install_ports.dart` and `platform/pairing_ports.dart` are everything these screens reach outside Dart, so `test/` runs every state with no Rust and no phone. The bridge's own tests run the thread and the adapters against the simulated feeder.
- **Errors on a stream are values.** flutter_rust_bridge decodes a stream's errors as `AnyhowException` whatever Rust encoded, so nothing sends a typed error with `add_error`.
- **An uncertain request is never sent again.** The run ends, and a listener takes over: stock's server with no trial enabled, for up to 20 minutes. Stock reporting its version means it did not install, and the ledger may then close the request. The replacement answering on the network, or advertising Bluetooth setup, means it did, and a new run picks up from bank 0.
- **A wrong Wi-Fi is not a dead end.** A feeder that took the Wi-Fi over Bluetooth and never joined it is waited for, then the owner can enter the Wi-Fi again. A run from bank 0 then goes through Bluetooth setup again, with the feeder put back in setup mode by its buttons, and asks stock nothing. Setup on any feeder but the one the install paired with stops the run, so the backup and the promotion never move to another. A consumer image from stock with no Wi-Fi to give is refused in Rust before the run starts.
- **A foreground service keeps the process alive** while a run or a listener needs the broker. The notification says so. Swiping the app away ends the run, which is why the screen asks you to keep it open.
- **Leaving does not strand a feeder.** A run records its feeder as soon as it pairs, and each feeder's request stays in its own ledger. The first screen reads both: a paired feeder can be picked up from bank 0, and a request with no settled outcome offers the listener and a look over Bluetooth. Neither ever sends the request again.

### Which builds can install

Stock's install goes only from a build whose sources can be reviewed, as with every other caller. A debug APK built from a clean commit can install. One built from a dirty tree builds and runs, and its first install screen says it cannot send an install. Commit first.

### The image it installs

The APK carries one image, the consumer build signed with the key the image trusts, so nothing is downloaded at install time. The phone never holds a key a feeder trusts: the update manifest is signed elsewhere, and the run promotes it as signed.

The image is a build-time input. `make app-installer-input` copies a review and its signed manifest into `assets/installer/`, which Git ignores, and prints the pin to build with:

```sh
make app-installer-input REVIEW=private/<review> MANIFEST=private/<version>.manifest
WELLFED_INSTALLER_PIN=<what it printed, with the version> flutter build apk --debug
```

The pin is compiled in: the digest of the review (which binds the image and its build manifest), the digest of the manifest, and the manifest's version. At startup the bridge checks the unpacked files against it, and against the review's own digests, as the image server does. A mismatch, or an image with no pin, is refused on the first screen.

A release build's pin goes in the bridge as `RELEASE_PIN`, which the environment cannot override. It is empty until a release image exists, since only the offline release key can sign one. Until then a build with no input says it has no firmware to install.

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
