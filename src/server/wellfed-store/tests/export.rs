//! Every exported table reads out of a snapshot in its key's order and
//! inserts into a fresh database as it was, every column included; an
//! import archives the feeders, gives the accounts no password, and is
//! refused where an account exists.

use futures_util::TryStreamExt;
use jiff::Timestamp;
use wellfed_formats::export::{Row, Table};
use wellfed_store::Store;
use sqlx::SqlitePool;
use tempfile::TempDir;

/// A row in every exported table, and a second with every nullable column
/// null where the table has one, beside the rows that are not exported.
const FILL: &str = "
INSERT INTO users VALUES (x'0192c5d0000070008000000000000001', 'ada', 'hash-a', 'owner', 1, NULL);
INSERT INTO users VALUES (x'0192c5d0000070008000000000000002', 'bob', 'hash-b', 'member', 2, 99);
INSERT INTO sessions VALUES (x'aa', x'0192c5d0000070008000000000000001', 1, 2);
INSERT INTO tokens VALUES (x'0192c5d0000070008000000000000009', x'bb',
    x'0192c5d0000070008000000000000001', 'phone', 1, NULL, NULL);
INSERT INTO household VALUES (1, 'Home', 'grams', '24h', 'Europe/Berlin');
INSERT INTO feeders (id, identity, name, role, address, key_nonce, key_sealed, synced, epoch, created_ms)
    VALUES (x'0192c5d0000070008000000000000003', x'0101010101010101010101010101010101010101010101010101010101010101',
        'kitchen', 'server', '192.0.2.7:4444', x'070707070707070707070707', x'0102', 5, 9, 3);
INSERT INTO feeders (id, identity, name, role, address, key_nonce, key_sealed, synced, epoch, created_ms)
    VALUES (x'0192c5d0000070008000000000000004', x'0202020202020202020202020202020202020202020202020202020202020202',
        'hall', 'owner', NULL, x'070707070707070707070707', x'03', NULL, NULL, 4);
INSERT INTO phone_keys VALUES (x'0192c5d0000070008000000000000003', x'5151', x'0192c5d0000070008000000000000001',
    x'0192c5d0000070008000000000000009', 'member', 60, 61, 'token_revoked', 62, '{}');
INSERT INTO phone_keys VALUES (x'0192c5d0000070008000000000000003', x'5050', x'0192c5d0000070008000000000000002',
    NULL, 'viewer', 63, 64, NULL, NULL, NULL);
INSERT INTO kept_clients VALUES (x'0192c5d0000070008000000000000003', x'5252', 65);
INSERT INTO feeder_state VALUES (x'0192c5d0000070008000000000000003', '{\"a\":1}', 10, '{\"b\":2}', 11);
INSERT INTO feeder_state VALUES (x'0192c5d0000070008000000000000004', NULL, NULL, NULL, NULL);
INSERT INTO events VALUES (x'0192c5d0000070008000000000000003', 9, 2, 100, 12, 'fed', x'00ff');
INSERT INTO events VALUES (x'0192c5d0000070008000000000000003', 9, 1, NULL, 11, 'boot', x'01');
INSERT INTO gaps VALUES (x'0192c5d0000070008000000000000003', 9, 3, 4, 13);
INSERT INTO samples (id, feeder_id, received_ms, uptime_ms, metric, number, value)
    VALUES (7, x'0192c5d0000070008000000000000003', 14, 15, 'rssi', -60, x'05');
INSERT INTO samples (id, feeder_id, received_ms, uptime_ms, metric, number, value)
    VALUES (8, x'0192c5d0000070008000000000000003', 16, 17, 'power', NULL, x'06');
INSERT INTO sample_rollups VALUES (x'0192c5d0000070008000000000000003', 'rssi', 0, 1, -60, -60, -60);
UPDATE sample_marks SET rolled = 7, version = 1;
INSERT INTO projection_version VALUES (0, 3);
INSERT INTO feeds VALUES (x'0192c5d0000070008000000000000003', 9, 1, 2, 'client', x'0a0b0c0d',
    2, 2, 'done', 'requested', 100, 12);
INSERT INTO feeds VALUES (x'0192c5d0000070008000000000000003', 9, 2, 3, 'panel', NULL,
    1, 0, 'failed', 'jam', NULL, 13);
