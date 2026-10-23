/* FW34: the Bluetooth LE peripheral a firmware with `bluetooth = true`
 * links beside example_entry.c (petlibre-build stages it only then).
 *
 * One task waits for the Wi-Fi shim's radio (hci_board_init refuses to
 * start BT before wifi_on), brings up the vendor stack in the order the
 * SDK's ble_peripheral example uses, then runs the GAP message loop. It
 * serves one private GATT service with a single control characteristic
 * (write, notify), whose UUIDs, like the advertised name, come from Rust
 * (`petlibre_core::ble_setup`), and holds one connection. It advertises
 * only while Rust asks it to.
 *
 * Nothing here writes flash or eFuse. Pairing is off and the stack keeps no
 * keys (gap_config_le_key_storage_flag(0)); the FTL bond store is not
 * linked, and its two entry points below refuse, so the link audit can keep
 * ftl_init and every flash writer absent. Access control is the signed
 * request the Rust side checks.
 *
 * GATT callbacks only copy bytes into rings and record events; Rust drains
 * them from its own task. Requests from Rust (notify, advertise, disconnect)
 * go to this task's IO queue, so every stack call happens on the task that
 * started the stack.
 */
#include "FreeRTOS.h"
#include "task.h"
#include <string.h>
#include <platform/platform_stdlib.h>
#include "wifi_conf.h"
#include <os_msg.h>
#include <app_msg.h>
#include <trace_app.h>
#include <bte.h>
#include <gap.h>
#include <gap_adv.h>
#include <gap_bond_le.h>
#include <gap_config.h>
#include <gap_conn_le.h>
#include <gap_le.h>
#include <gap_msg.h>
#include <profile_server.h>
#include "rtk_coex.h"

#define PETLIBRE_BLE_EVENTS 16
/* Holds one signed request at petlibre_core::ble_setup::MAX_MESSAGE plus its prefix. */
#define PETLIBRE_BLE_CONTROL_RING 1024
#define PETLIBRE_BLE_NOTIFY_RING 512
#define PETLIBRE_BLE_WIFI_WAIT_MS 60000
#define PETLIBRE_BLE_QUEUE 0x20
#define PETLIBRE_BLE_MTU 247
/* Units of 0.625 ms: about 100 ms. */
#define PETLIBRE_BLE_ADV_INTERVAL 160

/* Event kinds; shared with Rust (`sdk::BleEvent`). */
enum {
    PETLIBRE_BLE_EV_READY = 1,      /* result: 0, or a PETLIBRE_BLE_FAIL_* */
    PETLIBRE_BLE_EV_ADVERTISING = 2, /* value: 1 started, 0 stopped */
    PETLIBRE_BLE_EV_CONNECT = 3,    /* value: interval (1.25 ms units) */
    PETLIBRE_BLE_EV_DISCONNECT = 4, /* result: HCI cause */
    PETLIBRE_BLE_EV_MTU = 5,        /* value: ATT MTU */
    PETLIBRE_BLE_EV_NOTIFY = 8,     /* value: 1 enabled, 0 disabled */
};

enum {
    PETLIBRE_BLE_FAIL_WIFI = -1,
    PETLIBRE_BLE_FAIL_BTE = -2,
    PETLIBRE_BLE_FAIL_SERVICE = -3,
    PETLIBRE_BLE_FAIL_STACK = -4,
};

/* What Rust hands over at start (`sdk::BleService`). */
struct petlibre_ble_config {
    const unsigned char *name;
    unsigned int name_len;
    unsigned char service[16];
    unsigned char control[16];
};

struct petlibre_ble_event {
    unsigned int seq;
    unsigned int kind;
    int result;
    unsigned int value;
};

/* Counters and link state for health lines (`sdk::BleStats`). */
struct petlibre_ble_stats {
    unsigned int control_dropped;
    unsigned int notify_dropped;
    unsigned int heap_free;
    unsigned int mtu;
    unsigned int connected;
};

/* A byte ring with one producer and one consumer, each side under a
 * critical section so a connect can clear it. */
struct petlibre_ring {
    unsigned char *bytes;
    unsigned int size;
    unsigned int head;
    unsigned int held;
};

static unsigned char petlibre_control_bytes[PETLIBRE_BLE_CONTROL_RING];
static unsigned char petlibre_notify_bytes[PETLIBRE_BLE_NOTIFY_RING];
static struct petlibre_ring petlibre_control = {petlibre_control_bytes, PETLIBRE_BLE_CONTROL_RING, 0, 0};
static struct petlibre_ring petlibre_notify = {petlibre_notify_bytes, PETLIBRE_BLE_NOTIFY_RING, 0, 0};

