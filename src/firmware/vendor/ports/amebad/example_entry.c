/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 *
 * One task joins the configured network and keeps it. It runs
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
 * Blocking TCP helpers for the Rust transport client, and the broker
 * settings from a second generated header. The client and its protocol are
 * Rust (petlibre-core transport); nothing here parses network data.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"
#include "lwip/dns.h"
#include "lwip/dhcp.h"
#include "lwip/sockets.h"
#include <platform/platform_stdlib.h>
#include "petlibre_wifi_credentials.h"
#include "petlibre_transport_credentials.h"
#include "rl6548.h"

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

/* Join credentials, shared with Rust (`network::JoinCredentials`):
 * prefilled with the build's, replaced by stored ones unless the fallback
 * has taken over (petlibre-hal network.rs). */
struct petlibre_join_credentials {
    const unsigned char *ssid;
    unsigned int ssid_len;
    const unsigned char *psk;
    unsigned int psk_len;
    unsigned int source; /* 0 build, 1 stored, 2 fallback */
};
void petlibre_wifi_select(struct petlibre_join_credentials *out);
void petlibre_wifi_outcome(int joined);
static struct petlibre_join_credentials petlibre_creds;

static rtw_result_t petlibre_scan_result(rtw_scan_handler_result_t *result)
{
    if (result->scan_complete == RTW_TRUE) {
        petlibre_scan_done = 1;
        return RTW_SUCCESS;
    }
    rtw_scan_result_t *ap = &result->ap_details;
    unsigned int length = petlibre_creds.ssid_len;
    if (ap->SSID.len == length && memcmp(ap->SSID.val, petlibre_creds.ssid, length) == 0 &&
        (!petlibre_found || ap->signal_strength > petlibre_found_rssi)) {
        petlibre_found_rssi = ap->signal_strength;
        petlibre_found_security = (unsigned int)ap->security;
        petlibre_found = 1;
    }
    return RTW_SUCCESS;
}

/* One join attempt: scan, connect, DHCP. 1 when an address was assigned. */
static int petlibre_join_once(unsigned int attempt)
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
    result = wifi_connect((char *)petlibre_creds.ssid, (rtw_security_t)petlibre_found_security,
                          (char *)petlibre_creds.psk, (int)petlibre_creds.ssid_len, (int)petlibre_creds.psk_len,
                          -1, NULL);
    /* The event's value names the credentials' source, never the credentials. */
    petlibre_event(PETLIBRE_EV_CONNECT, attempt, result, petlibre_creds.source, petlibre_ms(begin));
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