INSERT INTO lock_changes VALUES (x'0192c5d0000070008000000000000003', 9, 4, 1, 'client', x'0a0b0c0d', 101, 14);
INSERT INTO lock_changes VALUES (x'0192c5d0000070008000000000000003', 9, 5, 0, 'panel', NULL, NULL, 15);
INSERT INTO boots VALUES (x'0192c5d0000070008000000000000003', 9, 1, 1, 4, NULL, 11);
INSERT INTO clock_syncs VALUES (x'0192c5d0000070008000000000000003', 9, 6, 'client', x'0a0b0c0d', -20, 102, 16);
INSERT INTO clock_syncs VALUES (x'0192c5d0000070008000000000000003', 9, 7, 'feeder', NULL, NULL, NULL, 17);
INSERT INTO blocked_changes VALUES (x'0192c5d0000070008000000000000003', 9, 8, 'empty', 103, 18);
INSERT INTO blocked_changes VALUES (x'0192c5d0000070008000000000000003', 9, 9, NULL, NULL, 19);
INSERT INTO feed_traces VALUES (x'0192c5d0000070008000000000000003', 9, 1, 50, '{}', 1, x'0c', NULL);
INSERT INTO feed_traces VALUES (x'0192c5d0000070008000000000000003', 9, 2, 51, NULL, NULL, NULL, '\"gone\"');
INSERT INTO feeder_stats VALUES (x'0192c5d0000070008000000000000003', 'food', 52, 4, '{}');
INSERT INTO feeder_stats VALUES (x'0192c5d0000070008000000000000003', 'motor', 53, NULL, '{}');
INSERT INTO food_readings VALUES (x'0192c5d0000070008000000000000003', 55, 4, 1, 2, 3, 4);
INSERT INTO food_readings VALUES (x'0192c5d0000070008000000000000003', 54, NULL, NULL, 0, 0, NULL);
INSERT INTO commands (seq, key, user_id, token_id, feeder_id, kind, body_hash, request_id, boot_count,
        state, outcome, feed, sent_ms, updated_ms, history_epoch, history_seq)
    VALUES (1, x'11', x'0192c5d0000070008000000000000001', x'0192c5d0000070008000000000000009',
        x'0192c5d0000070008000000000000003', 'feed', x'22', x'33', 4, 'resolved', '{}', 1, 20, 21, 9, 1);
INSERT INTO commands (seq, key, user_id, token_id, feeder_id, kind, body_hash, request_id, boot_count,
        state, outcome, feed, sent_ms, updated_ms, history_epoch, history_seq)
    VALUES (2, x'12', x'0192c5d0000070008000000000000002', NULL,
        x'0192c5d0000070008000000000000003', 'lock', x'23', x'34', NULL, 'unknown', NULL, NULL, 22, 23, NULL, NULL);
INSERT INTO installs VALUES (x'0192c5d0000070008000000000000005', x'0192c5d0000070008000000000000003',
    x'44', 7, x'0192c5d0000070008000000000000001', 24, 'failed', 10, 20, '{\"kind\":\"lost\"}', 25, 'release');
INSERT INTO installs VALUES (x'0192c5d0000070008000000000000006', x'0192c5d0000070008000000000000003',
    x'45', 8, x'0192c5d0000070008000000000000001', 26, 'activated', 20, 20, NULL, NULL, 'stock');
INSERT INTO pets VALUES (x'0192c5d0000070008000000000000007', 'Tom', 'cat', '2020-01-02', 250, 27);
INSERT INTO pets VALUES (x'0192c5d0000070008000000000000008', 'Rex', 'dog', NULL, NULL, 28);
INSERT INTO pet_weights VALUES (x'0192c5d000007000800000000000000a', x'0192c5d0000070008000000000000007', 29, 4200);
INSERT INTO pet_feeders VALUES (1, x'0192c5d0000070008000000000000007', x'0192c5d0000070008000000000000003', 30, 31);
INSERT INTO pet_feeders VALUES (2, x'0192c5d0000070008000000000000007', x'0192c5d0000070008000000000000003', 32, NULL);
INSERT INTO foods VALUES (x'0192c5d000007000800000000000000b', 'Acme', 'Dry', 3600, 2000, 110, 33);
INSERT INTO feeder_foods VALUES (1, x'0192c5d0000070008000000000000003', x'0192c5d000007000800000000000000b', 34);
INSERT INTO refills VALUES (1, x'0192c5d000007000800000000000000c', x'0192c5d0000070008000000000000003',
    35, 2000, 1, x'0192c5d000007000800000000000000b', x'0192c5d0000070008000000000000001');
INSERT INTO refills VALUES (2, x'0192c5d000007000800000000000000d', x'0192c5d0000070008000000000000003',
    36, 50, 0, NULL, x'0192c5d0000070008000000000000002');
INSERT INTO alerts VALUES (1, x'0192c5d000007000800000000000000e', x'0192c5d0000070008000000000000003',
    'feed', '9:2', '{\"kind\":\"feed\"}', 37, 38, 39, x'0192c5d0000070008000000000000001', 41);
