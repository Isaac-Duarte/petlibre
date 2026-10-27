-- Alerts: what the server noticed about a feeder, kept with where each
-- stands. `kind` is the condition's tag and `subject` what tells two of
-- one kind on one feeder apart (a feed, a journal entry, an install; empty
-- for a condition the feeder is in or not); `condition` is the API's
-- `AlertCondition` as JSON. An alert is open until `acked_ms`, and cleared
-- at `cleared_ms`. `seq` orders the list and is its cursor. `acked_by`
-- names no foreign key: the alerts outlive the accounts they name.
CREATE TABLE alerts (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id BLOB NOT NULL UNIQUE,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    subject TEXT NOT NULL,
    condition TEXT NOT NULL,
    since_ms INTEGER NOT NULL,
    opened_ms INTEGER NOT NULL,
    acked_ms INTEGER,
    acked_by BLOB,
    cleared_ms INTEGER,
    CHECK ((acked_ms IS NULL) = (acked_by IS NULL))
) STRICT;

-- One alert at a time per condition that holds, so a condition raised
-- twice (a tick and an event racing) is one alert. A feed's alert never
-- clears, so its feed is never raised again.
CREATE UNIQUE INDEX alerts_standing ON alerts (feeder_id, kind, subject)
    WHERE cleared_ms IS NULL;

CREATE INDEX alerts_by_feeder ON alerts (feeder_id, seq);