static struct petlibre_ble_event petlibre_ble_events[PETLIBRE_BLE_EVENTS];
static unsigned int petlibre_ble_seq;
static unsigned int petlibre_ble_started;
static struct petlibre_ble_config petlibre_ble_config;
static struct petlibre_ble_stats petlibre_ble_counts;
static void *petlibre_ble_io_queue;
static void *petlibre_ble_evt_queue;
static T_SERVER_ID petlibre_ble_service;
static uint8_t petlibre_ble_conn;
static T_GAP_DEV_STATE petlibre_ble_state;
/* What Rust asked for; the BLE task alone reads it and drives the stack. */
static volatile unsigned int petlibre_ble_wanted;
static unsigned int petlibre_ble_ready;

/* Our IO messages, beside the stack's IO_MSG_TYPE_BT_STATUS. */
#define PETLIBRE_BLE_IO 0x7e
enum {
    PETLIBRE_BLE_IO_NOTIFY = 1,
    PETLIBRE_BLE_IO_DISCONNECT = 2,
    PETLIBRE_BLE_IO_ADVERTISE = 3,
};

static void petlibre_ble_event(unsigned int kind, int result, unsigned int value)
{
    taskENTER_CRITICAL();
    struct petlibre_ble_event *e = &petlibre_ble_events[petlibre_ble_seq % PETLIBRE_BLE_EVENTS];
    e->seq = ++petlibre_ble_seq;
    e->kind = kind;
    e->result = result;
    e->value = value;
    taskEXIT_CRITICAL();
}

/* All of `data` or none of it, so a message is never split by a full ring. */
static int petlibre_ring_put(struct petlibre_ring *ring, const unsigned char *data, unsigned int length)
{
    int stored = 0;
    taskENTER_CRITICAL();
    if (length <= ring->size - ring->held) {
        unsigned int tail = (ring->head + ring->held) % ring->size;
        unsigned int first = length < ring->size - tail ? length : ring->size - tail;
        memcpy(ring->bytes + tail, data, first);
        memcpy(ring->bytes, data + first, length - first);
        ring->held += length;
        stored = 1;
    }
    taskEXIT_CRITICAL();
    return stored;
}

static unsigned int petlibre_ring_take(struct petlibre_ring *ring, unsigned char *out, unsigned int length)
{
    taskENTER_CRITICAL();
    unsigned int take = length < ring->held ? length : ring->held;
    unsigned int first = take < ring->size - ring->head ? take : ring->size - ring->head;
    memcpy(out, ring->bytes + ring->head, first);
    memcpy(out + first, ring->bytes, take - first);
    ring->head = (ring->head + take) % ring->size;
    ring->held -= take;
    taskEXIT_CRITICAL();
    return take;
}

static void petlibre_ring_clear(struct petlibre_ring *ring)
{
    taskENTER_CRITICAL();
    ring->head = 0;
    ring->held = 0;
    taskEXIT_CRITICAL();
}

/* The stack's bond store, normally FTL in flash. Nothing is bonded, and
 * these refuse rather than write, so no key reaches flash. */
uint32_t ftl_save_to_storage(void *p_data, uint16_t offset, uint16_t size)
{
    (void)p_data;
    (void)offset;
    (void)size;
    return 1;
}

uint32_t ftl_load_from_storage(void *p_data, uint16_t offset, uint16_t size)
{
    (void)p_data;
    (void)offset;
    (void)size;
    return 1;
}

/* Attribute indices in petlibre_ble_table. */
#define PETLIBRE_BLE_CONTROL_INDEX 2
#define PETLIBRE_BLE_CONTROL_CCCD_INDEX 3

/* The control UUID is filled in from the config before the service is
 * added; the stack keeps a pointer to this table. */
