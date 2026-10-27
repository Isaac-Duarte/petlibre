-- Samples carry a number beside their raw bytes, and hourly rollups of
-- those numbers outlive the raw rows.
--
-- `id` is AUTOINCREMENT so an id is never reused, even after retention
-- empties the table: the rollups' mark and the page cursor rely on it.
-- `number` is the one figure the metric is charted by, decoded once at
-- ingest; NULL when the sample has none (no battery reading, no gateway
-- round trip). Rows from before this migration are copied with none, and
-- the start's refresh decodes them (`sample_marks.version` 0).

CREATE TABLE samples_numbered (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    received_ms INTEGER NOT NULL,
    uptime_ms INTEGER NOT NULL,
    metric TEXT NOT NULL,
    number INTEGER,
    value BLOB NOT NULL
) STRICT;

INSERT INTO samples_numbered (feeder_id, received_ms, uptime_ms, metric, value)
SELECT feeder_id, received_ms, uptime_ms, metric, value FROM samples ORDER BY received_ms, rowid;

DROP TABLE samples;
ALTER TABLE samples_numbered RENAME TO samples;

CREATE INDEX samples_by_metric ON samples (feeder_id, metric, received_ms);

-- One row per feeder, metric and UTC hour (`hour_ms`, its first
-- millisecond) over the samples that carry a number. The mean is
-- `sum / count`. Kept forever.
CREATE TABLE sample_rollups (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    metric TEXT NOT NULL,
    hour_ms INTEGER NOT NULL,
    count INTEGER NOT NULL,
    min INTEGER NOT NULL,
    max INTEGER NOT NULL,
    sum INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, metric, hour_ms)
) STRICT, WITHOUT ROWID;

-- `rolled`: every sample with an id up to it is in the rollups, and none
-- above it is. `version`: the sample-number code the stored numbers were
-- decoded by.
CREATE TABLE sample_marks (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    rolled INTEGER NOT NULL,
    version INTEGER NOT NULL
) STRICT;

INSERT INTO sample_marks (id, rolled, version) VALUES (0, 0, 0);
