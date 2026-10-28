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
 * Before an intentional restart Rust asks the task to leave for good
 * (`wellfed_wifi_restarting`, wellfed-hal link.rs): it disconnects, so the
 * access point drops the protected association instead of rejecting the
 * next boot's with status 30, and never joins again.
 * Battery mode asks for the radio off (`wellfed_wifi_radio`, wellfed-hal
 * link.rs): the task leaves the same way, turns the radio off (wifi_off)
 * and waits until Rust asks for it on again, then turns it on (wifi_on, as
 * at boot) and joins afresh. Between attempts and inside one it checks the
 * wish too, so it neither scans nor associates while the radio should be
 * off. A restart wins over both.
 * The SSID and password come from a generated header built from a private
 * file; neither is ever logged here. Nothing registers fast connect or
 * calls rtw_efuse_boot_write.
 *
 * Non-blocking socket helpers for the Rust device API; its protocol is Rust
 * (wellfed-feeder), and nothing here parses network data.
 */
#include "FreeRTOS.h"
#include "task.h"
#include "wifi_conf.h"
#include "wifi_constants.h"
#include "lwip_netconf.h"
#include "lwip/dhcp.h"
#include "lwip/prot/dhcp.h"
#include "lwip/inet_chksum.h"
#include "lwip/sockets.h"
#include <platform/platform_stdlib.h>
#include "wellfed_wifi_credentials.h"
#include "rl6548.h"
#include <stddef.h>

void wellfed_app_start(void);

/* The call trail: the last calls into a few vendor primitives, kept where
 * a watchdog reset leaves the RAM alone, so the boot after a silent hang
 * can say where it stalled. The tick hook's crumb names the task; the trail
 * names the calls. Rust decodes and prints it (wellfed-hal
 * supervision/trail.rs, which shares the layout and step codes below).
 *
 * The builder links the KM4 image with --wrap for each primitive wrapped
 * here, so a call to it from any other object, the prebuilt Wi-Fi driver's
 * included, lands in __wrap_NAME, which notes the step and calls
 * __real_NAME. A call inside the object that defines the primitive is not
 * redirected: rtk_wlan_if.o's internal timer calls (rltk_set_mode_prehandle
 * to cancel_timer_ex, rltk_set_mode_posthandle to mod_timer) and
 * freertos_intfs.o's own call to rtw_os_indicate_disconnect go unnoted.
 *
 * The driver masks interrupts with save_and_cli around every packet buffer
 * it allocates or frees, so the ring turns over many times during a join.
 * The rarer steps therefore also go to sticky marks, one slot per kind,
 * which that traffic never evicts: the last wifi_connect call and return,
 * indication, disconnect and timer call.
 *
 * rtw_enter_critical and rtw_exit_critical are deliberately not wrapped.
 * They guard every driver register access (bus_read and bus_write), the
 * WLAN interrupt and the receive tasklet, and a note on that path stretched
 * the first WPA3 SAE join's stall past the watchdog feeder's 1 s gap, so
 * every trial boot of that build was stopped by the supervisor.
 *
 * A note masks nothing on the ring's path: it claims its ring slot by
 * incrementing the count with LDREX/STREX (which an interrupt between the
 * two makes retry), then writes the slot with plain stores, meta last. A
 * note from an interrupt that lands mid-note claims the next slot, so the
 * two never share one. A reset between the claim and meta's store leaves
 * the slot with the number it held a ring earlier, which the reader drops.
 * A mark is rewritten with interrupts masked (PRIMASK saved and restored),
 * so neither an interrupt nor a higher-priority task can mix two notes in
 * it; that short section is only on the rare marked steps, never on a
 * critical section's own note. A mark's meta is cleared first and its
 * number stored last, so a reset mid-mark leaves a mark the reader drops.
 * A note preempted between its claim and its mark finds a newer note there
 * and leaves it, so a mark always holds its kind's latest note.
 * Only the critical-section wrappers change the depth, one counter for the
 * whole CPU, and they do it with interrupts already masked (PRIMASK, in a task
 * or an interrupt). A note never blocks, allocates or prints, and is safe
 * from any task or interrupt. The wrappers run from RAM, as the vendor's
 * save_and_cli and the driver's timer functions do.
 * Nothing is noted until Rust's start has read the last boot's trail and
 * started this one's (wellfed_trail_start); the switch is in .bss, which
 * app_start clears before anything runs, so the SDK's own startup cannot
 * overwrite the trail before it is read. The header carries the KM4 boot
 * count the start was given, so the reader takes only the boot just before,
 * and the check folds in the layout's version, so a trail of another layout
 * is refused.
 *
 * The trail sits just below the time carry, at 0x1007BC50, taken from the
 * heap's end by the builder's linker script (its size is the builder's
 * TRAIL_BYTES); the time carry's comment says why a warm reset leaves that
 * RAM alone. Each store reaches the RAM before a reset: the ROM's mpu_init
 * enables the MPU with the default map as background (PRIVDEFENA) and no
 * region, app_mpu_nocache_init's regions miss this address, and the default
 * map makes it Normal write-through, read-allocate, as the SDK's cache
 * header says. */
