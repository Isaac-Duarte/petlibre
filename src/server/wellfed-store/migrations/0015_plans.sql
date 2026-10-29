-- The feeding plans last read from each feeder, as JSON, and when: the
-- next meal is reckoned from them while the feeder is offline.
ALTER TABLE feeder_state ADD COLUMN plans TEXT;
ALTER TABLE feeder_state ADD COLUMN plans_ms INTEGER;
