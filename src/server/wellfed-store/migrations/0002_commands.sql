-- The command journal: every write the server sends a feeder, recorded
-- before it is sent and kept forever. `key` is the client's
-- `Idempotency-Key` (or one the server drew), so a retry finds its entry
-- whether or not the feeder's ring of 32 write ids still holds it.
-- `body_hash` is the SHA-256 of the request's canonical JSON, so a key
-- reused for another request is refused. `request_id` is the device
-- request id it went out under, and the one a retry resends. `seq` orders
-- the list and is its cursor. `user_id` names no foreign key: the journal
-- outlives the accounts it names.
CREATE TABLE commands (
    seq INTEGER PRIMARY KEY,
    key BLOB NOT NULL UNIQUE,
    user_id BLOB NOT NULL,
    token_id BLOB,
    feeder_id BLOB NOT NULL REFERENCES feeders (id),
    kind TEXT NOT NULL,
    body_hash BLOB NOT NULL,
    request_id BLOB NOT NULL,
    boot_count INTEGER,
    state TEXT NOT NULL,
    outcome TEXT,
    feed INTEGER,
    sent_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX commands_by_user ON commands (user_id, seq);
CREATE INDEX commands_by_feeder ON commands (feeder_id, seq);
CREATE INDEX commands_by_state ON commands (state, seq);