static T_ATTRIB_APPL petlibre_ble_table[] = {
    {
        ATTRIB_FLAG_VOID | ATTRIB_FLAG_LE,
        {LO_WORD(GATT_UUID_PRIMARY_SERVICE), HI_WORD(GATT_UUID_PRIMARY_SERVICE)},
        UUID_128BIT_SIZE,
        petlibre_ble_config.service,
        GATT_PERM_READ,
    },
    {
        ATTRIB_FLAG_VALUE_INCL,
        {LO_WORD(GATT_UUID_CHARACTERISTIC), HI_WORD(GATT_UUID_CHARACTERISTIC),
         GATT_CHAR_PROP_WRITE | GATT_CHAR_PROP_NOTIFY},
        1,
        NULL,
        GATT_PERM_READ,
    },
    {
        ATTRIB_FLAG_VALUE_APPL | ATTRIB_FLAG_UUID_128BIT,
        {0},
        0,
        NULL,
        GATT_PERM_WRITE,
    },
    {
        ATTRIB_FLAG_VALUE_INCL | ATTRIB_FLAG_CCCD_APPL,
        {LO_WORD(GATT_UUID_CHAR_CLIENT_CONFIG), HI_WORD(GATT_UUID_CHAR_CLIENT_CONFIG),
         LO_WORD(GATT_CLIENT_CHAR_CONFIG_DEFAULT), HI_WORD(GATT_CLIENT_CHAR_CONFIG_DEFAULT)},
        2,
        NULL,
        GATT_PERM_READ | GATT_PERM_WRITE,
    },
};

static T_APP_RESULT petlibre_ble_read(uint8_t conn_id, T_SERVER_ID service_id, uint16_t index,
                                      uint16_t offset, uint16_t *length, uint8_t **value)
{
    (void)conn_id;
    (void)service_id;
    (void)index;
    (void)offset;
    (void)length;
    (void)value;
    return APP_RESULT_ATTR_NOT_FOUND;
}

static T_APP_RESULT petlibre_ble_write(uint8_t conn_id, T_SERVER_ID service_id, uint16_t index,
                                       T_WRITE_TYPE type, uint16_t length, uint8_t *value,
                                       P_FUN_WRITE_IND_POST_PROC *post)
{
    (void)conn_id;
    (void)service_id;
    (void)type;
    *post = NULL;
    if (value == NULL) {
        return APP_RESULT_INVALID_VALUE_SIZE;
    }
    if (index != PETLIBRE_BLE_CONTROL_INDEX) {
        return APP_RESULT_ATTR_NOT_FOUND;
    }
    if (!petlibre_ring_put(&petlibre_control, value, length)) {
        petlibre_ble_counts.control_dropped += length;
    }
    return APP_RESULT_SUCCESS;
}

static void petlibre_ble_cccd(uint8_t conn_id, T_SERVER_ID service_id, uint16_t index, uint16_t bits)
{
    (void)conn_id;
    (void)service_id;
    if (index == PETLIBRE_BLE_CONTROL_CCCD_INDEX) {
        petlibre_ble_event(PETLIBRE_BLE_EV_NOTIFY, 0, (bits & GATT_CLIENT_CHAR_CONFIG_NOTIFY) != 0);
    }
}

static const T_FUN_GATT_SERVICE_CBS petlibre_ble_callbacks = {
    petlibre_ble_read,
    petlibre_ble_write,
    petlibre_ble_cccd,
};

static T_APP_RESULT petlibre_ble_profile(T_SERVER_ID service_id, void *data)
{
    (void)service_id;
    (void)data;
    return APP_RESULT_SUCCESS;
}

/* Send what Rust queued as notifications, split to the ATT payload. A
 * refused send drops the rest: the host then sees a short message and
 * gives up, and the Rust side drops the link. */
static void petlibre_ble_flush(void)
{
    unsigned int payload = petlibre_ble_counts.mtu > 3 ? petlibre_ble_counts.mtu - 3 : 20;
    unsigned char chunk[PETLIBRE_BLE_MTU];
    for (;;) {
        unsigned int n = petlibre_ring_take(&petlibre_notify, chunk, payload < sizeof chunk ? payload : sizeof chunk);
        if (n == 0) {
            return;
        }
        if (!petlibre_ble_counts.connected ||
            !server_send_data(petlibre_ble_conn, petlibre_ble_service, PETLIBRE_BLE_CONTROL_INDEX, chunk,
                              (uint16_t)n, GATT_PDU_TYPE_NOTIFICATION)) {
            petlibre_ble_counts.notify_dropped += n;
            unsigned char rest[PETLIBRE_BLE_NOTIFY_RING];
            petlibre_ble_counts.notify_dropped += petlibre_ring_take(&petlibre_notify, rest, sizeof rest);
            return;
        }
    }
}

/* Start or stop advertising to match what Rust wants. While connected the
 * stack does not advertise; a disconnect calls this again. */
