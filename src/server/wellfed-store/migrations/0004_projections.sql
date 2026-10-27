-- Projections: typed rows decoded from the raw events, written in the
-- raw insert's transaction so SQL can filter, count and join them. The
-- `events` table stays the truth: every row here can be rebuilt from it,
-- and is when `projection_version` differs from the build's. Each row
-- keeps its event's epoch and number (`seq`), its feeder clock time
-- (`at_s`, absent while that clock was invalid) and when the server
-- received it. `source` is who caused it (`panel`, `client` or `feeder`),
-- with the client's fingerprint in `client` for `client` only.

-- The projection code's version the rows were built by. One row.
CREATE TABLE projection_version (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 0),
    version INTEGER NOT NULL
) STRICT;

-- One row per feed: the feeder numbers its feeds within an epoch, and a
-- later event for the same feed replaces the row (the higher `seq` wins).
CREATE TABLE feeds (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    feed INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    source TEXT NOT NULL,
    client BLOB,
    requested INTEGER NOT NULL,
    completed INTEGER NOT NULL,
    outcome TEXT NOT NULL,
    reason TEXT NOT NULL,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, feed)
) STRICT, WITHOUT ROWID;

CREATE INDEX feeds_by_time ON feeds (feeder_id, received_ms);

-- The panel lock turned on or off.
CREATE TABLE lock_changes (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    locked INTEGER NOT NULL,
    source TEXT NOT NULL,
    client BLOB,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;

CREATE INDEX lock_changes_by_time ON lock_changes (feeder_id, received_ms);

-- Each boot: the bank it ran from and the reset flags behind it.
CREATE TABLE boots (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    bank INTEGER NOT NULL,
    reset_flags INTEGER NOT NULL,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;

CREATE INDEX boots_by_time ON boots (feeder_id, received_ms);

-- The feeder's clock was set: valid from then on. `jump_ms` is how far it
-- moved when it was valid before.
CREATE TABLE clock_syncs (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    source TEXT NOT NULL,
    client BLOB,
    jump_ms INTEGER,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;

CREATE INDEX clock_syncs_by_time ON clock_syncs (feeder_id, received_ms);

-- The lasting reason the feeder cannot feed changed: `reason` is the new
-- one, or absent once it can feed again.
CREATE TABLE blocked_changes (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    reason TEXT,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;

CREATE INDEX blocked_changes_by_time ON blocked_changes (feeder_id, received_ms);