#define WELLFED_TRAIL_ENTRIES 64
#define WELLFED_TRAIL_MARKS 5
#define WELLFED_TRAIL_WORDS 220
#define WELLFED_TRAIL_MAGIC 0x504c5452u /* "PLTR" */
#define WELLFED_TRAIL_VERSION 3u
#define WELLFED_TRAIL_DEPTH_SHIFT 8
#define WELLFED_TRAIL_ISR_SHIFT 15
#define WELLFED_TRAIL_SEQ_SHIFT 16

/* Step codes, shared with Rust (`trail::Step`). */
enum {
    WELLFED_STEP_SAVE_AND_CLI = 1,
    WELLFED_STEP_RESTORE_FLAGS = 2,
    WELLFED_STEP_SET_TIMER = 3,
    WELLFED_STEP_DEL_TIMER = 4,
    WELLFED_STEP_CANCEL_TIMER = 5,
    WELLFED_STEP_INDICATION = 6,
    WELLFED_STEP_INDICATION_RETURNED = 7,
    WELLFED_STEP_INDICATE_DISCONNECT = 8,
    WELLFED_STEP_INDICATE_DISCONNECT_RETURNED = 9,
    WELLFED_STEP_CONNECT = 10,
    WELLFED_STEP_CONNECT_RETURNED = 11,
};

/* Mark slots, shared with Rust (`trail::Mark`); none for critical sections. */
enum {
    WELLFED_MARK_NONE = -1,
    WELLFED_MARK_CONNECT = 0,
    WELLFED_MARK_CONNECT_RETURNED = 1,
    WELLFED_MARK_INDICATION = 2,
    WELLFED_MARK_DISCONNECT = 3,
    WELLFED_MARK_TIMER = 4,
};

/* One ring note. meta: the step in bits 0-7, the wrapped critical sections
 * open (this one included, at most 127) in 8-14, an interrupt context in 15
 * and the note's number's low 16 bits in 16-31, stored after lr and arg so
 * a slot whose number matches holds a whole note. lr is the wrapped call's
 * return address (0 for the shim's own steps); arg is the step's first
 * argument or result. */
struct wellfed_trail_entry {
    unsigned int meta;
    unsigned int lr;
    unsigned int arg;
};

/* A mark: the same note with its whole number, stored last. */
struct wellfed_trail_mark {
    unsigned int seq;
    unsigned int meta;
    unsigned int lr;
    unsigned int arg;
};

struct wellfed_trail {
    unsigned int magic;
    unsigned int boot;  /* the KM4 boot count the start was given */
    unsigned int check; /* ~(magic ^ boot ^ version) */
    unsigned int count; /* notes since the trail started */
    unsigned int depth; /* wrapped critical sections open now, CPU-wide */
    unsigned int reserved[3];
    struct wellfed_trail_mark marks[WELLFED_TRAIL_MARKS];
    struct wellfed_trail_entry entries[WELLFED_TRAIL_ENTRIES];
};

/* The word offsets Rust reads (trail.rs `MARKS_AT`, `ENTRIES_AT`). */
_Static_assert(sizeof(struct wellfed_trail) == WELLFED_TRAIL_WORDS * sizeof(unsigned int),
               "the trail's words must match the builder's TRAIL_BYTES and Rust's TRAIL_WORDS");
_Static_assert(offsetof(struct wellfed_trail, marks) == 8 * sizeof(unsigned int), "marks at word 8");
_Static_assert(offsetof(struct wellfed_trail, entries) == 28 * sizeof(unsigned int), "entries at word 28");
_Static_assert(offsetof(struct wellfed_trail_mark, seq) == 0, "a mark's number is its first word");
_Static_assert(offsetof(struct wellfed_trail_mark, meta) == sizeof(unsigned int), "then the note's meta");

