/* Minimal SDK main for the UART-only first-boot profile. Vendor reset/startup
 * still runs before this function, including pad pulls and internal ADC work.
 */
#include "ameba_soc.h"
#include "main.h"

/* Stock KM0 0x0c000328 and SDK app_audio_pad_enable supply the audio-shared
 * pads even when they are used as GPIO. Keep the SDK's calibrated LDO helper;
 * this does not initialize playback or change feeder output commands.
 */
__attribute__((noinline)) void petlibro_shared_pad_power_init(void)
{
    u32 function_before = HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0);
    u32 pad_before = HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL);

    HAL_WRITE32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0,
                function_before | BIT_SYS_AMACRO_EN);
    HAL_WRITE32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL,
                pad_before | BIT_LSYS_AC_ANA_PORB);
    SOCPS_AudioLDO(ENABLE);
    DiagPrintf("PLPWR1 before_func=%08x before_pad=%08x after_func=%08x after_pad=%08x\n",
               function_before, pad_before,
               HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0),
               HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL));
}

int main(void)
{
    InterruptRegister((IRQ_FUN)IPC_INTHandler, IPC_IRQ_LP, (u32)IPCM4_DEV, 2);
    InterruptEn(IPC_IRQ_LP, 2);
    pmu_acquire_wakelock(PMU_OS);
    InterruptDis(UART_LOG_IRQ_LP);
    km4_flash_highspeed_init();
    km4_boot_on();
    petlibro_shared_pad_power_init();
    vTaskStartScheduler();
    for (;;) {}
}
