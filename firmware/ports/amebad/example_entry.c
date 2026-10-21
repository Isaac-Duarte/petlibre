/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 *
 * FW07 stage D: one task joins the configured network and keeps it. It runs
 * LwIP_Init and wifi_on(STA), then repeats: scan for the network (to learn
 * its security), wifi_connect, DHCP; on success it resolves a fixed public
 * name every few seconds and watches the link. When the link drops, or DNS
 * fails several times in a row, it leaves and rejoins with bounded backoff,
 * which resets only after a lookup succeeds. The SDK's static-address DHCP
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
#include "lwip/dns.h"
#include "lwip/sockets.h"
#include <platform/platform_stdlib.h>
#include "petlibre_wifi_credentials.h"

void petlibre_app_start(void);

#define PETLIBRE_WIFI_EVENTS 16
#define PETLIBRE_SCAN_WAIT_MS 10000
#define PETLIBRE_WATCH_MS 500
#define PETLIBRE_BACKOFF_MIN_MS 1000
#define PETLIBRE_BACKOFF_MAX_MS 30000
/* Asked of the DNS server from DHCP with our own query every time, so lwIP's
 * resolver cache cannot answer for a server that has stopped responding. */
#define PETLIBRE_DNS_LABELS "\x07" "example" "\x03" "com"
#define PETLIBRE_DNS_PERIOD_MS 5000
#define PETLIBRE_DNS_TIMEOUT_MS 3000
#define PETLIBRE_DNS_FAILURES 2

/* Event kinds; shared with Rust (`sdk::WifiEvent`). */
enum {
    PETLIBRE_EV_ON = 1,      /* result: wifi_on; ms: duration */
    PETLIBRE_EV_SCAN = 2,    /* result: scan; value: found (0/1); rssi, security */
    PETLIBRE_EV_CONNECT = 3, /* result: wifi_connect; ms */
    PETLIBRE_EV_DHCP = 4,    /* result: LwIP_DHCP state; value: IPv4 (host order) when assigned; ms */
    PETLIBRE_EV_LOST = 5,    /* result: 0 link dropped, 1 DNS failed; value: ms connected */
    PETLIBRE_EV_BACKOFF = 6, /* value: ms until the next attempt */
    PETLIBRE_EV_DNS = 7,     /* result: 0 or a PETLIBRE_DNS_* failure; value: IPv4 (host order) when resolved; ms */
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

/* DNS lookup failures, as the event result. */
enum {
    PETLIBRE_DNS_NO_SERVER = -1,
    PETLIBRE_DNS_SOCKET = -2,
    PETLIBRE_DNS_SEND = -3,
    PETLIBRE_DNS_TIMEOUT = -4,
    PETLIBRE_DNS_MISMATCH = -5, /* wrong ID, not a response, or truncated */
    PETLIBRE_DNS_RCODE = -6,
    PETLIBRE_DNS_NO_ADDRESS = -7,
};

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

/* Skip a possibly compressed name at `at`; the offset after it, or -1. */
static int petlibre_dns_skip_name(const uint8_t *m, int length, int at)
{
    while (at < length) {
        if ((m[at] & 0xc0) == 0xc0) {
            return at + 2 <= length ? at + 2 : -1;
        }
        if (m[at] == 0) {
            return at + 1;
        }
        at += 1 + m[at];
    }
    return -1;
}

/* One A query for the fixed name; 0 and *ip set, or a PETLIBRE_DNS_* failure. */
static int petlibre_dns_query(uint16_t id, unsigned int *ip)
{
    const ip_addr_t *server = dns_getserver(0);
    if (server == NULL || ip_addr_isany(server)) {
        return PETLIBRE_DNS_NO_SERVER;
    }
    uint8_t m[512];
    static const uint8_t question[] = PETLIBRE_DNS_LABELS "\x00" "\x00\x01" "\x00\x01"; /* A, IN */
    memset(m, 0, 12);
    m[0] = id >> 8;
    m[1] = id & 0xff;
    m[2] = 0x01; /* recursion desired */
    m[5] = 1;    /* one question */
    memcpy(m + 12, question, sizeof(question) - 1);
    int length = 12 + (int)sizeof(question) - 1;

    int s = lwip_socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) {
        return PETLIBRE_DNS_SOCKET;
    }
    int timeout = PETLIBRE_DNS_TIMEOUT_MS;
    lwip_setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_port = PP_HTONS(53);
    to.sin_addr.s_addr = ip4_addr_get_u32(ip_2_ip4(server));
    int status = PETLIBRE_DNS_SEND;
    if (lwip_sendto(s, m, length, 0, (struct sockaddr *)&to, sizeof(to)) == length) {
        status = PETLIBRE_DNS_TIMEOUT;
        for (int datagrams = 0; datagrams < 4; datagrams++) {
            int got = lwip_recv(s, m, sizeof(m), 0);
            if (got < 0) {
                break;
            }
            /* Skip a few stray datagrams; each waits at most the timeout. */
            if (got < 12 || m[0] != (id >> 8) || m[1] != (id & 0xff)) {
                status = PETLIBRE_DNS_MISMATCH;
                continue;
            }
            if (!(m[2] & 0x80) || (m[2] & 0x02)) {
                status = PETLIBRE_DNS_MISMATCH;
                break;
            }
            if (m[3] & 0x0f) {
                status = PETLIBRE_DNS_RCODE;
                break;
            }
            status = PETLIBRE_DNS_NO_ADDRESS;
            int at = 12;
            for (int q = (m[4] << 8) | m[5]; q > 0 && at >= 0; q--) {
                at = petlibre_dns_skip_name(m, got, at);
                at = at >= 0 && at + 4 <= got ? at + 4 : -1;
            }
            for (int a = (m[6] << 8) | m[7]; a > 0 && at >= 0; a--) {
                at = petlibre_dns_skip_name(m, got, at);
                if (at < 0 || at + 10 > got) {
                    break;
                }
                int type = (m[at] << 8) | m[at + 1];
                int size = (m[at + 8] << 8) | m[at + 9];
                at += 10;
                if (at + size > got) {
                    break;
                }
                if (type == 1 && size == 4) {
                    *ip = ((unsigned int)m[at] << 24) | ((unsigned int)m[at + 1] << 16) |
                          ((unsigned int)m[at + 2] << 8) | m[at + 3];
                    status = *ip != 0 ? 0 : PETLIBRE_DNS_NO_ADDRESS;
                    break;
                }
                at += size;
            }
            break;
        }
    }
    lwip_close(s);
    return status;
}

