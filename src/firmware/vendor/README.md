# Replacement firmware

The application is Rust, with minimal C entry hooks and a pinned Realtek AmebaD SDK. Start with [architecture and status](../../../docs/firmware/README.md), [SDK builds](../../../docs/development/firmware-builds.md), and [boot/flash layout](../../../docs/firmware/boot-and-flash.md).

```sh
make -C src/firmware/vendor test
make -C src/firmware/vendor help
```

Host tests need no SDK or private inputs. `bootstrap` explicitly downloads the pinned SDK/compiler. SDK build outputs remain ignored. Builds never flash hardware. The current default application requests physical STOP/off GPIO and a bounded LED sequence around the synthetic diagnostic; the full feeding backend remains unopened. See [startup behavior](../../../docs/firmware/startup-diagnostics.md).

The [UART recovery reference](../../../docs/hardware/recovery.md) is restricted to one exact tested profile; a new build is not automatically accepted by its writer.
