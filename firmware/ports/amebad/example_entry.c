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
 *
 * FW08: blocking TCP helpers for the Rust transport client, and the broker
 * settings from a second generated header. The client and its protocol are
 * Rust (wellfed-core transport); nothing here parses network data.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"
#include "lwip/dns.h"
#include "lwip/sockets.h"
#include <platform/platform_stdlib.h>
#include "wellfed_wifi_credentials.h"
#include "wellfed_transport_credentials.h"

void wellfed_app_start(void);

#define WELLFED_WIFI_EVENTS 16
#define WELLFED_SCAN_WAIT_MS 10000
#define WELLFED_WATCH_MS 500
#define WELLFED_BACKOFF_MIN_MS 1000
#define WELLFED_BACKOFF_MAX_MS 30000
/* Asked of the DNS server from DHCP with our own query every time, so lwIP's
 * resolver cache cannot answer for a server that has stopped responding. */
#define WELLFED_DNS_LABELS "\x07" "example" "\x03" "com"
#define WELLFED_DNS_PERIOD_MS 5000
#define WELLFED_DNS_TIMEOUT_MS 3000
#define WELLFED_DNS_FAILURES 2

/* Event kinds; shared with Rust (`sdk::WifiEvent`). */
enum {
    WELLFED_EV_ON = 1,      /* result: wifi_on; ms: duration */
    WELLFED_EV_SCAN = 2,    /* result: scan; value: found (0/1); rssi, security */
    WELLFED_EV_CONNECT = 3, /* result: wifi_connect; ms */
    WELLFED_EV_DHCP = 4,    /* result: LwIP_DHCP state; value: IPv4 (host order) when assigned; ms */
    WELLFED_EV_LOST = 5,    /* result: 0 link dropped, 1 DNS failed; value: ms connected */
    WELLFED_EV_BACKOFF = 6, /* value: ms until the next attempt */
    WELLFED_EV_DNS = 7,     /* result: 0 or a WELLFED_DNS_* failure; value: IPv4 (host order) when resolved; ms */
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

/* DNS lookup failures, as the event result. */
enum {
    WELLFED_DNS_NO_SERVER = -1,
    WELLFED_DNS_SOCKET = -2,
    WELLFED_DNS_SEND = -3,
    WELLFED_DNS_TIMEOUT = -4,
    WELLFED_DNS_MISMATCH = -5, /* wrong ID, not a response, or truncated */
    WELLFED_DNS_RCODE = -6,
    WELLFED_DNS_NO_ADDRESS = -7,
};

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

/* Skip a possibly compressed name at `at`; the offset after it, or -1. */
static int wellfed_dns_skip_name(const uint8_t *m, int length, int at)
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

/* One A query for the fixed name; 0 and *ip set, or a WELLFED_DNS_* failure. */
static int wellfed_dns_query(uint16_t id, unsigned int *ip)
{
    const ip_addr_t *server = dns_getserver(0);
    if (server == NULL || ip_addr_isany(server)) {
        return WELLFED_DNS_NO_SERVER;
    }
    uint8_t m[512];
    static const uint8_t question[] = WELLFED_DNS_LABELS "\x00" "\x00\x01" "\x00\x01"; /* A, IN */
    memset(m, 0, 12);
    m[0] = id >> 8;
    m[1] = id & 0xff;
    m[2] = 0x01; /* recursion desired */
    m[5] = 1;    /* one question */
    memcpy(m + 12, question, sizeof(question) - 1);
    int length = 12 + (int)sizeof(question) - 1;

    int s = lwip_socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) {
        return WELLFED_DNS_SOCKET;
    }
    int timeout = WELLFED_DNS_TIMEOUT_MS;
    lwip_setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_port = PP_HTONS(53);
    to.sin_addr.s_addr = ip4_addr_get_u32(ip_2_ip4(server));
    int status = WELLFED_DNS_SEND;
    if (lwip_sendto(s, m, length, 0, (struct sockaddr *)&to, sizeof(to)) == length) {
        status = WELLFED_DNS_TIMEOUT;
        for (int datagrams = 0; datagrams < 4; datagrams++) {
            int got = lwip_recv(s, m, sizeof(m), 0);
            if (got < 0) {
                break;
            }
            /* Skip a few stray datagrams; each waits at most the timeout. */
            if (got < 12 || m[0] != (id >> 8) || m[1] != (id & 0xff)) {
                status = WELLFED_DNS_MISMATCH;
                continue;
            }
            if (!(m[2] & 0x80) || (m[2] & 0x02)) {
                status = WELLFED_DNS_MISMATCH;
                break;
            }
            if (m[3] & 0x0f) {
                status = WELLFED_DNS_RCODE;
                break;
            }
            status = WELLFED_DNS_NO_ADDRESS;
            int at = 12;
            for (int q = (m[4] << 8) | m[5]; q > 0 && at >= 0; q--) {
                at = wellfed_dns_skip_name(m, got, at);
                at = at >= 0 && at + 4 <= got ? at + 4 : -1;
            }
            for (int a = (m[6] << 8) | m[7]; a > 0 && at >= 0; a--) {
                at = wellfed_dns_skip_name(m, got, at);
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
                    status = *ip != 0 ? 0 : WELLFED_DNS_NO_ADDRESS;
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
static int wellfed_dns(unsigned int attempt)
{
    static uint16_t id;
    unsigned int ip = 0;
    TickType_t begin = xTaskGetTickCount();
    id = (uint16_t)(id + 1 + (begin & 0xff));
    int result = wellfed_dns_query(id, &ip);
    if (result != 0) {
        ip = 0;
    }
    wellfed_event(WELLFED_EV_DNS, attempt, result, ip, wellfed_ms(begin));
    return result == 0;
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
            TickType_t joined = xTaskGetTickCount();
            TickType_t looked = joined;
            int due = 1;
            unsigned int failures = 0;
            int dns_lost = 0;
            while (wifi_is_connected_to_ap() == RTW_SUCCESS) {
                if (due || wellfed_ms(looked) >= WELLFED_DNS_PERIOD_MS) {
                    due = 0;
                    looked = xTaskGetTickCount();
                    if (wellfed_dns(attempt)) {
                        failures = 0;
                        backoff = WELLFED_BACKOFF_MIN_MS;
                    } else if (++failures >= WELLFED_DNS_FAILURES) {
                        dns_lost = 1;
                        break;
                    }
                }
                vTaskDelay(WELLFED_WATCH_MS / portTICK_PERIOD_MS);
            }
            wellfed_event(WELLFED_EV_LOST, attempt, dns_lost, wellfed_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
            if (dns_lost) {
                wifi_disconnect();
            }
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

/* Broker settings; shared with Rust (`sdk::TransportConfig`). */
struct wellfed_transport_config {
    unsigned int broker; /* IPv4, host order */
    unsigned int port;
    const char *client_id;
    const char *username;
    const char *password;
    unsigned int password_len;
    const char *namespace_;
};

void wellfed_transport_config(struct wellfed_transport_config *out)
{
    out->broker = WELLFED_MQTT_BROKER;
    out->port = WELLFED_MQTT_PORT;
    out->client_id = WELLFED_MQTT_CLIENT_ID;
    out->username = WELLFED_MQTT_USERNAME;
    out->password = WELLFED_MQTT_PASSWORD;
    out->password_len = sizeof(WELLFED_MQTT_PASSWORD) - 1;
    out->namespace_ = WELLFED_MQTT_NAMESPACE;
}

/* A pending socket error, cleared by reading it. */
static int wellfed_socket_error(int s)
{
    int error = 0;
    socklen_t size = sizeof(error);
    if (lwip_getsockopt(s, SOL_SOCKET, SO_ERROR, &error, &size) != 0) {
        return -1;
    }
    return error;
}

/* Connect within `timeout_ms`: a socket, or -1. */
int wellfed_tcp_connect(unsigned int ip, unsigned int port, unsigned int timeout_ms)
{
    int s = lwip_socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        return -1;
    }
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_port = lwip_htons((u16_t)port);
    to.sin_addr.s_addr = lwip_htonl(ip);
    int flags = lwip_fcntl(s, F_GETFL, 0);
    lwip_fcntl(s, F_SETFL, flags | O_NONBLOCK);
    int connected = lwip_connect(s, (struct sockaddr *)&to, sizeof(to)) == 0;
    if (!connected) {
        fd_set writable;
        FD_ZERO(&writable);
        FD_SET(s, &writable);
        struct timeval wait = {timeout_ms / 1000, (timeout_ms % 1000) * 1000};
        connected = lwip_select(s + 1, NULL, &writable, NULL, &wait) == 1 && wellfed_socket_error(s) == 0;
    }
    if (!connected) {
        lwip_close(s);
        return -1;
    }
    lwip_fcntl(s, F_SETFL, flags);
    int send_timeout = 3000;
    lwip_setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &send_timeout, sizeof(send_timeout));
    return s;
}

/* Send all of `data`: 0, or -1 on error or a stalled peer. */
int wellfed_tcp_send(int s, const void *data, unsigned int length)
{
    const char *at = data;
    while (length > 0) {
        int sent = lwip_send(s, at, length, 0);
        if (sent <= 0) {
            return -1;
        }
        at += sent;
        length -= (unsigned int)sent;
    }
    return 0;
}

/* Bytes received, 0 when `timeout_ms` passed with none, -1 when closed. */
int wellfed_tcp_recv(int s, void *buffer, unsigned int length, unsigned int timeout_ms)
{
    int timeout = (int)timeout_ms;
    lwip_setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    int got = lwip_recv(s, buffer, length, 0);
    if (got > 0) {
        return got;
    }
    if (got < 0) {
        int error = wellfed_socket_error(s);
        if (error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

void wellfed_tcp_close(int s)
{
    lwip_close(s);
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    wellfed_app_start();
}