/* One lookup of the fixed name. 1 when it resolved to a nonzero address. */
static int petlibre_dns(unsigned int attempt)
{
    static uint16_t id;
    unsigned int ip = 0;
    TickType_t begin = xTaskGetTickCount();
    id = (uint16_t)(id + 1 + (begin & 0xff));
    int result = petlibre_dns_query(id, &ip);
    if (result != 0) {
        ip = 0;
    }
    petlibre_event(PETLIBRE_EV_DNS, attempt, result, ip, petlibre_ms(begin));
    return result == 0;
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
            TickType_t joined = xTaskGetTickCount();
            TickType_t looked = joined;
            int due = 1;
            unsigned int failures = 0;
            int dns_lost = 0;
            while (wifi_is_connected_to_ap() == RTW_SUCCESS) {
                if (due || petlibre_ms(looked) >= PETLIBRE_DNS_PERIOD_MS) {
                    due = 0;
                    looked = xTaskGetTickCount();
                    if (petlibre_dns(attempt)) {
                        failures = 0;
                        backoff = PETLIBRE_BACKOFF_MIN_MS;
                    } else if (++failures >= PETLIBRE_DNS_FAILURES) {
                        dns_lost = 1;
                        break;
                    }
                }
                vTaskDelay(PETLIBRE_WATCH_MS / portTICK_PERIOD_MS);
            }
            petlibre_event(PETLIBRE_EV_LOST, attempt, dns_lost, petlibre_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
            if (dns_lost) {
                wifi_disconnect();
            }
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