static void petlibre_ble_follow(void)
{
    if (!petlibre_ble_ready || petlibre_ble_counts.connected) {
        return;
    }
    if (petlibre_ble_wanted && petlibre_ble_state.gap_adv_state == GAP_ADV_STATE_IDLE) {
        le_adv_start();
    } else if (!petlibre_ble_wanted && petlibre_ble_state.gap_adv_state == GAP_ADV_STATE_ADVERTISING) {
        le_adv_stop();
    }
}

static void petlibre_ble_on_dev_state(T_GAP_DEV_STATE state)
{
    int became_ready = state.gap_init_state != petlibre_ble_state.gap_init_state &&
                       state.gap_init_state == GAP_INIT_STATE_STACK_READY;
    if (state.gap_adv_state != petlibre_ble_state.gap_adv_state) {
        if (state.gap_adv_state == GAP_ADV_STATE_ADVERTISING) {
            petlibre_ble_event(PETLIBRE_BLE_EV_ADVERTISING, 0, 1);
        } else if (state.gap_adv_state == GAP_ADV_STATE_IDLE) {
            petlibre_ble_event(PETLIBRE_BLE_EV_ADVERTISING, 0, 0);
        }
    }
    petlibre_ble_state = state;
    if (became_ready) {
        petlibre_ble_ready = 1;
        petlibre_ble_event(PETLIBRE_BLE_EV_READY, 0, 0);
    }
    petlibre_ble_follow();
}

static void petlibre_ble_on_conn_state(uint8_t conn_id, T_GAP_CONN_STATE state, uint16_t cause)
{
    if (state == GAP_CONN_STATE_CONNECTED) {
        petlibre_ring_clear(&petlibre_control);
        petlibre_ring_clear(&petlibre_notify);
        petlibre_ble_conn = conn_id;
        petlibre_ble_counts.mtu = 23;
        petlibre_ble_counts.connected = 1;

        uint16_t interval = 0;
        le_get_conn_param(GAP_PARAM_CONN_INTERVAL, &interval, conn_id);
        petlibre_ble_event(PETLIBRE_BLE_EV_CONNECT, 0, interval);
    } else if (state == GAP_CONN_STATE_DISCONNECTED) {
        petlibre_ble_counts.connected = 0;
        petlibre_ble_event(PETLIBRE_BLE_EV_DISCONNECT, cause, 0);
        petlibre_ble_follow();
    }
}

static void petlibre_ble_on_gap_msg(T_IO_MSG *msg)
{
    T_LE_GAP_MSG gap;
    memcpy(&gap, &msg->u.param, sizeof(msg->u.param));
    switch (msg->subtype) {
    case GAP_MSG_LE_DEV_STATE_CHANGE:
        petlibre_ble_on_dev_state(gap.msg_data.gap_dev_state_change.new_state);
        break;
    case GAP_MSG_LE_CONN_STATE_CHANGE:
        petlibre_ble_on_conn_state(gap.msg_data.gap_conn_state_change.conn_id,
                                   (T_GAP_CONN_STATE)gap.msg_data.gap_conn_state_change.new_state,
                                   gap.msg_data.gap_conn_state_change.disc_cause);
        break;
    case GAP_MSG_LE_CONN_MTU_INFO:
        petlibre_ble_counts.mtu = gap.msg_data.gap_conn_mtu_info.mtu_size;
        petlibre_ble_event(PETLIBRE_BLE_EV_MTU, 0, gap.msg_data.gap_conn_mtu_info.mtu_size);
        break;
    case GAP_MSG_LE_BOND_JUST_WORK:
        /* Pairing is off; refuse anything that still asks. */
        le_bond_just_work_confirm(gap.msg_data.gap_bond_just_work_conf.conn_id, GAP_CFM_CAUSE_REJECT);
        break;
    default:
        break;
    }
}

/* Advertising carries the service UUID (hosts filter on it); the scan
 * response carries the name. */
