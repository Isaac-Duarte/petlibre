/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 *
 * FW07 stage C: one task joins the configured network and keeps it. It runs
 * LwIP_Init and wifi_on(STA), then repeats: scan for the network (to learn
 * its security), wifi_connect, DHCP; on success it watches the link and, when
 * it drops, rejoins with bounded backoff. The SDK's static-address DHCP
 * fallback counts as failure and is cleared. Nothing resets or writes flash;
 * the SDK's own reconnect stays unregistered. Events go to a ring the Rust
 * probe drains. The SSID and password come from a generated header built
 * from a private file; neither is ever logged here. Nothing registers fast
 * connect or calls rtw_efuse_boot_write.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"
#include <platform/platform_stdlib.h>
#include "wellfed_wifi_credentials.h"

void wellfed_app_start(void);

#define WELLFED_WIFI_EVENTS 16
#define WELLFED_SCAN_WAIT_MS 10000
#define WELLFED_WATCH_MS 500
#define WELLFED_BACKOFF_MIN_MS 1000
#define WELLFED_BACKOFF_MAX_MS 30000

/* Event kinds; shared with Rust (`sdk::WifiEvent`). */
enum {
    WELLFED_EV_ON = 1,      /* result: wifi_on; ms: duration */
    WELLFED_EV_SCAN = 2,    /* result: scan; value: found (0/1); rssi, security */
    WELLFED_EV_CONNECT = 3, /* result: wifi_connect; ms */
    WELLFED_EV_DHCP = 4,    /* result: LwIP_DHCP state; value: IPv4 (host order) when assigned; ms */
    WELLFED_EV_LOST = 5,    /* value: ms connected */
    WELLFED_EV_BACKOFF = 6, /* value: ms until the next attempt */
};

struct wellfed_wifi_event {
    unsigned int seq;
    unsigned int kind;
    unsigned int attempt;
    int result;
    unsigned int value;
    int rssi;
    unsigned int security;
    unsigned int ms;
};

static struct wellfed_wifi_event wellfed_events[WELLFED_WIFI_EVENTS];
static unsigned int wellfed_seq;
static unsigned int wellfed_started;
static volatile unsigned int wellfed_scan_done;
static volatile int wellfed_found_rssi;
static volatile unsigned int wellfed_found_security;
static volatile unsigned int wellfed_found;

static void wellfed_event(unsigned int kind, unsigned int attempt, int result, unsigned int value, unsigned int ms)
{
    taskENTER_CRITICAL();
    struct wellfed_wifi_event *e = &wellfed_events[wellfed_seq % WELLFED_WIFI_EVENTS];
    e->seq = ++wellfed_seq;
    e->kind = kind;
    e->attempt = attempt;
    e->result = result;
    e->value = value;
    e->rssi = kind == WELLFED_EV_SCAN ? wellfed_found_rssi : 0;
    e->security = kind == WELLFED_EV_SCAN ? wellfed_found_security : 0;
    e->ms = ms;
    taskEXIT_CRITICAL();
}

static unsigned int wellfed_ms(TickType_t since)
{
    return (unsigned int)((xTaskGetTickCount() - since) * portTICK_PERIOD_MS);
}

static rtw_result_t wellfed_scan_result(rtw_scan_handler_result_t *result)
{
    if (result->scan_complete == RTW_TRUE) {
        wellfed_scan_done = 1;
        return RTW_SUCCESS;
    }
    rtw_scan_result_t *ap = &result->ap_details;
    unsigned int length = sizeof(WELLFED_WIFI_SSID) - 1;
    if (ap->SSID.len == length && memcmp(ap->SSID.val, WELLFED_WIFI_SSID, length) == 0 &&
        (!wellfed_found || ap->signal_strength > wellfed_found_rssi)) {
        wellfed_found_rssi = ap->signal_strength;
        wellfed_found_security = (unsigned int)ap->security;
        wellfed_found = 1;
    }
    return RTW_SUCCESS;
}

