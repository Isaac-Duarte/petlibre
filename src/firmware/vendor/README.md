# Vendor ports

This folder holds the AmebaD C ports, pins and image build that the Rust firmware sits on. The application is Rust, with minimal C entry hooks and a pinned Realtek AmebaD SDK. Start with [architecture and status](../../../docs/firmware/README.md), [firmware builds](../../../docs/development/firmware-builds.md) and the [boot and flash layout](../../../docs/devices/plaf103/flash-layout.md).

```sh
make -C src/firmware/vendor test
make -C src/firmware/vendor help
```

Host tests need no SDK and no private inputs. `bootstrap` explicitly downloads the pinned SDK and compiler. SDK build outputs stay ignored. Builds never flash hardware.

The images do touch hardware when they run. The startup diagnostic requests physical STOP/off GPIO and runs a bounded LED sequence ([startup behavior](../../../docs/firmware/startup-diagnostics.md)).

The [UART recovery reference](../../../docs/devices/plaf103/recovery.md) is restricted to one exact tested profile. Its writer does not automatically accept a new build.
