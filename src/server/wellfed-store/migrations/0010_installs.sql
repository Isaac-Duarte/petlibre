-- Installs: an owner's request to install a release on one feeder, kept
-- as it progresses and once it ends. At most one row is `running`: the
-- service runs one install at a time, and every start marks a row a
-- stopped server left running as failed (`interrupted`). `failure` is the
-- API's `InstallFailure` as JSON. `user_id` names no foreign key: the
-- history outlives the accounts it names.
CREATE TABLE installs (
    id BLOB PRIMARY KEY NOT NULL,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    release_id BLOB NOT NULL,
    version INTEGER NOT NULL,
    user_id BLOB NOT NULL,
    started_ms INTEGER NOT NULL,
    state TEXT NOT NULL,
    written INTEGER NOT NULL,
    image_len INTEGER NOT NULL,
    failure TEXT,
    finished_ms INTEGER
) STRICT;

CREATE INDEX installs_by_feeder ON installs (feeder_id, started_ms);
CREATE INDEX installs_by_state ON installs (state);
