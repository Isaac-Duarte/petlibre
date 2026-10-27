-- Stock restores: an install whose target is the feeder's stock firmware,
-- from the backup the server keeps in its data directory (never in the
-- database). It shares the installs' one-at-a-time slot and their history,
-- so `target` names what an install writes (`release` or `stock`), and
-- `release_id` holds that target's name: the release's id, or the kept
-- backup's SHA-256.
ALTER TABLE installs ADD COLUMN target TEXT NOT NULL DEFAULT 'release';