static void petlibre_ble_gap_params(void)
{
    uint8_t name[GAP_DEVICE_NAME_LEN] = {0};
    unsigned int name_len = petlibre_ble_config.name_len < 29 ? petlibre_ble_config.name_len : 29;
    memcpy(name, petlibre_ble_config.name, name_len);
    le_set_gap_param(GAP_PARAM_DEVICE_NAME, GAP_DEVICE_NAME_LEN, name);
    uint16_t appearance = GAP_GATT_APPEARANCE_UNKNOWN;
    le_set_gap_param(GAP_PARAM_APPEARANCE, sizeof(appearance), &appearance);
    uint8_t mtu_req = 1;
    le_set_gap_param(GAP_PARAM_SLAVE_INIT_GATT_MTU_REQ, sizeof(mtu_req), &mtu_req);

    static uint8_t adv[3 + 2 + 16];
    adv[0] = 2;
    adv[1] = GAP_ADTYPE_FLAGS;
    adv[2] = GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED;
    adv[3] = 17;
    adv[4] = GAP_ADTYPE_128BIT_COMPLETE;
    memcpy(adv + 5, petlibre_ble_config.service, 16);
    static uint8_t scan[2 + 29];
    scan[0] = (uint8_t)(name_len + 1);
    scan[1] = GAP_ADTYPE_LOCAL_NAME_COMPLETE;
    memcpy(scan + 2, name, name_len);

    uint8_t event_type = GAP_ADTYPE_ADV_IND;
    uint8_t direct_type = GAP_REMOTE_ADDR_LE_PUBLIC;
    uint8_t direct_addr[GAP_BD_ADDR_LEN] = {0};
    uint8_t channels = GAP_ADVCHAN_ALL;
    uint8_t filter = GAP_ADV_FILTER_ANY;
    uint16_t interval = PETLIBRE_BLE_ADV_INTERVAL;
    le_adv_set_param(GAP_PARAM_ADV_EVENT_TYPE, sizeof(event_type), &event_type);
    le_adv_set_param(GAP_PARAM_ADV_DIRECT_ADDR_TYPE, sizeof(direct_type), &direct_type);
    le_adv_set_param(GAP_PARAM_ADV_DIRECT_ADDR, sizeof(direct_addr), direct_addr);
    le_adv_set_param(GAP_PARAM_ADV_CHANNEL_MAP, sizeof(channels), &channels);
    le_adv_set_param(GAP_PARAM_ADV_FILTER_POLICY, sizeof(filter), &filter);
    le_adv_set_param(GAP_PARAM_ADV_INTERVAL_MIN, sizeof(interval), &interval);
    le_adv_set_param(GAP_PARAM_ADV_INTERVAL_MAX, sizeof(interval), &interval);
    le_adv_set_param(GAP_PARAM_ADV_DATA, sizeof(adv), adv);
    le_adv_set_param(GAP_PARAM_SCAN_RSP_DATA, (uint16_t)(name_len + 2), scan);

    uint8_t pairing = GAP_PAIRING_MODE_NO_PAIRING;
    gap_set_param(GAP_PARAM_BOND_PAIRING_MODE, sizeof(pairing), &pairing);
}

static void petlibre_ble_task(void *unused)
{
    (void)unused;
    TickType_t begin = xTaskGetTickCount();
    while (!wifi_is_up(RTW_STA_INTERFACE)) {
        if ((xTaskGetTickCount() - begin) * portTICK_PERIOD_MS >= PETLIBRE_BLE_WIFI_WAIT_MS) {
            petlibre_ble_event(PETLIBRE_BLE_EV_READY, PETLIBRE_BLE_FAIL_WIFI, 0);
            vTaskDelete(NULL);
            return;
        }
        vTaskDelay(100 / portTICK_PERIOD_MS);
    }

    bt_trace_init();
    gap_config_max_le_link_num(1);
    gap_config_max_le_paired_device(1);
    gap_config_le_key_storage_flag(0);
    gap_config_max_mtu_size(PETLIBRE_BLE_MTU);
    if (!bte_init()) {
        petlibre_ble_event(PETLIBRE_BLE_EV_READY, PETLIBRE_BLE_FAIL_BTE, 0);
        vTaskDelete(NULL);
        return;
    }
    le_gap_init(1);
    petlibre_ble_gap_params();

    memcpy(petlibre_ble_table[PETLIBRE_BLE_CONTROL_INDEX].type_value, petlibre_ble_config.control, 16);
    server_init(1);
    if (!server_add_service(&petlibre_ble_service, (uint8_t *)petlibre_ble_table, sizeof(petlibre_ble_table),
                            petlibre_ble_callbacks)) {
        petlibre_ble_event(PETLIBRE_BLE_EV_READY, PETLIBRE_BLE_FAIL_SERVICE, 0);
        vTaskDelete(NULL);
        return;
    }
    server_register_app_cb(petlibre_ble_profile);

    os_msg_queue_create(&petlibre_ble_io_queue, PETLIBRE_BLE_QUEUE, sizeof(T_IO_MSG));
    os_msg_queue_create(&petlibre_ble_evt_queue, 2 * PETLIBRE_BLE_QUEUE, sizeof(uint8_t));
    if (!gap_start_bt_stack(petlibre_ble_evt_queue, petlibre_ble_io_queue, PETLIBRE_BLE_QUEUE)) {
        petlibre_ble_event(PETLIBRE_BLE_EV_READY, PETLIBRE_BLE_FAIL_STACK, 0);
        vTaskDelete(NULL);
        return;
    }
    bt_coex_init();

    for (;;) {
        uint8_t event;
        if (!os_msg_recv(petlibre_ble_evt_queue, &event, 0xFFFFFFFF)) {
            continue;
        }
        if (event != EVENT_IO_TO_APP) {
            gap_handle_msg(event);
            continue;
        }
        T_IO_MSG msg;
        if (!os_msg_recv(petlibre_ble_io_queue, &msg, 0)) {
            continue;
        }
        if (msg.type == IO_MSG_TYPE_BT_STATUS) {
            petlibre_ble_on_gap_msg(&msg);
        } else if (msg.type == PETLIBRE_BLE_IO && msg.subtype == PETLIBRE_BLE_IO_NOTIFY) {
            petlibre_ble_flush();
        } else if (msg.type == PETLIBRE_BLE_IO && msg.subtype == PETLIBRE_BLE_IO_DISCONNECT) {
            if (petlibre_ble_counts.connected) {
                le_disconnect(petlibre_ble_conn);
            }
        } else if (msg.type == PETLIBRE_BLE_IO && msg.subtype == PETLIBRE_BLE_IO_ADVERTISE) {
            petlibre_ble_follow();
        }
    }
}

