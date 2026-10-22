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
    /* Bits 16-31 belong to the FW11 trial (petlibre_trial_step). */
    u32 reg = BKUP_Read(BKUP_REG4);
    u32 count = (reg + 1) & 0xFFFFU;
    BKUP_Write(BKUP_REG4, (reg & 0xFFFF0000U) | count);
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

u32 petlibre_amebad_trial_decide(const u8 *bank1, const u8 *bank0, const u8 *record, u32 *reg);

/* FW11 boot trial (amebad-common/src/trial.rs decides): before KM4 runs, count an
 * unconfirmed bank-1 image's boots in backup register 4 and, at the limit
 * with a valid bank 0, clear bank 1's first marker byte and reset so the
 * loader falls back to bank 0. KM4 is off, so the SDK flash lock only masks
 * this core's interrupts (petlibre_trial_program).
 * Physical reads go through the direct SPIC window. */
#define PETLIBRE_BANK0_OFFSET 0x006000U
#define PETLIBRE_BANK1_OFFSET 0x186000U
#define PETLIBRE_CONFIRM_OFFSET 0x344000U
/* FLASH_TxData256BXIP without FLASH_Write_Lock: that lock's KM0 branch
 * asks KM4 to sleep whenever km4_status_on(), and after a watchdog reset the
 * HS clock bit it reads stays set while KM4 is not yet booted, so the lock
 * waits forever (observed). KM4 is not running here; mask this core's
 * interrupts and SysTick as the lock would, program, flush, restore. */
IMAGE2_RAM_TEXT_SECTION __attribute__((noinline, noclone))
static void petlibre_trial_program(u32 address, u32 length, u8 *data)
{
    u32 primask = __get_PRIMASK();
    __disable_irq();
    u32 systick = SysTick->CTRL;
    SysTick->CTRL = 0;
    FLASH_TxData256B(address, length, data);
    Cache_Flush();
    SysTick->CTRL = systick;
    __set_PRIMASK(primask);
}

__attribute__((noinline)) void petlibre_trial_step(void)
{
    const u8 *bank1 = (const u8 *)(SPI_FLASH_BASE + PETLIBRE_BANK1_OFFSET);
    const u8 *bank0 = (const u8 *)(SPI_FLASH_BASE + PETLIBRE_BANK0_OFFSET);
    const u8 *record = (const u8 *)(SPI_FLASH_BASE + PETLIBRE_CONFIRM_OFFSET);
    u32 reg = BKUP_Read(BKUP_REG4);
    u32 code = petlibre_amebad_trial_decide(bank1, bank0, record, &reg);
    BKUP_Write(BKUP_REG4, reg);
    switch (code & 0xFFU) {
    case 0:
        DiagPrintf("PLTRL1 bank=0\n");
        return;
    case 1:
        DiagPrintf("PLTRL1 confirmed=1\n");
        return;
    case 2:
        DiagPrintf("PLTRL1 confirmed=0 boot=%d limit=3\n", code >> 8);
        return;
    case 4:
        DiagPrintf("PLTRL1 error fallback=invalid boot=%d\n", code >> 8);
        return;
    default:
        break;
    }
    /* Clear only byte 0 ('8' -> 0x00); bytes 1-3 are rewritten unchanged,
     * since programming only clears bits. */
    u8 word[4] = {0x00, bank1[1], bank1[2], bank1[3]};
    petlibre_trial_program(PETLIBRE_BANK1_OFFSET, sizeof(word), word);
    DiagPrintf("PLTRL1 give_way=1 marker=%02x reset=1\n", bank1[0]);
    /* The vendor OTA reset: a short watchdog expiry resets the platform. */
    WDG_InitTypeDef wdg;
    u32 count_process, div_process;
    WDG_Scalar(50, &count_process, &div_process);
    wdg.CountProcess = count_process;
    wdg.DivFacProcess = div_process;
    wdg.RstAllPERI = 1;
    WDG_Init(&wdg);
    WDG_Cmd(ENABLE);
    for (;;) {}
}

int main(void)
{
    InterruptRegister((IRQ_FUN)IPC_INTHandler, IPC_IRQ_LP, (u32)IPCM4_DEV, 2);
    InterruptEn(IPC_IRQ_LP, 2);
    pmu_acquire_wakelock(PMU_OS);
    InterruptDis(UART_LOG_IRQ_LP);
    km4_flash_highspeed_init();
    petlibre_shared_pad_power_init();
    petlibre_trial_step();
    petlibre_wifi_fw_start();
    km4_boot_on();
    petlibre_flash_peer_start();
    vTaskStartScheduler();
    for (;;) {}
}