extern volatile struct wellfed_trail wellfed_status_trail;
static volatile unsigned int wellfed_trail_on;

/* change: +1 for a critical section's entry (counted before the note), -1
 * for its exit (counted after), 0 for anything else; nonzero only with
 * interrupts masked. mark: the step's mark slot, or WELLFED_MARK_NONE. */
IMAGE2_RAM_TEXT_SECTION
static void wellfed_trail_note(unsigned int step, int change, int mark, unsigned int lr, unsigned int arg)
{
    if (!wellfed_trail_on) {
        return;
    }
    volatile struct wellfed_trail *t = &wellfed_status_trail;
    unsigned int depth = t->depth;
    if (change > 0) {
        depth++;
        t->depth = depth;
    }

    unsigned int n = __atomic_fetch_add(&t->count, 1u, __ATOMIC_RELAXED);
    unsigned int meta = step | ((depth > 127u ? 127u : depth) << WELLFED_TRAIL_DEPTH_SHIFT) |
                        ((__get_IPSR() != 0u ? 1u : 0u) << WELLFED_TRAIL_ISR_SHIFT) |
                        (n << WELLFED_TRAIL_SEQ_SHIFT);
    volatile struct wellfed_trail_entry *e = &t->entries[n % WELLFED_TRAIL_ENTRIES];
    e->lr = lr;
    e->arg = arg;
    __asm volatile("" ::: "memory");
    e->meta = meta;
    if (mark >= 0) {
        unsigned int primask = __get_PRIMASK();
        __disable_irq();
        volatile struct wellfed_trail_mark *m = &t->marks[mark];
        /* Only a note preempted between its claim and here can find a newer one
         * already marked, a few notes ahead at most; anything else is older,
         * however far the count has run since. */
        if (m->meta == 0u || (m->seq - n) > 4096u) {
            m->meta = 0;
            m->lr = lr;
            m->arg = arg;
            m->meta = meta;
            m->seq = n;
        }
        __set_PRIMASK(primask);
    }

    if (change < 0 && depth > 0u) {
        t->depth = depth - 1u;
    }
}

/* One word of the last boot's trail, as the RAM holds it, read in place
 * before wellfed_trail_start: what a power-on left, or a build without the
 * trail, is refused by the reader's magic and check. 0 past the end. */
unsigned int wellfed_trail_word(unsigned int index)
{
    volatile unsigned int *words = (volatile unsigned int *)&wellfed_status_trail;
    return index < WELLFED_TRAIL_WORDS ? words[index] : 0u;
}

/* An empty trail for the boot numbered boot, then noting from here on. */
void wellfed_trail_start(unsigned int boot)
{
    wellfed_trail_on = 0;
    volatile unsigned int *words = (volatile unsigned int *)&wellfed_status_trail;
    for (int i = 0; i < WELLFED_TRAIL_WORDS; i++) {
        words[i] = 0;
    }
    wellfed_status_trail.magic = WELLFED_TRAIL_MAGIC;
    wellfed_status_trail.boot = boot;
    wellfed_status_trail.check = ~(WELLFED_TRAIL_MAGIC ^ boot ^ WELLFED_TRAIL_VERSION);
    DCache_Clean((u32)&wellfed_status_trail, sizeof(struct wellfed_trail));
    wellfed_trail_on = 1;
}

#define WELLFED_LR() ((unsigned int)__builtin_return_address(0))

void __real_save_and_cli(void);
void __real_restore_flags(void);
void __real_rtw_set_timer(void *ptimer, u32 delay_time);
void __real_rtw_del_timer(void *ptimer);
u8 __real_rtw_cancel_timer(void *ptimer);
void __real_wifi_indication(rtw_event_indicate_t event, char *buf, int buf_len, int flags);
void __real_rtw_os_indicate_disconnect(void *adapter);
void __wrap_save_and_cli(void);
void __wrap_restore_flags(void);
void __wrap_rtw_set_timer(void *ptimer, u32 delay_time);
void __wrap_rtw_del_timer(void *ptimer);
u8 __wrap_rtw_cancel_timer(void *ptimer);
void __wrap_wifi_indication(rtw_event_indicate_t event, char *buf, int buf_len, int flags);
void __wrap_rtw_os_indicate_disconnect(void *adapter);

