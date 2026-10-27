-- A removed feeder is archived, not deleted: its history, samples and
-- journal stay readable, and adopting the same identity again clears this
-- and continues them.
ALTER TABLE feeders ADD COLUMN archived_ms INTEGER;

-- Each phone key the server made an invite for, recorded before the invite
-- is asked for, so a phone that joins is always attributed. `user_id` and
-- `token_id` carry no foreign key: a deleted account's keys must still be
-- revoked. `revoke` is why the key is being revoked (null while it
-- stands); the row goes once the feeder has dropped the key. `attempt` is
-- the last attempt's outcome, as JSON.
CREATE TABLE phone_keys (
    feeder_id BLOB NOT NULL REFERENCES feeders (id),
    key BLOB NOT NULL,
    user_id BLOB NOT NULL,
    token_id BLOB,
    role TEXT NOT NULL,
    invited_ms INTEGER NOT NULL,
    revoke TEXT,
    revoke_ms INTEGER,
    attempt TEXT,
    PRIMARY KEY (feeder_id, key)
) STRICT;

CREATE INDEX phone_keys_by_user ON phone_keys (user_id);
CREATE INDEX phone_keys_by_token ON phone_keys (token_id);

-- Clients the server did not invite that an owner chose to keep.
CREATE TABLE kept_clients (
    feeder_id BLOB NOT NULL REFERENCES feeders (id),
    key BLOB NOT NULL,
    kept_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, key)
) STRICT;
