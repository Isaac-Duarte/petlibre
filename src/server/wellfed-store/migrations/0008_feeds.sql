-- Feeds in the command journal. `history_epoch` and `history_seq` are the
-- newest event the server held of the feeder's history when the write was
-- sent (both null when it held none): a feed whose answer was lost can only
-- have ended in a `Fed` event after it. `feed` is the feeder's number for
-- a feed, from its answer or its `Fed` event, and the index finds a feed's
-- entry by it.
ALTER TABLE commands ADD COLUMN history_epoch INTEGER;
ALTER TABLE commands ADD COLUMN history_seq INTEGER;

CREATE INDEX commands_by_feed ON commands (feeder_id, feed);