/* One attempt with the credentials Rust selects, reporting the outcome. */
static int petlibre_join(unsigned int attempt)
{
    petlibre_creds.ssid = (const unsigned char *)PETLIBRE_WIFI_SSID;
    petlibre_creds.ssid_len = sizeof(PETLIBRE_WIFI_SSID) - 1;
    petlibre_creds.psk = (const unsigned char *)PETLIBRE_WIFI_PASSWORD;
    petlibre_creds.psk_len = sizeof(PETLIBRE_WIFI_PASSWORD) - 1;
    petlibre_creds.source = 0;
    petlibre_wifi_select(&petlibre_creds);
    int joined = petlibre_join_once(attempt);
    petlibre_wifi_outcome(joined);
    return joined;
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

/* Broker settings; shared with Rust (`sdk::TransportConfig`). */
struct petlibre_transport_config {
    unsigned int broker; /* IPv4, host order */
    unsigned int port;
    const char *client_id;
    const char *username;
    const char *password;
    unsigned int password_len;
    const char *namespace_;
    const char *request_key; /* 64 lowercase hex characters, or "" */
};

void petlibre_transport_config(struct petlibre_transport_config *out)
{
    out->broker = PETLIBRE_MQTT_BROKER;
    out->port = PETLIBRE_MQTT_PORT;
    out->client_id = PETLIBRE_MQTT_CLIENT_ID;
    out->username = PETLIBRE_MQTT_USERNAME;
    out->password = PETLIBRE_MQTT_PASSWORD;
    out->password_len = sizeof(PETLIBRE_MQTT_PASSWORD) - 1;
    out->namespace_ = PETLIBRE_MQTT_NAMESPACE;
    out->request_key = PETLIBRE_MQTT_REQUEST_KEY;
}

u32 Rand(void); /* ROM (swlib/string/rand.h): a seeded software generator */

/* Inputs for a session's request nonce (petlibre-core request_auth): the ROM
 * generator, the CPU-rate SysTick count and the tick count at the moment of a
 * connect, whose timing varies with the network. Not a vetted RNG; the nonce
 * only has to differ between sessions. */
void petlibre_entropy(unsigned int out[4])
{
    out[0] = Rand();
    out[1] = *(volatile unsigned int *)0xE000E018; /* SysTick current value */
    out[2] = xTaskGetTickCount();
    out[3] = Rand();
}

/* A pending socket error, cleared by reading it. */
static int petlibre_socket_error(int s)
{
    int error = 0;
    socklen_t size = sizeof(error);
    if (lwip_getsockopt(s, SOL_SOCKET, SO_ERROR, &error, &size) != 0) {
        return -1;
    }
    return error;
}

/* Connect within `timeout_ms`: a socket, or -1. */
int petlibre_tcp_connect(unsigned int ip, unsigned int port, unsigned int timeout_ms)
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
        connected = lwip_select(s + 1, NULL, &writable, NULL, &wait) == 1 && petlibre_socket_error(s) == 0;
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
int petlibre_tcp_send(int s, const void *data, unsigned int length)
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
int petlibre_tcp_recv(int s, void *buffer, unsigned int length, unsigned int timeout_ms)
{
    int timeout = (int)timeout_ms;
    lwip_setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    int got = lwip_recv(s, buffer, length, 0);
    if (got > 0) {
        return got;
    }
    if (got < 0) {
        int error = petlibre_socket_error(s);
        if (error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

void petlibre_tcp_close(int s)
{
    lwip_close(s);
}

/* The speaker variant's listener (petlibre-hal stream.rs): a socket on
 * `port` for every local address, or -1. */
int petlibre_tcp_listen(unsigned int port)
{
    int s = lwip_socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        return -1;
    }
    int reuse = 1;
    lwip_setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    struct sockaddr_in at;
    memset(&at, 0, sizeof(at));
    at.sin_family = AF_INET;
    at.sin_port = lwip_htons((u16_t)port);
    at.sin_addr.s_addr = lwip_htonl(INADDR_ANY);
    if (lwip_bind(s, (struct sockaddr *)&at, sizeof(at)) != 0 || lwip_listen(s, 1) != 0) {
        lwip_close(s);
        return -1;
    }
    return s;
}

/* One connection within `timeout_ms`: a socket, or -1. */
int petlibre_tcp_accept(int listener, unsigned int timeout_ms)
{
    fd_set readable;
    FD_ZERO(&readable);
    FD_SET(listener, &readable);
    struct timeval wait = {timeout_ms / 1000, (timeout_ms % 1000) * 1000};
    if (lwip_select(listener + 1, &readable, NULL, NULL, &wait) != 1) {
        return -1;
    }
    int s = lwip_accept(listener, NULL, NULL);
    if (s < 0) {
        return -1;
    }
    int send_timeout = 3000;
    lwip_setsockopt(s, SOL_SOCKET, SO_SNDTIMEO, &send_timeout, sizeof(send_timeout));
    return s;
}

/* The device API's sockets. The loop task serves every client and
 * steps the feeding owner between them, so nothing here ever waits: each
 * call takes what is ready now and returns (petlibre-hal api.rs). */
static void petlibre_nonblocking(int s)
{
    int flags = lwip_fcntl(s, F_GETFL, 0);
    lwip_fcntl(s, F_SETFL, flags | O_NONBLOCK);
}

/* A listener on `port` for every local address, or -1. */
int petlibre_api_listen(unsigned int port)
{
    int s = lwip_socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) {
        return -1;
    }
    int reuse = 1;
    lwip_setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    struct sockaddr_in at;
    memset(&at, 0, sizeof(at));
    at.sin_family = AF_INET;
    at.sin_port = lwip_htons((u16_t)port);
    at.sin_addr.s_addr = lwip_htonl(INADDR_ANY);
    if (lwip_bind(s, (struct sockaddr *)&at, sizeof(at)) != 0 || lwip_listen(s, 2) != 0) {
        lwip_close(s);
        return -1;
    }
    petlibre_nonblocking(s);
    return s;
}

/* A waiting connection: a socket, -1 when none waits, -2 on an error. */
int petlibre_api_accept(int listener)
{
    int s = lwip_accept(listener, NULL, NULL);
    if (s < 0) {
        int error = petlibre_socket_error(listener);
        return (error == 0 || error == EAGAIN || error == EWOULDBLOCK) ? -1 : -2;
    }
    petlibre_nonblocking(s);
    int on = 1;
    lwip_setsockopt(s, IPPROTO_TCP, TCP_NODELAY, &on, sizeof(on));
    return s;
}

/* Bytes received now; 0 when none are waiting, -1 once closed or failed. */
int petlibre_api_recv(int s, void *buffer, unsigned int length)
{
    int got = lwip_recv(s, buffer, length, MSG_DONTWAIT);
    if (got > 0) {
        return got;
    }
    if (got < 0) {
        int error = petlibre_socket_error(s);
        if (error == 0 || error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

/* Bytes the stack took now (0 while its send buffer is full), -1 on error. */
int petlibre_api_send(int s, const void *data, unsigned int length)
{
    int sent = lwip_send(s, data, length, MSG_DONTWAIT);
    if (sent >= 0) {
        return sent;
    }
    int error = petlibre_socket_error(s);
    return (error == 0 || error == EAGAIN || error == EWOULDBLOCK) ? 0 : -1;
}

/* The mDNS socket: UDP 5353, a member of 224.0.0.251, or -1. Opened only
 * once DHCP gave an address: a socket opened on the static fallback never
 * saw a multicast query (physical, fw47-api-02). */
int petlibre_mdns_open(void)
{
    int s = lwip_socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) {
        return -1;
    }
    int reuse = 1;
    lwip_setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &reuse, sizeof(reuse));
    struct sockaddr_in at;
    memset(&at, 0, sizeof(at));
    at.sin_family = AF_INET;
    at.sin_port = lwip_htons(5353);
    at.sin_addr.s_addr = lwip_htonl(INADDR_ANY);
    struct ip_mreq join;
    memset(&join, 0, sizeof(join));
    join.imr_multiaddr.s_addr = lwip_htonl(0xe00000fbu);
    join.imr_interface.s_addr = lwip_htonl(INADDR_ANY);
    u8_t ttl = 255;
    if (lwip_bind(s, (struct sockaddr *)&at, sizeof(at)) != 0
        || lwip_setsockopt(s, IPPROTO_IP, IP_ADD_MEMBERSHIP, &join, sizeof(join)) != 0) {
        lwip_close(s);
        return -1;
    }
    lwip_setsockopt(s, IPPROTO_IP, IP_MULTICAST_TTL, &ttl, sizeof(ttl));
    petlibre_nonblocking(s);
    return s;
}

/* One datagram received now, with its sender; 0 when none, -1 on error. */
int petlibre_mdns_recv(int s, void *buffer, unsigned int length, unsigned int *ip, unsigned int *port)
{
    struct sockaddr_in from;
    socklen_t size = sizeof(from);
    int got = lwip_recvfrom(s, buffer, length, MSG_DONTWAIT, (struct sockaddr *)&from, &size);
    if (got > 0) {
        *ip = lwip_ntohl(from.sin_addr.s_addr);
        *port = lwip_ntohs(from.sin_port);
        return got;
    }
    if (got < 0) {
        int error = petlibre_socket_error(s);
        if (error == 0 || error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

/* Send one datagram to `ip`:`port`: 0, or -1. */
int petlibre_mdns_send(int s, const void *data, unsigned int length, unsigned int ip, unsigned int port)
{
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_port = lwip_htons((u16_t)port);
    to.sin_addr.s_addr = lwip_htonl(ip);
    int sent = lwip_sendto(s, data, length, MSG_DONTWAIT, (struct sockaddr *)&to, sizeof(to));
    return sent == (int)length ? 0 : -1;
}

/* The station's IPv4 address, host order, or 0 until DHCP gave one: before
 * that the SDK reports its static fallback (192.168.1.80). */
unsigned int petlibre_ipv4(void)
{
    if (!dhcp_supplied_address(&xnetif[0])) {
        return 0;
    }
    uint8_t *a = LwIP_GetIP(&xnetif[0]);
    return ((unsigned int)a[0] << 24) | ((unsigned int)a[1] << 16) | ((unsigned int)a[2] << 8) | a[3];
}

/* The speaker. Stock 1.1.15 plays through the codec's line output
 * (CODEC_Init application 8) from SPORT by GDMA and holds PB23 HIGH while it
 * plays, the amplifier enable that Rust drives (petlibre-hal audio.rs).
 * This shim owns only the codec, SPORT and an 8-page DMA ring of 16-bit mono
 * at 8, 16, 32 or 48 kHz (the rates the I2S PLL divides to). Rust fills pages and decides when to open and close. When no
 * filled page is ready the DMA plays a zero page and counts an underrun, so
 * the stream never stops on its own. */
/* Defined in the pinned fwlib (rtl8721d_pll.c, rtl8721dhp_audio.c) with no
 * header declaration. */
void PLL_Div(u32 div);
BOOL AUDIO_SP_TXGDMA_Restart(u8 GDMA_Index, u8 GDMA_ChNum, u32 tx_addr, u32 tx_length);

#define PETLIBRE_AUDIO_PAGE 512 /* bytes: 256 samples, 16 ms at 16 kHz */
#define PETLIBRE_AUDIO_PAGES 8

/* Multiples of 32 bytes at 32-byte alignment: the restart cleans the cache
 * over each page the DMA reads. */
static u8 petlibre_audio_ring[PETLIBRE_AUDIO_PAGE * PETLIBRE_AUDIO_PAGES] __attribute__((aligned(32)));
static u8 petlibre_audio_zero[PETLIBRE_AUDIO_PAGE] __attribute__((aligned(32)));
/* Filled pages the DMA owns; set by Rust's task, cleared by the DMA interrupt. */
static volatile u8 petlibre_audio_own[PETLIBRE_AUDIO_PAGES];
static unsigned int petlibre_audio_user;
static volatile unsigned int petlibre_audio_dma;
static volatile unsigned int petlibre_audio_on_zero;
static volatile unsigned int petlibre_audio_underruns;
static GDMA_InitTypeDef petlibre_audio_gdma;
static int petlibre_audio_opened;

static u32 petlibre_audio_done(void *data)
{
    GDMA_InitTypeDef *gdma = data;
    GDMA_ClearINT(gdma->GDMA_Index, gdma->GDMA_ChNum);
    if (!petlibre_audio_on_zero) {
        petlibre_audio_own[petlibre_audio_dma] = 0;
        petlibre_audio_dma = (petlibre_audio_dma + 1) % PETLIBRE_AUDIO_PAGES;
    }
    u8 *next = petlibre_audio_zero;
    petlibre_audio_on_zero = !petlibre_audio_own[petlibre_audio_dma];
    if (petlibre_audio_on_zero) {
        petlibre_audio_underruns++;
    } else {
        next = petlibre_audio_ring + petlibre_audio_dma * PETLIBRE_AUDIO_PAGE;
    }
    AUDIO_SP_TXGDMA_Restart(gdma->GDMA_Index, gdma->GDMA_ChNum, (u32)next, PETLIBRE_AUDIO_PAGE);
    return 0;
}

/* Clocks, codec and SPORT as stock sets them for `rate_hz`; the DMA starts
 * on the zero page. 1 when streaming, 0 when open or the rate is unsupported. */
int petlibre_audio_open(unsigned int rate_hz)
{
    u32 rate;
    u32 divider; /* 98.304 MHz down to 256 x rate */
    switch (rate_hz) {
    case 8000:
        rate = SR_8K;
        divider = 48;
        break;
    case 16000:
        rate = SR_16K;
        divider = 24;
        break;
    case 32000:
        rate = SR_32K;
        divider = 12;
        break;
    case 48000:
        rate = SR_48K;
        divider = 8;
        break;
    default:
        return 0;
    }
    if (petlibre_audio_opened) {
        return 0;
    }
    memset(petlibre_audio_zero, 0, sizeof(petlibre_audio_zero));
    for (int i = 0; i < PETLIBRE_AUDIO_PAGES; i++) {
        petlibre_audio_own[i] = 0;
    }
    petlibre_audio_user = 0;
    petlibre_audio_dma = 0;
    petlibre_audio_on_zero = 1;
    petlibre_audio_underruns = 0;

    PLL_I2S_Set(ENABLE); /* 98.304 MHz */
    RCC_PeriphClockCmd(APBPeriph_AUDIOC, APBPeriph_AUDIOC_CLOCK, ENABLE);
    RCC_PeriphClockCmd(APBPeriph_SPORT, APBPeriph_SPORT_CLOCK, ENABLE);
    PLL_Div(divider);
    CODEC_Init(rate, WL_16, CH_MONO, APP_LINE_OUT);

    SP_InitTypeDef sport;
    AUDIO_SP_StructInit(&sport);
    sport.SP_MonoStereo = SP_CH_MONO;
    sport.SP_WordLen = SP_WL_16;
    AUDIO_SP_Init(AUDIO_SPORT_DEV, &sport);
    AUDIO_SP_TdmaCmd(AUDIO_SPORT_DEV, ENABLE);
    AUDIO_SP_TxStart(AUDIO_SPORT_DEV, ENABLE);
    if (!AUDIO_SP_TXGDMA_Init(0, &petlibre_audio_gdma, &petlibre_audio_gdma, petlibre_audio_done,
                              petlibre_audio_zero, PETLIBRE_AUDIO_PAGE)) {
        AUDIO_SP_TxStart(AUDIO_SPORT_DEV, DISABLE);
        AUDIO_SP_TdmaCmd(AUDIO_SPORT_DEV, DISABLE);
        CODEC_DeInit(APP_LINE_OUT);
        return 0;
    }
    petlibre_audio_opened = 1;
    return 1;
}

/* The next page to fill (PETLIBRE_AUDIO_PAGE bytes), or NULL while the DMA
 * owns every page. */
void *petlibre_audio_page(void)
{
    if (!petlibre_audio_opened || petlibre_audio_own[petlibre_audio_user]) {
        return NULL;
    }
    return petlibre_audio_ring + petlibre_audio_user * PETLIBRE_AUDIO_PAGE;
}

/* Hand the page from petlibre_audio_page to the DMA. */
void petlibre_audio_submit(void)
{
    petlibre_audio_own[petlibre_audio_user] = 1;
    petlibre_audio_user = (petlibre_audio_user + 1) % PETLIBRE_AUDIO_PAGES;
}

/* 1 once every submitted page has played. */
int petlibre_audio_drained(void)
{
    for (int i = 0; i < PETLIBRE_AUDIO_PAGES; i++) {
        if (petlibre_audio_own[i]) {
            return 0;
        }
    }
    return 1;
}

unsigned int petlibre_audio_underruns_seen(void)
{
    return petlibre_audio_underruns;
}

/* Stop the DMA, SPORT and codec output; the clocks stay on for the next open. */
void petlibre_audio_close(void)
{
    if (!petlibre_audio_opened) {
        return;
    }
    GDMA_Cmd(petlibre_audio_gdma.GDMA_Index, petlibre_audio_gdma.GDMA_ChNum, DISABLE);
    GDMA_ClearINT(petlibre_audio_gdma.GDMA_Index, petlibre_audio_gdma.GDMA_ChNum);
    GDMA_ChnlFree(petlibre_audio_gdma.GDMA_Index, petlibre_audio_gdma.GDMA_ChNum);
    AUDIO_SP_TxStart(AUDIO_SPORT_DEV, DISABLE);
    AUDIO_SP_TdmaCmd(AUDIO_SPORT_DEV, DISABLE);
    CODEC_DeInit(APP_LINE_OUT);
    petlibre_audio_opened = 0;
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    petlibre_app_start();
}
