# Evidence levels

Every finding in these docs carries one of these labels. A label says how the fact was established and on which unit. It never says that repeating the experiment is safe or authorized.

| Label | Also written as | Meaning |
| --- | --- | --- |
| **Observed** | physical, physical observation | Seen on a real unit: part markings, UART output, a meter reading, or a recorded physical operation. Holds for that unit and that image unless stated otherwise. |
| **Recovered** | static analysis | Traced in stock firmware instructions, the vendor app or the SDK, without electrical verification of the wiring or timing. |
| **Modeled** | simulation, host-tested | Host tests, the simulator, or execution against supplied register/ROM/NOR models. Says nothing about hardware reliability. |
| **External** | peer project | Reported by another project, usually on a different model (see [references](references.md)). A lead for our own checks, never a fact about our devices until we observe it. |
| **Unknown** | — | Not established. Unknown flash ranges are not free space, and unknown pins are not unused. |

Most findings come from one research unit. A finding applies to other units of the same model only when it says so. A finding on one model never carries over to another model by default, even one with the same SoC.
