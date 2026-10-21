/* SDK/FreeRTOS entry only: no shell, networking, FTL, keyscan, touch or audio.
 * The Rust example runs a synthetic diagnostic without opening feeder I/O.
 */
#include "ameba_soc.h"
#include "main.h"

int main(void)
{
    InterruptRegister(IPC_INTHandler, IPC_IRQ, (u32)IPCM0_DEV, 5);
    InterruptEn(IPC_IRQ, 5);
    ipc_table_init();
    pmu_acquire_wakelock(PMU_OS);
    pre_example_entry();
    example_entry();
    vTaskStartScheduler();
    for (;;) {}
}
