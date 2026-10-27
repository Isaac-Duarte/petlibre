-- Accounts that expire, sessions that last from their last use, and the
-- household.

-- Past `expires_ms` an account is disabled, not deleted.
ALTER TABLE users ADD COLUMN expires_ms INTEGER;

-- A session ends a fixed time after `last_used_ms`. Sessions held nothing
-- worth keeping across this change, so the table is made again.
DROP TABLE sessions;
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY NOT NULL,
    user_id BLOB NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_ms INTEGER NOT NULL,
    last_used_ms INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_by_user ON sessions (user_id);

-- One row, written with the first owner. `zone` is an IANA name.
CREATE TABLE household (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    name TEXT NOT NULL,
    units TEXT NOT NULL,
    time_format TEXT NOT NULL,
    zone TEXT NOT NULL
) STRICT;

-- A server set up before the household existed gets the defaults.
INSERT INTO household (id, name, units, time_format, zone)
SELECT 1, 'Home', 'grams', '24h', 'UTC' WHERE EXISTS (SELECT 1 FROM users);
