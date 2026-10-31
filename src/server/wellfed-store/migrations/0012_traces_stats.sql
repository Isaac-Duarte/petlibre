-- What the mirror reads from a feeder beside its history: each feed's
-- trace, and its statistics. Neither is a projection: nothing in the raw
-- events rebuilds them, so a projection rebuild leaves them alone.

-- One feed's trace, fetched after its `Fed` event, keyed as `feeds` is.
-- `summary` is the trace's summary as JSON and `body` its entries in the
-- feeder's own encoding, `stored` whether the feeder had it in storage
-- rather than RAM. A feeder that had no trace for the feed (it was
-- overwritten, or a restart lost it) leaves `refusal`, its answer as JSON,
-- and no trace, so the feed is not asked for again.
CREATE TABLE feed_traces (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    feed INTEGER NOT NULL,
    fetched_ms INTEGER NOT NULL,
    summary TEXT,
    stored INTEGER,
    body BLOB,
    refusal TEXT,
    CHECK ((summary IS NULL) = (refusal IS NOT NULL)),
    PRIMARY KEY (feeder_id, epoch, feed)
) STRICT, WITHOUT ROWID;

-- The latest read of each statistics section, as JSON. `boot_count` is the
-- feeder's boot it was read in, when known: counters "since this boot"
-- start again at each.
CREATE TABLE feeder_stats (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    section TEXT NOT NULL,
    received_ms INTEGER NOT NULL,
    boot_count INTEGER,
    stats TEXT NOT NULL,
    PRIMARY KEY (feeder_id, section)
) STRICT, WITHOUT ROWID;

-- Every read of the food section, typed, for the food views and alerts:
-- the food beam's last reading (`present`, absent while unknown), its
-- low and refill transitions and the feeds since a refill, as the
-- feeder counts them.
CREATE TABLE food_readings (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    received_ms INTEGER NOT NULL,
    boot_count INTEGER,
    present INTEGER,
    ran_low INTEGER NOT NULL,
    refilled INTEGER NOT NULL,
    feeds_since_refill INTEGER
) STRICT;

CREATE INDEX food_readings_by_time ON food_readings (feeder_id, received_ms);
