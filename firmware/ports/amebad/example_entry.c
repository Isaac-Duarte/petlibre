/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 *
 * FW07: a minimal Wi-Fi station bring-up the Rust probe starts and polls.
 * One task runs LwIP_Init and wifi_on(STA), then a bounded number of scans,
 * keeping the 32 strongest networks of each. Networks are reported only as a
 * CRC-32 of their SSID, so neighbours' names
 * never reach a log; no credentials are compiled in. Nothing here registers
 * fast connect or calls rtw_efuse_boot_write.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"

void wellfed_app_start(void);

#define WELLFED_WIFI_APS 32
#define WELLFED_WIFI_SCANS 3
#define WELLFED_WIFI_SCAN_WAIT_MS 10000
#define WELLFED_WIFI_SCAN_GAP_MS 5000

/* Shared with Rust (`sdk::WifiState`); keep the layout in step. */
struct wellfed_wifi_ap {
    unsigned int ssid_crc;
    unsigned int security;
    int rssi;
    unsigned int channel;
};

struct wellfed_wifi_state {
    unsigned int started;
    unsigned int on_done;
    int on_result;
    unsigned int on_ms;
    unsigned int scans_done;
    int scan_result;
    unsigned int scan_ms;
    unsigned int count;
    unsigned int dropped;
    struct wellfed_wifi_ap aps[WELLFED_WIFI_APS];
};

static struct wellfed_wifi_state wellfed_state;
static struct wellfed_wifi_state wellfed_scan;
static volatile unsigned int wellfed_scan_complete;

static unsigned int wellfed_crc32(const unsigned char *data, unsigned int length)
{
    unsigned int crc = 0xffffffffu;
    for (unsigned int i = 0; i < length; i++) {
        crc ^= data[i];
        for (int bit = 0; bit < 8; bit++) {
            crc = (crc >> 1) ^ (0xedb88320u & (0u - (crc & 1u)));
        }
    }
    return ~crc;
}

static rtw_result_t wellfed_scan_result(rtw_scan_handler_result_t *result)
{
    if (result->scan_complete == RTW_TRUE) {
        wellfed_scan_complete = 1;
        return RTW_SUCCESS;
    }
    rtw_scan_result_t *ap = &result->ap_details;
    unsigned int length = ap->SSID.len > 32 ? 32 : ap->SSID.len;
    struct wellfed_wifi_ap found = {
        wellfed_crc32(ap->SSID.val, length), (unsigned int)ap->security, ap->signal_strength, ap->channel};
    taskENTER_CRITICAL();
    if (wellfed_scan.count < WELLFED_WIFI_APS) {
        wellfed_scan.aps[wellfed_scan.count++] = found;
    } else {
        /* Full: keep the strongest, so a nearby network is not lost among many. */
        unsigned int weakest = 0;
        for (unsigned int i = 1; i < WELLFED_WIFI_APS; i++) {
            if (wellfed_scan.aps[i].rssi < wellfed_scan.aps[weakest].rssi) {
                weakest = i;
            }
        }
        if (found.rssi > wellfed_scan.aps[weakest].rssi) {
            wellfed_scan.aps[weakest] = found;
        }
        wellfed_scan.dropped++;
    }
    taskEXIT_CRITICAL();
    return RTW_SUCCESS;
}

static void wellfed_wifi_task(void *unused)
{
    (void)unused;
    TickType_t begin = xTaskGetTickCount();
    LwIP_Init();
    int on = wifi_on(RTW_MODE_STA);
    taskENTER_CRITICAL();
    wellfed_state.on_result = on;
    wellfed_state.on_ms = (unsigned int)((xTaskGetTickCount() - begin) * portTICK_PERIOD_MS);
    wellfed_state.on_done = 1;
    taskEXIT_CRITICAL();
    for (unsigned int n = 0; on == RTW_SUCCESS && n < WELLFED_WIFI_SCANS; n++) {
        taskENTER_CRITICAL();
        wellfed_scan.count = 0;
        wellfed_scan.dropped = 0;
        taskEXIT_CRITICAL();
        wellfed_scan_complete = 0;
        begin = xTaskGetTickCount();
        int result = wifi_scan_networks(wellfed_scan_result, NULL);
        while (result == RTW_SUCCESS && !wellfed_scan_complete &&
               (xTaskGetTickCount() - begin) * portTICK_PERIOD_MS < WELLFED_WIFI_SCAN_WAIT_MS) {
            vTaskDelay(50 / portTICK_PERIOD_MS);
        }
        if (result == RTW_SUCCESS && !wellfed_scan_complete) {
            result = RTW_TIMEOUT;
        }
        taskENTER_CRITICAL();
        for (unsigned int i = 0; i < wellfed_scan.count; i++) {
            wellfed_state.aps[i] = wellfed_scan.aps[i];
        }
        wellfed_state.count = wellfed_scan.count;
        wellfed_state.dropped = wellfed_scan.dropped;
        wellfed_state.scan_result = result;
        wellfed_state.scan_ms = (unsigned int)((xTaskGetTickCount() - begin) * portTICK_PERIOD_MS);
        wellfed_state.scans_done = n + 1;
        taskEXIT_CRITICAL();
        vTaskDelay(WELLFED_WIFI_SCAN_GAP_MS / portTICK_PERIOD_MS);
    }
    vTaskDelete(NULL);
}

/* Once per boot: 1 when the task was created. */
int wellfed_wifi_start(void)
{
    if (wellfed_state.started) {
        return 0;
    }
    wellfed_state.started = 1;
    if (xTaskCreate(wellfed_wifi_task, "wellfed_wifi", 1280, NULL, tskIDLE_PRIORITY + 3, NULL) != pdPASS) {
        wellfed_state.started = 0;
        return 0;
    }
    return 1;
}

void wellfed_wifi_snapshot(struct wellfed_wifi_state *out)
{
    taskENTER_CRITICAL();
    *out = wellfed_state;
    taskEXIT_CRITICAL();
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    wellfed_app_start();
}
