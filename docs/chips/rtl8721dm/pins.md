# RTL8721DM pins

One row per GPIO bonded out on the QFN68 package, and one column per device. The same pin often does different jobs on different boards. `PA25` is the buzzer on the PLAF103 and an RFID UART line on the AF301, so a role in one column never says anything about another. Labels follow the [evidence levels](../../evidence.md).

## Sources

- **Bonded pins (External):** the 34 GPIOs come from [earlynerd's AF301 pin map](https://github.com/earlynerd/Petlibro-One-Custom-Firmware/blob/main/Docs/FEEDER_PINMAP.md), which checked them against the datasheet's package figure and a live chip. Every pin the PLAF103 map uses is in that list. `PA7`/`PA8` are the LOGUART. That project also reports that `PB13`, `PB14`, `PB16` and `PB17` fault in the SDK's `gpio_init` and are readable only through the ROM, and that re-muxing the internal flash pads hangs execute-in-place (External).
- **Peripheral functions (Recovered):** the UART, ADC and PWM columns are the SDK HAL pin tables at the pinned revision (`component/common/mbed/targets/hal/rtl8721d/`: `serial_api.c`, `analogin_api.c`, `pwmout_api.c`). The pinmux supports more functions (I²C, SPI, I²S, keyscan, captouch) than these tables list; check `rtl8721d_pinmux.h` and the datasheet before relying on a pin's absence here.
- **SDK number** is `(port << 5) | pin` ([chip](README.md#pins)).

## Table

| Pin | SDK no. | SDK HAL functions | [PLAF103](../../devices/plaf103/board.md) | AF301 (External) |
| --- | --- | --- | --- | --- |
| PA0 | 0 | — | Connection/OTA indicator (Observed) | Meal button |
| PA2 | 2 | — | Low-food indicator (Observed) | Feed button |
| PA4 | 4 | — | Lock indicator (Observed) | Lid button |
| PA7 | 7 | UART2 TX (LOGUART) | Console TX; ROM download strap (Recovered; header continuity not established) | Console |
| PA8 | 8 | UART2 RX (LOGUART) | Console RX (Recovered) | Console |
| PA12 | 12 | UART3 TX, PWM0 | — | Driven, role unknown |
| PA13 | 13 | UART3 RX, PWM1 | Motor output A (Recovered) | — |
| PA14 | 14 | — | Motor output B (Recovered) | — |
| PA15 | 15 | — | Food emitter enable (Observed) | — |
| PA16 | 16 | — | Outlet input, LOW when blocked (Observed) | Driven, role unknown |
| PA17 | 17 | — | Food-level input, LOW when blocked (Observed) | Driven, role unknown |
| PA18 | 18 | UART0 TX | Portion feedback input (Recovered) | — |
| PA19 | 19 | UART0 RX | External-power presence, HIGH on adapter (Observed) | — |
| PA25 | 25 | UART3 RX, PWM4 | Buzzer output (Recovered) | RFID UART RX |
| PA26 | 26 | UART3 TX, PWM5 | — | RFID UART TX |
| PA27 | 27 | — | — | Boot strap (`NORMAL_MODE_SEL`) |
| PA28 | 28 | PWM6 | — | Driven, role unknown |
| PA30 | 30 | PWM7 | — | — |
| PB1 | 33 | UART3 TX, ADC4 | Current estimate, ADC (Recovered) | Analog, role unknown |
| PB2 | 34 | UART3 RX, ADC5 | Battery/power measurement, ADC (Observed) | Analog, role unknown |
| PB3 | 35 | ADC6; `SWD_CLK` | Outlet emitter enable, needs SWD released (Observed) | Analog, role unknown |
| PB4 | 36 | ADC0, PWM8 | Reset button, active LOW (Observed) | Analog, role unknown |
| PB5 | 37 | ADC1, PWM9 | Mic button, active LOW (Observed) | Analog, role unknown |
| PB6 | 38 | ADC2 | Dispense button, active LOW (Observed) | Analog, role unknown |
| PB7 | 39 | ADC3, PWM17 | Lock button, active LOW (Observed) | Analog, role unknown |
| PB13 | 45 | — | — | ROM-readable only |
| PB14 | 46 | — | — | ROM-readable only |
| PB16 | 48 | — | — | ROM-readable only |
| PB17 | 49 | — | — | ROM-readable only |
| PB22 | 54 | PWM14 | Warning/fault indicator (Observed) | — |
| PB23 | 55 | PWM15 | Audio amplifier enable (Observed) | — |
| PB26 | 58 | — | — | Lock button |
| PB29 | 61 | — | — | — |
| PB31 | 63 | — | — | — |

A dash in a device column means that device's docs record no role. It does not mean the pin is unused. The PLAF103 details, open questions and stock instruction anchors are on its [board page](../../devices/plaf103/board.md).
