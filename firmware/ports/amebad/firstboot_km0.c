/* Minimal SDK main for the UART-only first-boot profile. Vendor reset/startup
 * still runs before this function, including pad pulls and internal ADC work.
 */
#include "ameba_soc.h"
#include "main.h"

/* Stock KM0 0x0c000328 and SDK app_audio_pad_enable supply the audio-shared
 * pads even when they are used as GPIO. Keep the SDK's calibrated LDO helper;
 * this does not initialize playback or change feeder output commands.
 * Stock and the SDK run this after KM4 boot; running it first powers the pads
 * before KM4 configures them and keeps PLPWR1 from interleaving with KM4 logs.
 *
 * SOCPS_AudioLDO(ENABLE) restores a trim saved only by its disable path, so a
 * cold start leaves the LDO voltage at 0. Apply the trim stock KM4 CODEC_Init
 * (0x0e029134) and SDK CODEC_Init write: 0x68 on 1.8 V supply, else 0x2A.
 * The raw Port A input is sampled after each step (read-only) to locate which
 * one disables PA0-PA7 input readback.
 */
__attribute__((noinline)) void petlibre_shared_pad_power_init(void)
{
    u32 function_before = HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0);
    u32 pad_before = HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL);
    u32 ext_a[4];

    ext_a[0] = GPIOA_BASE->EXT_PORT[0];

    HAL_WRITE32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0,
                function_before | BIT_SYS_AMACRO_EN);
    HAL_WRITE32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL,
                pad_before | BIT_LSYS_AC_ANA_PORB);
    ext_a[1] = GPIOA_BASE->EXT_PORT[0];
    SOCPS_AudioLDO(ENABLE);
    ext_a[2] = GPIOA_BASE->EXT_PORT[0];
    u32 pad = HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL);
    pad &= ~(BIT_LSYS_MASK_AC_LDO_REG << BIT_LSYS_SHIFT_AC_LDO_REG);
    pad |= (is_power_supply18() ? 0x68U : 0x2AU) << BIT_LSYS_SHIFT_AC_LDO_REG;
    HAL_WRITE32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL, pad);
    ext_a[3] = GPIOA_BASE->EXT_PORT[0];
    DiagPrintf("PLPWR1 before_func=%08x before_pad=%08x after_func=%08x after_pad=%08x ext_a=%08x,%08x,%08x,%08x\n",
               function_before, pad_before,
               HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_LP_FUNC_EN0),
               HAL_READ32(SYSTEM_CTRL_BASE_LP, REG_AUDIO_SHARE_PAD_CTRL),
               ext_a[0], ext_a[1], ext_a[2], ext_a[3]);
}

u32 petlibre_amebad_flash_peer_step(u32 boot_count, u32 ticks);
void petlibre_amebad_flash_peer_irq(VOID *data, u32 status, u32 channel);

/* FW06 peer: the Rust step answers KM4's HELLO and bumps a heartbeat in backup
 * register 5 every tick; IPC channel 16 parks this core in RAM for a lease.
 * The boot count lives in user backup register 4, which survives a watchdog
 * platform reset but not power loss. */
static void petlibre_flash_peer_task(void *context)
{
    (void)context;
    u32 count = BKUP_Read(BKUP_REG4) + 1;
    BKUP_Write(BKUP_REG4, count);
    for (;;) {
        petlibre_amebad_flash_peer_step(count, SYSTIMER_TickGet());
        vTaskDelay(1);
    }
}

__attribute__((noinline)) void petlibre_flash_peer_start(void)
{
    IPC_INTUserHandler(16, (VOID *)petlibre_amebad_flash_peer_irq, NULL);
    IPC_INTConfig(IPCM4_DEV, 16, ENABLE);
    xTaskCreate(petlibre_flash_peer_task, "petlibre-peer", 256, NULL,
                tskIDLE_PRIORITY + 1, NULL);
}

/* FW07: on AmebaD the WLAN firmware runs on KM0 (lib_wifi_fw.a). Without
 * it the KM4 driver's firmware commands are never consumed ("fw cmd fifo
 * full", "FW Enable failed") and DHCP never reaches the access point. The
 * vendor main starts it under the same backup-register bit, which app_start
 * sets from ps_config.km0_config_wifi_enable. It only creates its tasks
 * here; they run once the scheduler starts. An FW06 park masks interrupts,
 * so none of its code runs while KM4 owns flash. */
__attribute__((noinline)) void petlibre_wifi_fw_start(void)
{
    if ((BKUP_Read(0) & BIT_WIFI_ENABLE) != 0) {
        wifi_FW_init_ram();
    }
}

int main(void)
{
    InterruptRegister((IRQ_FUN)IPC_INTHandler, IPC_IRQ_LP, (u32)IPCM4_DEV, 2);
    InterruptEn(IPC_IRQ_LP, 2);
    pmu_acquire_wakelock(PMU_OS);
    InterruptDis(UART_LOG_IRQ_LP);
    km4_flash_highspeed_init();
    petlibre_shared_pad_power_init();
    petlibre_wifi_fw_start();
    km4_boot_on();
    petlibre_flash_peer_start();
    vTaskStartScheduler();
    for (;;) {}
}
