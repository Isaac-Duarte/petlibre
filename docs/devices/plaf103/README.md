# PETLIBRO Granary PLAF103

The 5 L Granary feeder, and Wellfed's research device. Labels follow the [evidence levels](../../evidence.md).

## Identification

The inspected research feeder is a PETLIBRO Granary **PLAF103**, 5 L, with FCC ID **2A3DEPLAF103S**. Board photographs identify **RTL8721DM** and **GD25Q64ESIG**. UART reports flash ID `C8 40 17`, consistent with the identified GD25Q64E's nominal 64 Mbit (**8 MiB**) capacity. Two separately acquired matching 8 MiB reads establish external-flash read coverage on that unit.

The [FCC internal-photo exhibit](https://fccid.io/2A3DEPLAF103S/Internal-Photos/Internal-Photos-6465982.pdf) is a comparison reference and shows a board marked `PLAF103 MB V1.0`; the actual research unit's PCB revision has not been established. The [GD25Q64E datasheet](https://download.gigadevice.com/Datasheet/DS-00484-GD25Q64E-Rev1.6.pdf) is the external-flash reference. These links identify research sources; compare markings on your own unit.

Do not assume PLAF108 compatibility. No schematic, complete connector pinout, measured motor driver behavior, or complete physical GPIO mapping is established.

## Pages

| Topic | Page |
| --- | --- |
| Signal map, motor truth table, connectors | [Board](board.md), [chip pin table](../../chips/rtl8721dm/pins.md) |
| What works, by block | [Feature support](feature-support.md) |
| Motor driver and electrical checks | [Motor driver](motor-driver.md) |
| Console and loader entry | [UART](uart.md) |
| Flash layout and boot | [Flash layout](flash-layout.md) |
| Backup and recovery | [Recovery](recovery.md) |
| Relay bench automation | [Bench](bench.md) |
| Stock 1.1.15 behavior and protocol | [Stock firmware](stock/README.md), [stock OTA requirements](stock/ota-requirements.md) |

Machine-readable device data lives in [`src/firmware/devices/plaf103/`](../../../src/firmware/devices/plaf103/).
