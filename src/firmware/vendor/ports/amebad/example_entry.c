/* Hooks called by the vendor's KM4 main; preserve its startup and RTOS setup.
 * The application itself is the Rust staticlib linked into this image.
 *
 * One task joins the configured network and keeps it. It runs
 * LwIP_Init and wifi_on(STA), then repeats: scan for the network (to learn
 * its security), wifi_connect, DHCP; on success it watches the link, every
 * half second asking Rust (`wellfed_wifi_watch`, wellfed-hal link.rs)
 * whether to keep it, given the association, the lease and, when Rust asks
 * for one, an ICMP echo to the lease's gateway. Nothing asks the internet:
 * a network without a route out is kept. When Rust says leave, the task
 * leaves and rejoins with bounded backoff, which resets once Rust reports
 * the join settled. The SDK's static-address DHCP fallback counts as
 * failure and is cleared. Nothing resets or writes flash; the SDK's own
 * reconnect stays unregistered. Events go to a ring the Rust probe drains.
 * The SSID and password come from a generated header built from a private
 * file; neither is ever logged here. Nothing registers fast connect or
 * calls rtw_efuse_boot_write.
 *
 * Blocking TCP helpers for the Rust transport client, and the broker
 * settings from a second generated header. The client and its protocol are
 * Rust (wellfed-core transport); nothing here parses network data.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"
#include "lwip/dhcp.h"
#include "lwip/inet_chksum.h"
#include "lwip/sockets.h"
#include <platform/platform_stdlib.h>
#include "wellfed_wifi_credentials.h"
#include "wellfed_transport_credentials.h"
#include "rl6548.h"

void wellfed_app_start(void);

#define WELLFED_WIFI_EVENTS 16
#define WELLFED_SCAN_WAIT_MS 10000
#define WELLFED_WATCH_MS 500
#define WELLFED_BACKOFF_MIN_MS 1000
#define WELLFED_BACKOFF_MAX_MS 30000
/* One echo per probe; the gateway is one hop away, so an answer later than
 * this is as good as none. */
#define WELLFED_ECHO_TIMEOUT_MS 1000
#define WELLFED_ECHO_ID 0x504c /* "PL" */

/* Event kinds; shared with Rust (`sdk::WifiEvent`). */
enum {
    WELLFED_EV_ON = 1,      /* result: wifi_on; ms: duration */
    WELLFED_EV_SCAN = 2,    /* result: scan; value: found (0/1); rssi, security */
    WELLFED_EV_CONNECT = 3, /* result: wifi_connect; ms */
    WELLFED_EV_DHCP = 4,    /* result: LwIP_DHCP state; value: IPv4 (host order) when assigned; ms */
    WELLFED_EV_LOST = 5,    /* result: the cause (`liveness::Cause`): 0 association, 2 lease, 3 gateway; value: ms connected */
    WELLFED_EV_BACKOFF = 6, /* value: ms until the next attempt */
    /* 7 was the internet DNS check; never reused. */
    WELLFED_EV_GATEWAY = 8, /* result: 0 or a WELLFED_ECHO_* failure; value: the gateway's IPv4 (host order); ms */
};

/* wellfed_wifi_watch's probe argument and answer; shared with Rust
 * (`liveness::Probe::from_code`, `liveness::Verdict::code`). */
enum {
    WELLFED_PROBE_NONE = 0,
    WELLFED_PROBE_ANSWERED = 1,
    WELLFED_PROBE_SILENT = 2,
    WELLFED_PROBE_NO_GATEWAY = 3,
};
enum {
    WELLFED_WATCH_HOLD = 0,
    WELLFED_WATCH_PROBE = 1,
    WELLFED_WATCH_SETTLED = 2,
    WELLFED_WATCH_LEAVE = 0x100, /* plus the cause, the lost event's result */
};
int wellfed_wifi_watch(int associated, int leased, int probe);

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