/* One join attempt: scan, connect, DHCP. 1 when an address was assigned. */
static int wellfed_join(unsigned int attempt)
{
    wellfed_found = 0;
    wellfed_scan_done = 0;
    TickType_t begin = xTaskGetTickCount();
    int result = wifi_scan_networks(wellfed_scan_result, NULL);
    while (result == RTW_SUCCESS && !wellfed_scan_done && wellfed_ms(begin) < WELLFED_SCAN_WAIT_MS) {
        vTaskDelay(50 / portTICK_PERIOD_MS);
    }
    if (result == RTW_SUCCESS && !wellfed_scan_done) {
        result = RTW_TIMEOUT;
    }
    wellfed_event(WELLFED_EV_SCAN, attempt, result, wellfed_found, wellfed_ms(begin));
    if (result != RTW_SUCCESS || !wellfed_found) {
        return 0;
    }
    begin = xTaskGetTickCount();
    result = wifi_connect(WELLFED_WIFI_SSID, (rtw_security_t)wellfed_found_security, WELLFED_WIFI_PASSWORD,
                          sizeof(WELLFED_WIFI_SSID) - 1, sizeof(WELLFED_WIFI_PASSWORD) - 1, -1, NULL);
    wellfed_event(WELLFED_EV_CONNECT, attempt, result, 0, wellfed_ms(begin));
    if (result != RTW_SUCCESS) {
        return 0;
    }
    begin = xTaskGetTickCount();
    uint8_t state = LwIP_DHCP(0, DHCP_START);
    unsigned int ip = 0;
    if (state == DHCP_ADDRESS_ASSIGNED) {
        uint8_t *a = LwIP_GetIP(&xnetif[0]);
        ip = ((unsigned int)a[0] << 24) | ((unsigned int)a[1] << 16) | ((unsigned int)a[2] << 8) | a[3];
    }
    wellfed_event(WELLFED_EV_DHCP, attempt, state, ip, wellfed_ms(begin));
    if (state != DHCP_ADDRESS_ASSIGNED) {
        /* Never keep the SDK's static fallback address. */
        LwIP_DHCP(0, DHCP_STOP);
        LwIP_ReleaseIP(0);
        wifi_disconnect();
        return 0;
    }
    return 1;
}

static void wellfed_wifi_task(void *unused)
{
    (void)unused;
    TickType_t begin = xTaskGetTickCount();
    LwIP_Init();
    int on = wifi_on(RTW_MODE_STA);
    wellfed_event(WELLFED_EV_ON, 0, on, 0, wellfed_ms(begin));
    if (on != RTW_SUCCESS) {
        vTaskDelete(NULL);
        return;
    }
    unsigned int attempt = 0;
    unsigned int backoff = WELLFED_BACKOFF_MIN_MS;
    for (;;) {
        attempt++;
        if (wellfed_join(attempt)) {
            backoff = WELLFED_BACKOFF_MIN_MS;
            TickType_t joined = xTaskGetTickCount();
            while (wifi_is_connected_to_ap() == RTW_SUCCESS) {
                vTaskDelay(WELLFED_WATCH_MS / portTICK_PERIOD_MS);
            }
            wellfed_event(WELLFED_EV_LOST, attempt, 0, wellfed_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
        }
        wellfed_event(WELLFED_EV_BACKOFF, attempt, 0, backoff, 0);
        vTaskDelay(backoff / portTICK_PERIOD_MS);
        backoff = backoff * 2 > WELLFED_BACKOFF_MAX_MS ? WELLFED_BACKOFF_MAX_MS : backoff * 2;
    }
}

/* Once per boot: 1 when the task was created. */
int wellfed_wifi_start(void)
{
    if (wellfed_started) {
        return 0;
    }
    wellfed_started = 1;
    if (xTaskCreate(wellfed_wifi_task, "wellfed_wifi", 1280, NULL, tskIDLE_PRIORITY + 3, NULL) != pdPASS) {
        wellfed_started = 0;
        return 0;
    }
    return 1;
}

/* Copy the event with sequence number `seq` (from 1); 0 when it is not yet
 * recorded, -1 when it was overwritten. */
int wellfed_wifi_event(unsigned int seq, struct wellfed_wifi_event *out)
{
    int status;
    taskENTER_CRITICAL();
    if (seq == 0 || seq > wellfed_seq) {
        status = 0;
    } else if (wellfed_seq - seq >= WELLFED_WIFI_EVENTS) {
        status = -1;
    } else {
        *out = wellfed_events[(seq - 1) % WELLFED_WIFI_EVENTS];
        status = 1;
    }
    taskEXIT_CRITICAL();
    return status;
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    wellfed_app_start();
}