static int petlibre_ble_post(uint16_t subtype)
{
    if (petlibre_ble_io_queue == NULL || petlibre_ble_evt_queue == NULL) {
        return 0;
    }
    T_IO_MSG msg = {0};
    msg.type = PETLIBRE_BLE_IO;
    msg.subtype = subtype;
    uint8_t event = EVENT_IO_TO_APP;
    return os_msg_send(petlibre_ble_io_queue, &msg, 0) && os_msg_send(petlibre_ble_evt_queue, &event, 0);
}

/* Once per boot: 1 when the task was created. The config is copied. */
int petlibre_ble_start(const struct petlibre_ble_config *config)
{
    if (petlibre_ble_started) {
        return 0;
    }
    petlibre_ble_started = 1;
    petlibre_ble_config = *config;
    if (xTaskCreate(petlibre_ble_task, "petlibre_ble", 1024, NULL, tskIDLE_PRIORITY + 1, NULL) != pdPASS) {
        petlibre_ble_started = 0;
        return 0;
    }
    return 1;
}

/* As petlibre_wifi_event: 1 copied, 0 not yet recorded, -1 overwritten. */
int petlibre_ble_event_read(unsigned int seq, struct petlibre_ble_event *out)
{
    int status;
    taskENTER_CRITICAL();
    if (seq == 0 || seq > petlibre_ble_seq) {
        status = 0;
    } else if (petlibre_ble_seq - seq >= PETLIBRE_BLE_EVENTS) {
        status = -1;
    } else {
        *out = petlibre_ble_events[(seq - 1) % PETLIBRE_BLE_EVENTS];
        status = 1;
    }
    taskEXIT_CRITICAL();
    return status;
}

unsigned int petlibre_ble_control_read(void *buffer, unsigned int length)
{
    return petlibre_ring_take(&petlibre_control, buffer, length);
}

/* Queue `data` as control notifications; 1 when queued (sent later by the
 * BLE task). */
int petlibre_ble_notify(const void *data, unsigned int length)
{
    if (!petlibre_ring_put(&petlibre_notify, data, length)) {
        petlibre_ble_counts.notify_dropped += length;
        return 0;
    }
    return petlibre_ble_post(PETLIBRE_BLE_IO_NOTIFY);
}

/* Advertise (1) or not (0) whenever no host is connected. */
int petlibre_ble_advertise(int on)
{
    petlibre_ble_wanted = on != 0;
    return petlibre_ble_post(PETLIBRE_BLE_IO_ADVERTISE);
}

int petlibre_ble_disconnect(void)
{
    return petlibre_ble_post(PETLIBRE_BLE_IO_DISCONNECT);
}

void petlibre_ble_stats(struct petlibre_ble_stats *out)
{
    taskENTER_CRITICAL();
    *out = petlibre_ble_counts;
    taskEXIT_CRITICAL();
    out->heap_free = xPortGetFreeHeapSize();
}
