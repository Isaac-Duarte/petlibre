-- The schema. Until the first release, changes are folded into this file;
-- after it, a released migration is never edited and a change is a new
-- file. Times are Unix milliseconds (UTC); IDs are UUIDv7 bytes.

CREATE TABLE users (
    id BLOB PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL,
    created_ms INTEGER NOT NULL
) STRICT;

-- A browser session's token is stored hashed, so a copied database holds
-- no live session.
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL
) STRICT;

-- The server's own key for each feeder is sealed with the config file's
-- key, so the database alone cannot impersonate the server. `synced` is
-- the newest event held: the cursor a reconnect subscribes after. It
-- counts in `epoch`, the feeder's ring epoch, which a new ring draws at
-- random: a ring re-created by a storage wipe counts from 1 again under
-- another epoch.
CREATE TABLE feeders (
    id BLOB PRIMARY KEY NOT NULL,
    identity BLOB NOT NULL UNIQUE,
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    address TEXT,
    key_nonce BLOB NOT NULL,
    key_sealed BLOB NOT NULL,
    synced INTEGER,
    epoch INTEGER,
    created_ms INTEGER NOT NULL
) STRICT;

-- The last status and settings read, served while the feeder is offline.
CREATE TABLE feeder_state (
    feeder_id BLOB PRIMARY KEY NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    status TEXT,
    status_ms INTEGER,
    settings TEXT,
    settings_ms INTEGER
) STRICT;

-- Every event as the feeder recorded it: `body` is its postcard bytes,
-- decoded on read. The key makes a replayed event a no-op; event numbers
-- count within an epoch, so the epoch is part of it.
CREATE TABLE events (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    body BLOB NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;

CREATE INDEX events_by_kind ON events (feeder_id, epoch, kind, seq);

-- Events the feeder's ring dropped before the server read them.
CREATE TABLE gaps (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    from_seq INTEGER NOT NULL,
    to_seq INTEGER NOT NULL,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, from_seq)
) STRICT, WITHOUT ROWID;

-- Live samples, raw: `value` is the metric's postcard bytes.
CREATE TABLE samples (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    received_ms INTEGER NOT NULL,
    uptime_ms INTEGER NOT NULL,
    metric TEXT NOT NULL,
    value BLOB NOT NULL
) STRICT;

CREATE INDEX samples_by_metric ON samples (feeder_id, metric, received_ms);
