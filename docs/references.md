# References

Outside sources the docs cite. These are leads and comparison material, never authority over what a unit actually shows. Anything taken from a peer project is labelled **External** ([evidence levels](evidence.md)) until it is observed on our own unit.

## Datasheets and filings

| Source | Used for |
| --- | --- |
| [GD25Q64E datasheet](https://download.gigadevice.com/Datasheet/DS-00484-GD25Q64E-Rev1.6.pdf) | PLAF103 external flash ([device](devices/plaf103/README.md)) |
| [FCC 2A3DEPLAF103S internal photos](https://fccid.io/2A3DEPLAF103S/Internal-Photos/Internal-Photos-6465982.pdf) | Board comparison; shows `PLAF103 MB V1.0` |
| TC118AH datasheet (S&CIC2102 v1.1) | PLAF103 motor driver ([motor driver](devices/plaf103/motor-driver.md)); kept in ignored `private/references/` |
| Realtek AmebaD SDK | Pinned under [`src/firmware/vendor/`](../src/firmware/vendor/README.md) |

## Peer projects

Other people's work on PETLIBRO devices. Different models, different units, and different evidence standards: check a claim on your own unit before relying on it.

| Project | Model | What it offers |
| --- | --- | --- |
| [earlynerd/Petlibro-One-Custom-Firmware](https://github.com/earlynerd/Petlibro-One-Custom-Firmware) | AF301 (mainboard PLNF301), RTL8721DM | A bench-mapped GPIO table for the QFN68 package, RFID Modbus protocol, custom AmebaD firmware |
| [taylorfinnell/petlibro-esphome](https://github.com/taylorfinnell/petlibro-esphome) | PLAF108, PLWF105 (ESP-based) | ESPHome replacement firmware and backup steps |
| [Dr-Crow/plaf203](https://github.com/Dr-Crow/plaf203) | PLAF203 (Linux, camera) | Root access, boot flow and filesystem notes |
| [jjjonesjr33/petlibro](https://github.com/jjjonesjr33/petlibro) | Cloud API, many models | Home Assistant integration over the vendor cloud |

## Documentation models

Projects whose doc layout this tree follows: [Asahi Linux docs](https://github.com/AsahiLinux/docs) (per-block hardware pages, feature-support tables), [LibreTiny](https://github.com/libretiny-eu/libretiny) (per-chip and per-board pages, dumping and partition layouts), and [Valetudo](https://github.com/Hypfer/Valetudo) (supported-device lists, rooting and recovery pages).