/* A critical section's entry is noted once it is masked and its exit
 * before it unmasks, so a stall inside one ends the trail at its entry and
 * the depth only ever changes with interrupts masked. */
IMAGE2_RAM_TEXT_SECTION
void __wrap_save_and_cli(void)
{
    __real_save_and_cli();
    wellfed_trail_note(WELLFED_STEP_SAVE_AND_CLI, 1, WELLFED_MARK_NONE, WELLFED_LR(), 0);
}

IMAGE2_RAM_TEXT_SECTION
void __wrap_restore_flags(void)
{
    wellfed_trail_note(WELLFED_STEP_RESTORE_FLAGS, -1, WELLFED_MARK_NONE, WELLFED_LR(), 0);
    __real_restore_flags();
}

/* Each timer call is noted before it runs: the driver's timer list is
 * walked inside it, under save_and_cli. */
IMAGE2_RAM_TEXT_SECTION
void __wrap_rtw_set_timer(void *ptimer, u32 delay_time)
{
    wellfed_trail_note(WELLFED_STEP_SET_TIMER, 0, WELLFED_MARK_TIMER, WELLFED_LR(), (unsigned int)ptimer);
    __real_rtw_set_timer(ptimer, delay_time);
}

IMAGE2_RAM_TEXT_SECTION
void __wrap_rtw_del_timer(void *ptimer)
{
    wellfed_trail_note(WELLFED_STEP_DEL_TIMER, 0, WELLFED_MARK_TIMER, WELLFED_LR(), (unsigned int)ptimer);
    __real_rtw_del_timer(ptimer);
}

IMAGE2_RAM_TEXT_SECTION
u8 __wrap_rtw_cancel_timer(void *ptimer)
{
    wellfed_trail_note(WELLFED_STEP_CANCEL_TIMER, 0, WELLFED_MARK_TIMER, WELLFED_LR(), (unsigned int)ptimer);
    return __real_rtw_cancel_timer(ptimer);
}

/* The indications bracket the disconnect handler, which wakes wifi_connect,
 * and the driver's own teardown after it. */
IMAGE2_RAM_TEXT_SECTION
void __wrap_wifi_indication(rtw_event_indicate_t event, char *buf, int buf_len, int flags)
{
    unsigned int lr = WELLFED_LR();
    wellfed_trail_note(WELLFED_STEP_INDICATION, 0, WELLFED_MARK_INDICATION, lr, (unsigned int)event);
    __real_wifi_indication(event, buf, buf_len, flags);
    wellfed_trail_note(WELLFED_STEP_INDICATION_RETURNED, 0, WELLFED_MARK_INDICATION, lr, (unsigned int)event);
}

IMAGE2_RAM_TEXT_SECTION
void __wrap_rtw_os_indicate_disconnect(void *adapter)
{
    unsigned int lr = WELLFED_LR();
    wellfed_trail_note(WELLFED_STEP_INDICATE_DISCONNECT, 0, WELLFED_MARK_DISCONNECT, lr, (unsigned int)adapter);
    __real_rtw_os_indicate_disconnect(adapter);
    wellfed_trail_note(WELLFED_STEP_INDICATE_DISCONNECT_RETURNED, 0, WELLFED_MARK_DISCONNECT, lr, (unsigned int)adapter);
}

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
    WELLFED_EV_LEAVE = 9,   /* result: wifi_disconnect; value: 1 still associated at the bound; ms */
    WELLFED_EV_AP = 10,     /* after an address: result: the channel (1-14, 36-177), or -1; value: the BSSID's last 4 bytes */
    WELLFED_EV_RENEW = 11,  /* lwIP's DHCP client took a new ACK without a rejoin (renew or rebind) */
    WELLFED_EV_OFF = 12,    /* battery mode: result: wifi_off; ms: duration */
};

/* The lost event's cause Rust answers before an intentional restart
 * (`liveness::Cause::Restart`). */
#define WELLFED_CAUSE_RESTART 4
/* The lost event's cause Rust answers while battery mode wants the radio
 * off (`liveness::Cause::Radio`). */
