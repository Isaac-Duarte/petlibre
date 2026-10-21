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
#include "petlibre_wifi_credentials.h"

void petlibre_app_start(void);

#define PETLIBRE_WIFI_EVENTS 16
#define PETLIBRE_SCAN_WAIT_MS 10000
#define PETLIBRE_WATCH_MS 500
#define PETLIBRE_BACKOFF_MIN_MS 1000
#define PETLIBRE_BACKOFF_MAX_MS 30000

/* Event kinds; shared with Rust (`sdk::WifiEvent`). */
enum {
    PETLIBRE_EV_ON = 1,      /* result: wifi_on; ms: duration */
    PETLIBRE_EV_SCAN = 2,    /* result: scan; value: found (0/1); rssi, security */
    PETLIBRE_EV_CONNECT = 3, /* result: wifi_connect; ms */
    PETLIBRE_EV_DHCP = 4,    /* result: LwIP_DHCP state; value: IPv4 (host order) when assigned; ms */
    PETLIBRE_EV_LOST = 5,    /* value: ms connected */
    PETLIBRE_EV_BACKOFF = 6, /* value: ms until the next attempt */
};

struct petlibre_wifi_event {
    unsigned int seq;
    unsigned int kind;
    unsigned int attempt;
    int result;
    unsigned int value;
    int rssi;
    unsigned int security;
    unsigned int ms;
};

static struct petlibre_wifi_event petlibre_events[PETLIBRE_WIFI_EVENTS];
static unsigned int petlibre_seq;
static unsigned int petlibre_started;
static volatile unsigned int petlibre_scan_done;
static volatile int petlibre_found_rssi;
static volatile unsigned int petlibre_found_security;
static volatile unsigned int petlibre_found;

static void petlibre_event(unsigned int kind, unsigned int attempt, int result, unsigned int value, unsigned int ms)
{
    taskENTER_CRITICAL();
    struct petlibre_wifi_event *e = &petlibre_events[petlibre_seq % PETLIBRE_WIFI_EVENTS];
    e->seq = ++petlibre_seq;
    e->kind = kind;
    e->attempt = attempt;
    e->result = result;
    e->value = value;
    e->rssi = kind == PETLIBRE_EV_SCAN ? petlibre_found_rssi : 0;
    e->security = kind == PETLIBRE_EV_SCAN ? petlibre_found_security : 0;
    e->ms = ms;
    taskEXIT_CRITICAL();
}

static unsigned int petlibre_ms(TickType_t since)
{
    return (unsigned int)((xTaskGetTickCount() - since) * portTICK_PERIOD_MS);
}

static rtw_result_t petlibre_scan_result(rtw_scan_handler_result_t *result)
{
    if (result->scan_complete == RTW_TRUE) {
        petlibre_scan_done = 1;
        return RTW_SUCCESS;
    }
    rtw_scan_result_t *ap = &result->ap_details;
    unsigned int length = sizeof(PETLIBRE_WIFI_SSID) - 1;
    if (ap->SSID.len == length && memcmp(ap->SSID.val, PETLIBRE_WIFI_SSID, length) == 0 &&
        (!petlibre_found || ap->signal_strength > petlibre_found_rssi)) {
        petlibre_found_rssi = ap->signal_strength;
        petlibre_found_security = (unsigned int)ap->security;
        petlibre_found = 1;
    }
    return RTW_SUCCESS;
}

/* One join attempt: scan, connect, DHCP. 1 when an address was assigned. */
static int petlibre_join(unsigned int attempt)
{
    petlibre_found = 0;
    petlibre_scan_done = 0;
    TickType_t begin = xTaskGetTickCount();
    int result = wifi_scan_networks(petlibre_scan_result, NULL);
    while (result == RTW_SUCCESS && !petlibre_scan_done && petlibre_ms(begin) < PETLIBRE_SCAN_WAIT_MS) {
        vTaskDelay(50 / portTICK_PERIOD_MS);
    }
    if (result == RTW_SUCCESS && !petlibre_scan_done) {
        result = RTW_TIMEOUT;
    }
    petlibre_event(PETLIBRE_EV_SCAN, attempt, result, petlibre_found, petlibre_ms(begin));
    if (result != RTW_SUCCESS || !petlibre_found) {
        return 0;
    }
    begin = xTaskGetTickCount();
    result = wifi_connect(PETLIBRE_WIFI_SSID, (rtw_security_t)petlibre_found_security, PETLIBRE_WIFI_PASSWORD,
                          sizeof(PETLIBRE_WIFI_SSID) - 1, sizeof(PETLIBRE_WIFI_PASSWORD) - 1, -1, NULL);
    petlibre_event(PETLIBRE_EV_CONNECT, attempt, result, 0, petlibre_ms(begin));
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
    petlibre_event(PETLIBRE_EV_DHCP, attempt, state, ip, petlibre_ms(begin));
    if (state != DHCP_ADDRESS_ASSIGNED) {
        /* Never keep the SDK's static fallback address. */
        LwIP_DHCP(0, DHCP_STOP);
        LwIP_ReleaseIP(0);
        wifi_disconnect();
        return 0;
    }
    return 1;
}

static void petlibre_wifi_task(void *unused)
{
    (void)unused;
    TickType_t begin = xTaskGetTickCount();
    LwIP_Init();
    int on = wifi_on(RTW_MODE_STA);
    petlibre_event(PETLIBRE_EV_ON, 0, on, 0, petlibre_ms(begin));
    if (on != RTW_SUCCESS) {
        vTaskDelete(NULL);
        return;
    }
    unsigned int attempt = 0;
    unsigned int backoff = PETLIBRE_BACKOFF_MIN_MS;
    for (;;) {
        attempt++;
        if (petlibre_join(attempt)) {
            backoff = PETLIBRE_BACKOFF_MIN_MS;
            TickType_t joined = xTaskGetTickCount();
            while (wifi_is_connected_to_ap() == RTW_SUCCESS) {
                vTaskDelay(PETLIBRE_WATCH_MS / portTICK_PERIOD_MS);
            }
            petlibre_event(PETLIBRE_EV_LOST, attempt, 0, petlibre_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
        }
        petlibre_event(PETLIBRE_EV_BACKOFF, attempt, 0, backoff, 0);
        vTaskDelay(backoff / portTICK_PERIOD_MS);
        backoff = backoff * 2 > PETLIBRE_BACKOFF_MAX_MS ? PETLIBRE_BACKOFF_MAX_MS : backoff * 2;
    }
}

/* Once per boot: 1 when the task was created. */
int petlibre_wifi_start(void)
{
    if (petlibre_started) {
        return 0;
    }
    petlibre_started = 1;
    if (xTaskCreate(petlibre_wifi_task, "petlibre_wifi", 1280, NULL, tskIDLE_PRIORITY + 3, NULL) != pdPASS) {
        petlibre_started = 0;
        return 0;
    }
    return 1;
}

/* Copy the event with sequence number `seq` (from 1); 0 when it is not yet
 * recorded, -1 when it was overwritten. */
int petlibre_wifi_event(unsigned int seq, struct petlibre_wifi_event *out)
{
    int status;
    taskENTER_CRITICAL();
    if (seq == 0 || seq > petlibre_seq) {
        status = 0;
    } else if (petlibre_seq - seq >= PETLIBRE_WIFI_EVENTS) {
        status = -1;
    } else {
        *out = petlibre_events[(seq - 1) % PETLIBRE_WIFI_EVENTS];
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
    petlibre_app_start();
}