/* Gateway echo failures, as the event result. */
enum {
    WELLFED_ECHO_NO_GATEWAY = -1,
    WELLFED_ECHO_SOCKET = -2,
    WELLFED_ECHO_SEND = -3,
    WELLFED_ECHO_TIMEOUT = -4,
};

static unsigned int wellfed_ms(TickType_t since)
{
    return (unsigned int)((xTaskGetTickCount() - since) * portTICK_PERIOD_MS);
}

/* Join credentials, shared with Rust (`network::JoinCredentials`):
 * prefilled with the build's, replaced by stored ones unless the fallback
 * has taken over (wellfed-hal network.rs). */
struct wellfed_join_credentials {
    const unsigned char *ssid;
    unsigned int ssid_len;
    const unsigned char *psk;
    unsigned int psk_len;
    unsigned int source; /* 0 build, 1 stored, 2 fallback */
};
void wellfed_wifi_select(struct wellfed_join_credentials *out);
void wellfed_wifi_outcome(int joined);
static struct wellfed_join_credentials wellfed_creds;

static rtw_result_t wellfed_scan_result(rtw_scan_handler_result_t *result)
{
    if (result->scan_complete == RTW_TRUE) {
        wellfed_scan_done = 1;
        return RTW_SUCCESS;
    }
    rtw_scan_result_t *ap = &result->ap_details;
    unsigned int length = wellfed_creds.ssid_len;
    if (ap->SSID.len == length && memcmp(ap->SSID.val, wellfed_creds.ssid, length) == 0 &&
        (!wellfed_found || ap->signal_strength > wellfed_found_rssi)) {
        wellfed_found_rssi = ap->signal_strength;
        wellfed_found_security = (unsigned int)ap->security;
        wellfed_found = 1;
    }
    return RTW_SUCCESS;
}

/* One join attempt: scan, connect, DHCP. 1 when an address was assigned. */
static int wellfed_join_once(unsigned int attempt)
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
    result = wifi_connect((char *)wellfed_creds.ssid, (rtw_security_t)wellfed_found_security,
                          (char *)wellfed_creds.psk, (int)wellfed_creds.ssid_len, (int)wellfed_creds.psk_len,
                          -1, NULL);
    /* The event's value names the credentials' source, never the credentials. */
    wellfed_event(WELLFED_EV_CONNECT, attempt, result, wellfed_creds.source, wellfed_ms(begin));
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

/* One attempt with the credentials Rust selects, reporting the outcome. */
static int wellfed_join(unsigned int attempt)
{
    wellfed_creds.ssid = (const unsigned char *)WELLFED_WIFI_SSID;
    wellfed_creds.ssid_len = sizeof(WELLFED_WIFI_SSID) - 1;
    wellfed_creds.psk = (const unsigned char *)WELLFED_WIFI_PASSWORD;
    wellfed_creds.psk_len = sizeof(WELLFED_WIFI_PASSWORD) - 1;
    wellfed_creds.source = 0;
    wellfed_wifi_select(&wellfed_creds);
    int joined = wellfed_join_once(attempt);
    wellfed_wifi_outcome(joined);
    return joined;
}

static unsigned int wellfed_be32(const uint8_t *a)
{
    return ((unsigned int)a[0] << 24) | ((unsigned int)a[1] << 16) | ((unsigned int)a[2] << 8) | a[3];
}

unsigned int wellfed_ipv4(void);

/* The lease holds: lwIP's DHCP client is bound, renewing or rebinding. It
 * gives the address up only once the lease ran out with renew and rebind
 * unanswered. */
static int wellfed_leased(void)
{
    return wellfed_ipv4() != 0;
}

/* One ICMP echo to `gateway` (host order): 0 when it answered, or a
 * WELLFED_ECHO_* failure. A raw socket sees every ICMP datagram to this
 * host and lwIP still answers echo requests itself, so the reply is matched
 * by source, type, identifier and sequence. */