INSERT INTO alerts VALUES (2, x'0192c5d000007000800000000000000f', x'0192c5d0000070008000000000000003',
    'offline', '', '{\"kind\":\"offline\"}', 42, 43, NULL, NULL, NULL);
INSERT INTO audit (seq, at_ms, user_id, user_name, token_id, action, command_seq)
    VALUES (5, 40, x'0192c5d0000070008000000000000001', 'ada', NULL, '{\"kind\":\"user_added\"}', NULL);
INSERT INTO job_runs VALUES ('backup', 1, 2, 'ok');
";

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap()
}

async fn open(dir: &TempDir, name: &str) -> (Store, SqlitePool) {
    let path = dir.path().join(name);
    let store = Store::open(&path, at(0)).await.unwrap();
    let pool = SqlitePool::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    (store, pool)
}

/// Every exported row, table by table, from one snapshot.
async fn export(store: &Store) -> Vec<(Table, Vec<Row>)> {
    let mut snapshot = store.snapshot().await.unwrap();
    let mut tables = Vec::new();
    for &table in Table::ALL {
        let rows: Vec<Row> = snapshot.rows(table).try_collect().await.unwrap();
        tables.push((table, rows));
    }
    tables
}

#[tokio::test]
async fn every_table_comes_out_in_order_and_goes_back_in_whole() {
    let dir = tempfile::tempdir().unwrap();
    let (first, pool) = open(&dir, "first.db").await;
    sqlx::raw_sql(FILL).execute(&pool).await.unwrap();

    let exported = export(&first).await;

    for (table, rows) in &exported {
        assert!(!rows.is_empty(), "{table:?} has rows");
    }
    let events = &exported
        .iter()
        .find(|(t, _)| *t == Table::Events)
        .unwrap()
        .1;
    let seqs: Vec<i64> = events
        .iter()
        .map(|row| match row {
            Row::Events(event) => event.seq,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(seqs, [1, 2], "in the key's order, not the insert's");

    let (second, raw) = open(&dir, "second.db").await;
    let mut import = second.begin_import("").await.unwrap().unwrap();
    for (_, rows) in &exported {
        for row in rows {
            import.insert(row).await.unwrap();
        }
    }
    let archived = import.commit(at(500)).await.unwrap();

    assert_eq!(archived, 2);
    assert_eq!(export(&second).await, exported);
    let passwords: Vec<String> = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_all(&raw)
        .await
        .unwrap();
    assert_eq!(passwords, ["", ""]);
    let keys: Vec<(Vec<u8>, Vec<u8>, i64)> =
        sqlx::query_as("SELECT key_nonce, key_sealed, archived_ms FROM feeders")
            .fetch_all(&raw)
            .await
            .unwrap();
    assert_eq!(
        keys,
        [(vec![0; 12], vec![], 500), (vec![0; 12], vec![], 500)]
    );
    for (table, query) in [
        ("sessions", "SELECT COUNT(*) FROM sessions"),
        ("tokens", "SELECT COUNT(*) FROM tokens"),
        ("job_runs", "SELECT COUNT(*) FROM job_runs"),
    ] {
        let count: i64 = sqlx::query_scalar(query).fetch_one(&raw).await.unwrap();
        assert_eq!(count, 0, "{table} is not imported");
    }
    // The sample ids came along, and the next one follows them.
    sqlx::query(
        "INSERT INTO samples (feeder_id, received_ms, uptime_ms, metric, value)
         SELECT id, 0, 0, 'rssi', x'00' FROM feeders LIMIT 1",
    )
    .execute(&raw)
    .await
    .unwrap();
    let next: i64 = sqlx::query_scalar("SELECT MAX(id) FROM samples")
        .fetch_one(&raw)
        .await
        .unwrap();
    assert_eq!(next, 9);
}

#[tokio::test]
async fn an_import_is_refused_where_an_account_exists_and_rolls_back_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let (first, pool) = open(&dir, "first.db").await;
    sqlx::raw_sql(FILL).execute(&pool).await.unwrap();
    let exported = export(&first).await;

    assert!(first.begin_import("").await.unwrap().is_none());

    let (second, raw) = open(&dir, "second.db").await;
    let mut import = second.begin_import("").await.unwrap().unwrap();
    for row in &exported[0].1 {
        import.insert(row).await.unwrap();
    }
    drop(import);
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&raw)
        .await
        .unwrap();
    assert_eq!(users, 0);
    assert!(second.begin_import("").await.unwrap().is_some());
}