#define WELLFED_CAUSE_RADIO 5
int wellfed_wifi_restarting(void);
/* 1 while the radio should be up, 0 while battery mode wants it off. */
int wellfed_wifi_radio(void);
/* How long the radio's return waits before trying wifi_on again. */
#define WELLFED_RADIO_RETRY_MS 5000
/* How long a leave waits for the driver to report the association gone:
 * inside the watchdog's shortest expiry (2.4 s at its clock's bound) with a gateway probe's
 * second before it. */
#define WELLFED_LEAVE_WAIT_MS 1000
#define WELLFED_POLL_MS 50

/* wellfed_wifi_watch's probe argument and answer; shared with Rust
 * (`liveness::Probe::from_code`, `liveness::Verdict::code`). */
enum {
    WELLFED_PROBE_NONE = 0,
    WELLFED_PROBE_ANSWERED = 1,
    WELLFED_PROBE_SILENT = 2,
    WELLFED_PROBE_NO_GATEWAY = 3,
    WELLFED_PROBE_UNSENT = 4,
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

/* Leave the network cleanly: before an intentional restart (then never
 * join again) or before battery mode turns the radio off. The disconnect
 * sends the access point a (protected) deauthentication from the driver's
 * command thread; wait, bounded, until the driver reports the association
 * gone, so it has left before the reset or the radio goes. Before a
 * restart the watchdog feeder has already stopped: if anything here hangs,
 * the reset comes all the same. */
static void wellfed_leave(unsigned int attempt)
{
    TickType_t begin = xTaskGetTickCount();
    LwIP_DHCP(0, DHCP_STOP);
    LwIP_ReleaseIP(0);
    int result = wifi_disconnect();
    while (wifi_is_connected_to_ap() == RTW_SUCCESS && wellfed_ms(begin) < WELLFED_LEAVE_WAIT_MS) {
        vTaskDelay(WELLFED_POLL_MS / portTICK_PERIOD_MS);
    }
    wellfed_event(WELLFED_EV_LEAVE, attempt, result, wifi_is_connected_to_ap() == RTW_SUCCESS,
                   wellfed_ms(begin));
}

static void wellfed_access_point(unsigned int attempt);

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
    /* A restart asked for during the scan (up to 10 s): no new association,
     * which the access point would hold against the next boot. Nor one
     * battery mode would only leave again. */
    if (result != RTW_SUCCESS || !wellfed_found || wellfed_wifi_restarting() || !wellfed_wifi_radio()) {
        return 0;
    }
    begin = xTaskGetTickCount();
    /* The trail shows whether wifi_connect returned before a stall. */
    wellfed_trail_note(WELLFED_STEP_CONNECT, 0, WELLFED_MARK_CONNECT, 0, attempt);
    result = wifi_connect((char *)wellfed_creds.ssid, (rtw_security_t)wellfed_found_security,
                          (char *)wellfed_creds.psk, (int)wellfed_creds.ssid_len, (int)wellfed_creds.psk_len,
                          -1, NULL);
    wellfed_trail_note(WELLFED_STEP_CONNECT_RETURNED, 0, WELLFED_MARK_CONNECT_RETURNED, 0, (unsigned int)result);
    /* The event's value names the credentials' source, never the credentials. */
    wellfed_event(WELLFED_EV_CONNECT, attempt, result, wellfed_creds.source, wellfed_ms(begin));
    if (result != RTW_SUCCESS) {
        return 0;
    }
    /* Asked for while it associated: leave the fresh association at once. */
    if (wellfed_wifi_restarting() || !wellfed_wifi_radio()) {
        wellfed_leave(attempt);
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
    wellfed_access_point(attempt);
    return 1;
}

/* The joined access point, for the statistics: its channel and the last
 * four bytes of its BSSID, which tell a home's few access points apart. */
static void wellfed_access_point(unsigned int attempt)
{
    /* wifi_get_channel takes an int * but hands it to wext_get_channel as a
     * u8 *, so only the low byte is written: the int starts at zero for the
     * rest to read as the channel. AmebaD is dual band, so a 2.4 GHz
     * (1-14) or 5 GHz (36-177) channel; anything else is none (-1). */
    int channel = 0;
    if (wifi_get_channel(&channel) != RTW_SUCCESS ||
        !((channel >= 1 && channel <= 14) || (channel >= 36 && channel <= 177))) {
        channel = -1;
    }
    unsigned char bssid[6] = {0};
    unsigned int tail = 0;
    if (wifi_get_ap_bssid(bssid) == RTW_SUCCESS) {
        tail = ((unsigned int)bssid[2] << 24) | ((unsigned int)bssid[3] << 16) | ((unsigned int)bssid[4] << 8) |
               bssid[5];
    }
    wellfed_event(WELLFED_EV_AP, attempt, channel, tail, 0);
}

/* lwIP's DHCP client restarts its lease clock at every ACK. Seeing it go
 * back while bound to the lease is a renewal (or a rebind): no event of
 * lwIP's says so. A read of one u16 from another task, so at worst one renewal
 * is seen a tick late. */
static unsigned int wellfed_lease_used(void)
{
    struct dhcp *dhcp = netif_dhcp_data(&xnetif[0]);
    return dhcp != NULL ? dhcp->lease_used : 0;
}

/* Bound to a lease: a release (an expiry) also zeroes the lease clock, and
 * is no renewal. */
static int wellfed_dhcp_bound(void)
{
    struct dhcp *dhcp = netif_dhcp_data(&xnetif[0]);
    return dhcp != NULL && dhcp->state == DHCP_STATE_BOUND;
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
    /* A restart asked for while the turn waited: join no more; nor while
     * battery mode wants the radio off. */
    int joined = !wellfed_wifi_restarting() && wellfed_wifi_radio() && wellfed_join_once(attempt);
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
    switch (result) {
    case 0:
        return WELLFED_PROBE_ANSWERED;
    case WELLFED_ECHO_NO_GATEWAY:
        return WELLFED_PROBE_NO_GATEWAY;
    case WELLFED_ECHO_TIMEOUT:
        return WELLFED_PROBE_SILENT;
    default:
        /* No socket or no send: nothing was asked of the gateway. */
        return WELLFED_PROBE_UNSENT;
    }
}

/* Watch a joined link until Rust says leave; the lost event's cause. */
static int wellfed_watch(unsigned int attempt, unsigned int *backoff)
{
    int probe = WELLFED_PROBE_NONE;
    unsigned int lease_used = wellfed_lease_used();
    for (;;) {
        unsigned int used = wellfed_lease_used();
        if (used < lease_used && wellfed_dhcp_bound()) {
            wellfed_event(WELLFED_EV_RENEW, attempt, 0, used, 0);
        }
        lease_used = used;
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
        /* In steps, so a restart's leave (or battery mode's) starts within
         * one rather than at the next watch tick. */
        for (unsigned int waited = 0;
             waited < WELLFED_WATCH_MS && !wellfed_wifi_restarting() && wellfed_wifi_radio();
             waited += WELLFED_POLL_MS) {
            vTaskDelay(WELLFED_POLL_MS / portTICK_PERIOD_MS);
        }
    }
}

static void wellfed_park(void)
{
    for (;;) {
        vTaskDelay(portMAX_DELAY);
    }
}

/* Wait `ms` in polls, ending early once a restart or battery mode wants
 * the task's attention. */
static void wellfed_wait(unsigned int ms)
{
    for (unsigned int waited = 0; waited < ms && !wellfed_wifi_restarting() && wellfed_wifi_radio();
         waited += WELLFED_POLL_MS) {
        vTaskDelay(WELLFED_POLL_MS / portTICK_PERIOD_MS);
    }
}

/* Battery mode, with no association held: turn the radio off and wait
 * until Rust asks for it on again, then turn it on as at boot (wifi_on in
 * station mode, the same power-save settings) for a fresh join. wifi_off
 * stops lwIP's DHCP client and takes the interface down, so the device
 * API's address goes with it. A restart asked for meanwhile parks the task
 * with the radio off. A wifi_on that fails is tried again every
 * WELLFED_RADIO_RETRY_MS; the boot's own wifi_on gives up instead, but here
 * the radio has worked this boot. */
static void wellfed_radio_off(unsigned int attempt)
{
    TickType_t begin = xTaskGetTickCount();
    LwIP_ReleaseIP(0);
    int off = wifi_off();
    wellfed_event(WELLFED_EV_OFF, attempt, off, 0, wellfed_ms(begin));
    for (;;) {
        while (!wellfed_wifi_radio() && !wellfed_wifi_restarting()) {
            vTaskDelay(WELLFED_POLL_MS / portTICK_PERIOD_MS);
        }
        if (wellfed_wifi_restarting()) {
            wellfed_park();
        }
        begin = xTaskGetTickCount();
        /* 0 started, 1 already running; below 0 the driver failed. */
        int on = wifi_on(RTW_MODE_STA);
        wellfed_event(WELLFED_EV_ON, attempt, on, 0, wellfed_ms(begin));
        if (on >= 0) {
            wifi_set_power_mode(0, 1);
            return;
        }
        for (unsigned int waited = 0; waited < WELLFED_RADIO_RETRY_MS && !wellfed_wifi_restarting();
             waited += WELLFED_POLL_MS) {
            vTaskDelay(WELLFED_POLL_MS / portTICK_PERIOD_MS);
        }
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
    /* Inactive power save off, leisure power save kept. After a rejected
     * join the driver tears down in the timer task, and its key-table writes
     * (rtw_clearstakey_cmd, then ROM_WIFI_CAM_WRITE) race rtw_ps_processor's
     * ips_enter, which powers the MAC down when unassociated. A store to the
     * powered-down MAC stalled the bus and the tick with it, until the
     * watchdog reset the board: the call trail caught it in every such hang. */
    wifi_set_power_mode(0, 1);
    unsigned int attempt = 0;
    unsigned int backoff = WELLFED_BACKOFF_MIN_MS;
    for (;;) {
        /* Between attempts no join holds the network: one that got no
         * address disconnected (wellfed_join_once). */
        if (wellfed_wifi_restarting()) {
            wellfed_park();
        }
        if (!wellfed_wifi_radio()) {
            wellfed_radio_off(attempt);
            backoff = WELLFED_BACKOFF_MIN_MS;
        }
        attempt++;
        if (wellfed_join(attempt)) {
            TickType_t joined = xTaskGetTickCount();
            int cause = wellfed_watch(attempt, &backoff);
            if (cause == WELLFED_CAUSE_RESTART) {
                wellfed_leave(attempt);
                wellfed_park();
            }
            /* Battery mode: no loss to count; the radio goes off at the
             * top of the loop. */
            if (cause == WELLFED_CAUSE_RADIO) {
                wellfed_leave(attempt);
                continue;
            }
            wellfed_event(WELLFED_EV_LOST, attempt, cause, wellfed_ms(joined), 0);
            LwIP_DHCP(0, DHCP_STOP);
            LwIP_ReleaseIP(0);
            /* Still associated: leave, so the next attempt joins afresh. */
            if (cause != 0) {
                wifi_disconnect();
            }
        }
        wellfed_event(WELLFED_EV_BACKOFF, attempt, 0, backoff, 0);
        wellfed_wait(backoff);
        backoff = backoff * 2 > WELLFED_BACKOFF_MAX_MS ? WELLFED_BACKOFF_MAX_MS : backoff * 2;
    }
}

/* One task's stack for the statistics, shared with Rust
 * (`wellfed_amebad_sys::TaskStack`). */
struct wellfed_task_stack {
    char name[16];
    unsigned int free_min;
};

#define WELLFED_TASKS 24

/* Each task's name and the fewest free stack bytes it has had, into `out`
 * (at most `max`); how many it wrote, with the number of tasks in `total`.
 * More tasks than WELLFED_TASKS lists none, and `total` says so. The RTOS
 * walks each stack's untouched fill, and the names point into the tasks'
 * control blocks, so the scheduler stays paused until they are copied; the
 * loop asks at most every few seconds, for an owner's statistics. */
int wellfed_task_stacks(struct wellfed_task_stack *out, int max, unsigned int *total)
{
    static TaskStatus_t status[WELLFED_TASKS];
    int written = 0;
    vTaskSuspendAll();
    UBaseType_t tasks = uxTaskGetNumberOfTasks();
    UBaseType_t count = tasks <= WELLFED_TASKS ? uxTaskGetSystemState(status, WELLFED_TASKS, NULL) : 0;
    for (UBaseType_t i = 0; i < count && written < max; i++) {
        struct wellfed_task_stack *task = &out[written++];
        memset(task->name, 0, sizeof(task->name));
        const char *name = status[i].pcTaskName != NULL ? status[i].pcTaskName : "";
        for (unsigned int c = 0; c < sizeof(task->name) && name[c] != '\0'; c++) {
            task->name[c] = name[c];
        }
        task->free_min = (unsigned int)status[i].usStackHighWaterMark * sizeof(StackType_t);
    }
    (void)xTaskResumeAll();
    *total = (unsigned int)tasks;
    return written;
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

void wellfed_tcp_close(int s)
{
    lwip_close(s);
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

/* The lease's gateway, host order, or 0 until DHCP gave one. */
unsigned int wellfed_gateway_ipv4(void)
{
    if (!dhcp_supplied_address(&xnetif[0])) {
        return 0;
    }
    return wellfed_be32(LwIP_GetGW(&xnetif[0]));
}

/* The KM0 RTC's time, control, status and prescaler registers, read only.
 * The SDK's RTC header says the RTC keeps counting through every reset but
 * a power loss; nothing in this firmware sets it, so the boot record shows
 * whether it runs at all and whether its count survives a watchdog reset,
 * an activation or a trial boot.
 * Reading it from KM4 is what the SDK itself does: the KM4 build compiles
 * rtl8721d_rtc.c (ram_common) and mbed's rtc_api.c, whose rtc_read and
 * RTC_Init access these registers at RTC_BASE with no clock enable first,
 * since the RTC sits in the always-on domain on the 32 kHz clock KM0 starts
 * at every boot. A read that faulted would fail bank 1's trial, which falls
 * back to bank 0; the first bench boot confirms it does not. */
void wellfed_rtc_read(unsigned int *out)
{
    out[0] = HAL_READ32(RTC_BASE, 0x00); /* TR */
    out[1] = HAL_READ32(RTC_BASE, 0x04); /* CR */
    out[2] = HAL_READ32(RTC_BASE, 0x08); /* ISR */
    out[3] = HAL_READ32(RTC_BASE, 0x0C); /* PRER */
}

/* The wall clock's record for the next boot after a warm reset: 16 words at
 * the top of KM4's main RAM (0x1007BFC0), which the builder's linker script
 * takes from the heap's end and names, with the call trail below it. A
 * watchdog or system reset leaves this RAM alone: the KM4 boot loader copies
 * only the image's .ram_image2.entry/.text/.data, app_start clears only
 * .bss, the heap ends below both, and the ROM, boot loader and stacks use
 * the secure regions above 0x1007C000 and the RAM below 0x10005000. A
 * power-on leaves whatever the cells hold, which the record's check
 * refuses. The SDK's cache header says the D-cache writes through; the
 * clean makes the store reach the RAM before a reset whichever way it is
 * set. */
extern unsigned int wellfed_time_carry[16];

void wellfed_time_carry_load(unsigned int *out)
{
    volatile unsigned int *carry = wellfed_time_carry;
    for (int i = 0; i < 16; i++) {
        out[i] = carry[i];
    }
}

void wellfed_time_carry_store(const unsigned int *words)
{
    volatile unsigned int *carry = wellfed_time_carry;
    for (int i = 0; i < 16; i++) {
        carry[i] = words[i];
    }
    DCache_Clean((u32)wellfed_time_carry, sizeof(unsigned int) * 16);
}

/* The NTP exchange's socket: UDP on an ephemeral port, non-blocking, or
 * -1. Datagrams go through wellfed_mdns_send and wellfed_mdns_recv, which
 * take any UDP socket. */
int wellfed_udp_open(void)
{
    int s = lwip_socket(AF_INET, SOCK_DGRAM, 0);
    if (s < 0) {
        return -1;
    }
    struct sockaddr_in at;
    memset(&at, 0, sizeof(at));
    at.sin_family = AF_INET;
    at.sin_port = 0;
    at.sin_addr.s_addr = lwip_htonl(INADDR_ANY);
    if (lwip_bind(s, (struct sockaddr *)&at, sizeof(at)) != 0) {
        lwip_close(s);
        return -1;
    }
    wellfed_nonblocking(s);
    return s;
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

void wellfed_tick_hook(const char *name, int suspended);

/* FreeRTOS calls this from every tick interrupt (the builder sets
 * configUSE_TICK_HOOK), with the scheduler suspended too. Rust leaves the
 * interrupted task and the ticks since the watchdog feeder last ran in a
 * backup register, so the boot after a silent watchdog reset can say which
 * task spun or masked interrupts (wellfed-hal supervision/crumb.rs). */
void vApplicationTickHook(void)
{
    wellfed_tick_hook(pcTaskGetName(NULL), xTaskGetSchedulerState() == taskSCHEDULER_SUSPENDED);
}

void pre_example_entry(void)
{
}

void example_entry(void)
{
    wellfed_app_start();
}