static int wellfed_echo(unsigned int gateway, uint16_t seq)
{
    uint8_t m[64];
    memset(m, 0, 16);
    m[0] = 8; /* echo request */
    m[4] = WELLFED_ECHO_ID >> 8;
    m[5] = WELLFED_ECHO_ID & 0xff;
    m[6] = seq >> 8;
    m[7] = seq & 0xff;
    memcpy(m + 8, "wellfed", 8);
    uint16_t sum = inet_chksum(m, 16);
    memcpy(m + 2, &sum, 2); /* already in network order */

    int s = lwip_socket(AF_INET, SOCK_RAW, IPPROTO_ICMP);
    if (s < 0) {
        return WELLFED_ECHO_SOCKET;
    }
    struct sockaddr_in to;
    memset(&to, 0, sizeof(to));
    to.sin_family = AF_INET;
    to.sin_addr.s_addr = lwip_htonl(gateway);
    int status = WELLFED_ECHO_SEND;
    if (lwip_sendto(s, m, 16, 0, (struct sockaddr *)&to, sizeof(to)) == 16) {
        status = WELLFED_ECHO_TIMEOUT;
        TickType_t sent = xTaskGetTickCount();
        /* Skip other ICMP traffic, waiting at most the timeout overall. */
        unsigned int waited;
        while ((waited = wellfed_ms(sent)) < WELLFED_ECHO_TIMEOUT_MS) {
            int left = (int)(WELLFED_ECHO_TIMEOUT_MS - waited);
            lwip_setsockopt(s, SOL_SOCKET, SO_RCVTIMEO, &left, sizeof(left));
            int got = lwip_recv(s, m, sizeof(m), 0);
            if (got < 20) {
                if (got < 0) {
                    break;
                }
                continue;
            }
            int at = (m[0] & 0x0f) * 4; /* the IPv4 header's length */
            if (at < 20 || got < at + 8 || wellfed_be32(m + 12) != gateway) {
                continue;
            }
            if (m[at] == 0 && m[at + 4] == (WELLFED_ECHO_ID >> 8) && m[at + 5] == (WELLFED_ECHO_ID & 0xff) &&
                m[at + 6] == (seq >> 8) && m[at + 7] == (seq & 0xff)) {
                status = 0;
                break;
            }
        }
    }
    lwip_close(s);
    return status;
}

/* One probe of the lease's gateway, as wellfed_wifi_watch's probe code. */
static int wellfed_gateway(unsigned int attempt)
{
    static uint16_t seq;
    unsigned int gateway = wellfed_be32(LwIP_GetGW(&xnetif[0]));
    TickType_t begin = xTaskGetTickCount();
    int result = gateway != 0 ? wellfed_echo(gateway, ++seq) : WELLFED_ECHO_NO_GATEWAY;
    wellfed_event(WELLFED_EV_GATEWAY, attempt, result, gateway, wellfed_ms(begin));
    if (result == WELLFED_ECHO_NO_GATEWAY) {
        return WELLFED_PROBE_NO_GATEWAY;
    }
    return result == 0 ? WELLFED_PROBE_ANSWERED : WELLFED_PROBE_SILENT;
}

