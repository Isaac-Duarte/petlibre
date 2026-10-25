-- A feeder's event numbers count within its ring's epoch, which a new ring
-- draws at random: a ring re-created by a storage wipe counts from 1 again
-- under another epoch. The epoch joins every event key, and `epoch` is the
-- one `synced` counts in. Everything mirrored before came from rings
-- written before epochs, which read as epoch 0.

ALTER TABLE feeders ADD COLUMN epoch INTEGER;
UPDATE feeders SET epoch = 0 WHERE synced IS NOT NULL;

CREATE TABLE events_by_epoch (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    at_s INTEGER,
    received_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    body BLOB NOT NULL,
    PRIMARY KEY (feeder_id, epoch, seq)
) STRICT, WITHOUT ROWID;
INSERT INTO events_by_epoch (feeder_id, epoch, seq, at_s, received_ms, kind, body)
    SELECT feeder_id, 0, seq, at_s, received_ms, kind, body FROM events;
DROP TABLE events;
ALTER TABLE events_by_epoch RENAME TO events;
CREATE INDEX events_by_kind ON events (feeder_id, epoch, kind, seq);

CREATE TABLE gaps_by_epoch (
    feeder_id BLOB NOT NULL REFERENCES feeders (id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL,
    from_seq INTEGER NOT NULL,
    to_seq INTEGER NOT NULL,
    received_ms INTEGER NOT NULL,
    PRIMARY KEY (feeder_id, epoch, from_seq)
) STRICT, WITHOUT ROWID;
INSERT INTO gaps_by_epoch (feeder_id, epoch, from_seq, to_seq, received_ms)
    SELECT feeder_id, 0, from_seq, to_seq, received_ms FROM gaps;
DROP TABLE gaps;
ALTER TABLE gaps_by_epoch RENAME TO gaps;