/* Watch a joined link until Rust says leave; the lost event's cause. */
static int wellfed_watch(unsigned int attempt, unsigned int *backoff)
{
    int probe = WELLFED_PROBE_NONE;
    for (;;) {
        int verdict = wellfed_wifi_watch(wifi_is_connected_to_ap() == RTW_SUCCESS, wellfed_leased(), probe);
        probe = WELLFED_PROBE_NONE;
        if (verdict >= WELLFED_WATCH_LEAVE) {
            return verdict - WELLFED_WATCH_LEAVE;
        }
        if (verdict == WELLFED_WATCH_PROBE) {
            probe = wellfed_gateway(attempt);
            continue;
        }
        if (verdict == WELLFED_WATCH_SETTLED) {
            *backoff = WELLFED_BACKOFF_MIN_MS;
        }
        vTaskDelay(WELLFED_WATCH_MS / portTICK_PERIOD_MS);
    }
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
            int cause = wellfed_watch(attempt, &backoff);
            wellfed_event(WELLFED_EV_LOST, attempt, cause, wellfed_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
            /* Still associated: leave, so the next attempt joins afresh. */
            if (cause != 0) {
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
    const char *request_key; /* 64 lowercase hex characters, or "" */
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
    out->request_key = WELLFED_MQTT_REQUEST_KEY;
}

u32 Rand(void); /* ROM (swlib/string/rand.h): a seeded software generator */

/* Inputs for a session's request nonce (wellfed-core request_auth): the ROM
 * generator, the CPU-rate SysTick count and the tick count at the moment of a
 * connect, whose timing varies with the network. Not a vetted RNG; the nonce
 * only has to differ between sessions. */
void wellfed_entropy(unsigned int out[4])
{
    out[0] = Rand();
    out[1] = *(volatile unsigned int *)0xE000E018; /* SysTick current value */
    out[2] = xTaskGetTickCount();
    out[3] = Rand();
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

/* Whether `s` is ready now: readable (data, a waiting connection, or a
 * closed or failed socket), or with `write`, room to send. A zero-timeout
 * select scans the socket under lwIP's lock and posts nothing, where
 * `getsockopt` waits on a round trip to the stack's thread; every idle
 * poll of the loop used to pay one. A failed select reports ready, so the
 * call that follows meets the error. */
static int wellfed_ready(int s, int write)
{
    fd_set ready;
    fd_set failed;
    FD_ZERO(&ready);
    FD_ZERO(&failed);
    FD_SET(s, &ready);
    FD_SET(s, &failed);
    struct timeval now = {0, 0};
    return lwip_select(s + 1, write ? NULL : &ready, write ? &ready : NULL, &failed, &now) != 0;
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

/* The speaker variant's listener (wellfed-hal stream.rs): a socket on
 * `port` for every local address, or -1. */
int wellfed_tcp_listen(unsigned int port)
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
int wellfed_tcp_accept(int listener, unsigned int timeout_ms)
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
 * call takes what is ready now and returns (wellfed-hal api.rs). */
static void wellfed_nonblocking(int s)
{
    int flags = lwip_fcntl(s, F_GETFL, 0);
    lwip_fcntl(s, F_SETFL, flags | O_NONBLOCK);
}

/* A listener on `port` for every local address, or -1. */
int wellfed_api_listen(unsigned int port)
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
    wellfed_nonblocking(s);
    return s;
}

/* A waiting connection: a socket, -1 when none waits, -2 on an error. */
int wellfed_api_accept(int listener)
{
    if (!wellfed_ready(listener, 0)) {
        return -1;
    }
    int s = lwip_accept(listener, NULL, NULL);
    if (s < 0) {
        int error = wellfed_socket_error(listener);
        return (error == 0 || error == EAGAIN || error == EWOULDBLOCK) ? -1 : -2;
    }
    wellfed_nonblocking(s);
    int on = 1;
    lwip_setsockopt(s, IPPROTO_TCP, TCP_NODELAY, &on, sizeof(on));
    return s;
}

/* Bytes received now; 0 when none are waiting, -1 once closed or failed. */
int wellfed_api_recv(int s, void *buffer, unsigned int length)
{
    if (!wellfed_ready(s, 0)) {
        return 0;
    }
    int got = lwip_recv(s, buffer, length, MSG_DONTWAIT);
    if (got > 0) {
        return got;
    }
    if (got < 0) {
        int error = wellfed_socket_error(s);
        if (error == 0 || error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

/* Bytes the stack took now (0 while its send buffer is full), -1 on error. */
int wellfed_api_send(int s, const void *data, unsigned int length)
{
    if (!wellfed_ready(s, 1)) {
        return 0;
    }
    int sent = lwip_send(s, data, length, MSG_DONTWAIT);
    if (sent >= 0) {
        return sent;
    }
    int error = wellfed_socket_error(s);
    return (error == 0 || error == EAGAIN || error == EWOULDBLOCK) ? 0 : -1;
}

/* The mDNS socket: UDP 5353, a member of 224.0.0.251, or -1. Opened only
 * once DHCP gave an address: a socket opened on the static fallback never
 * saw a multicast query (physical, fw47-api-02). */
int wellfed_mdns_open(void)
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
    wellfed_nonblocking(s);
    return s;
}

/* One datagram received now, with its sender; 0 when none, -1 on error. */
int wellfed_mdns_recv(int s, void *buffer, unsigned int length, unsigned int *ip, unsigned int *port)
{
    if (!wellfed_ready(s, 0)) {
        return 0;
    }
    struct sockaddr_in from;
    socklen_t size = sizeof(from);
    int got = lwip_recvfrom(s, buffer, length, MSG_DONTWAIT, (struct sockaddr *)&from, &size);
    if (got > 0) {
        *ip = lwip_ntohl(from.sin_addr.s_addr);
        *port = lwip_ntohs(from.sin_port);
        return got;
    }
    if (got < 0) {
        int error = wellfed_socket_error(s);
        if (error == 0 || error == EAGAIN || error == EWOULDBLOCK) {
            return 0;
        }
    }
    return -1;
}

/* Send one datagram to `ip`:`port`: 0, or -1. */
int wellfed_mdns_send(int s, const void *data, unsigned int length, unsigned int ip, unsigned int port)
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
unsigned int wellfed_ipv4(void)
{
    if (!dhcp_supplied_address(&xnetif[0])) {
        return 0;
    }
    uint8_t *a = LwIP_GetIP(&xnetif[0]);
    return ((unsigned int)a[0] << 24) | ((unsigned int)a[1] << 16) | ((unsigned int)a[2] << 8) | a[3];
}

/* The speaker. Stock 1.1.15 plays through the codec's line output
 * (CODEC_Init application 8) from SPORT by GDMA and holds PB23 HIGH while it
 * plays, the amplifier enable that Rust drives (wellfed-hal audio.rs).
 * This shim owns only the codec, SPORT and an 8-page DMA ring of 16-bit mono
 * at 8, 16, 32 or 48 kHz (the rates the I2S PLL divides to). Rust fills pages and decides when to open and close. When no
 * filled page is ready the DMA plays a zero page and counts an underrun, so
 * the stream never stops on its own. */
/* Defined in the pinned fwlib (rtl8721d_pll.c, rtl8721dhp_audio.c) with no
 * header declaration. */
void PLL_Div(u32 div);
BOOL AUDIO_SP_TXGDMA_Restart(u8 GDMA_Index, u8 GDMA_ChNum, u32 tx_addr, u32 tx_length);

#define WELLFED_AUDIO_PAGE 512 /* bytes: 256 samples, 16 ms at 16 kHz */
#define WELLFED_AUDIO_PAGES 8

/* Multiples of 32 bytes at 32-byte alignment: the restart cleans the cache
 * over each page the DMA reads. */
static u8 wellfed_audio_ring[WELLFED_AUDIO_PAGE * WELLFED_AUDIO_PAGES] __attribute__((aligned(32)));
static u8 wellfed_audio_zero[WELLFED_AUDIO_PAGE] __attribute__((aligned(32)));
/* Filled pages the DMA owns; set by Rust's task, cleared by the DMA interrupt. */
static volatile u8 wellfed_audio_own[WELLFED_AUDIO_PAGES];
static unsigned int wellfed_audio_user;
static volatile unsigned int wellfed_audio_dma;
static volatile unsigned int wellfed_audio_on_zero;
static volatile unsigned int wellfed_audio_underruns;
static GDMA_InitTypeDef wellfed_audio_gdma;
static int wellfed_audio_opened;

static u32 wellfed_audio_done(void *data)
{
    GDMA_InitTypeDef *gdma = data;
    GDMA_ClearINT(gdma->GDMA_Index, gdma->GDMA_ChNum);
    if (!wellfed_audio_on_zero) {
        wellfed_audio_own[wellfed_audio_dma] = 0;
        wellfed_audio_dma = (wellfed_audio_dma + 1) % WELLFED_AUDIO_PAGES;
    }
    u8 *next = wellfed_audio_zero;
    wellfed_audio_on_zero = !wellfed_audio_own[wellfed_audio_dma];
    if (wellfed_audio_on_zero) {
        wellfed_audio_underruns++;
    } else {
        next = wellfed_audio_ring + wellfed_audio_dma * WELLFED_AUDIO_PAGE;
    }
    AUDIO_SP_TXGDMA_Restart(gdma->GDMA_Index, gdma->GDMA_ChNum, (u32)next, WELLFED_AUDIO_PAGE);
    return 0;
}

/* Clocks, codec and SPORT as stock sets them for `rate_hz`; the DMA starts
 * on the zero page. 1 when streaming, 0 when open or the rate is unsupported. */
int wellfed_audio_open(unsigned int rate_hz)
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
    if (wellfed_audio_opened) {
        return 0;
    }
    memset(wellfed_audio_zero, 0, sizeof(wellfed_audio_zero));
    for (int i = 0; i < WELLFED_AUDIO_PAGES; i++) {
        wellfed_audio_own[i] = 0;
    }
    wellfed_audio_user = 0;
    wellfed_audio_dma = 0;
    wellfed_audio_on_zero = 1;
    wellfed_audio_underruns = 0;

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
    if (!AUDIO_SP_TXGDMA_Init(0, &wellfed_audio_gdma, &wellfed_audio_gdma, wellfed_audio_done,
                              wellfed_audio_zero, WELLFED_AUDIO_PAGE)) {
        AUDIO_SP_TxStart(AUDIO_SPORT_DEV, DISABLE);
        AUDIO_SP_TdmaCmd(AUDIO_SPORT_DEV, DISABLE);
        CODEC_DeInit(APP_LINE_OUT);
        return 0;
    }
    wellfed_audio_opened = 1;
    return 1;
}

/* The next page to fill (WELLFED_AUDIO_PAGE bytes), or NULL while the DMA
 * owns every page. */
void *wellfed_audio_page(void)
{
    if (!wellfed_audio_opened || wellfed_audio_own[wellfed_audio_user]) {
        return NULL;
    }
    return wellfed_audio_ring + wellfed_audio_user * WELLFED_AUDIO_PAGE;
}

/* Hand the page from wellfed_audio_page to the DMA. */
void wellfed_audio_submit(void)
{
    wellfed_audio_own[wellfed_audio_user] = 1;
    wellfed_audio_user = (wellfed_audio_user + 1) % WELLFED_AUDIO_PAGES;
}

/* 1 once every submitted page has played. */
int wellfed_audio_drained(void)
{
    for (int i = 0; i < WELLFED_AUDIO_PAGES; i++) {
        if (wellfed_audio_own[i]) {
            return 0;
        }
    }
    return 1;
}

unsigned int wellfed_audio_underruns_seen(void)
{
    return wellfed_audio_underruns;
}

/* Stop the DMA, SPORT and codec output; the clocks stay on for the next open. */
void wellfed_audio_close(void)
{
    if (!wellfed_audio_opened) {
        return;
    }
    GDMA_Cmd(wellfed_audio_gdma.GDMA_Index, wellfed_audio_gdma.GDMA_ChNum, DISABLE);
    GDMA_ClearINT(wellfed_audio_gdma.GDMA_Index, wellfed_audio_gdma.GDMA_ChNum);
    GDMA_ChnlFree(wellfed_audio_gdma.GDMA_Index, wellfed_audio_gdma.GDMA_ChNum);
    AUDIO_SP_TxStart(AUDIO_SPORT_DEV, DISABLE);
    AUDIO_SP_TdmaCmd(AUDIO_SPORT_DEV, DISABLE);
    CODEC_DeInit(APP_LINE_OUT);
    wellfed_audio_opened = 0;
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    wellfed_app_start();
}
