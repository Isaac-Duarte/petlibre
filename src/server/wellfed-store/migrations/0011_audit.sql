-- The audit log: who did what through the server. A row is either an
-- account change (`action`, the typed change as JSON) or a write in the
-- command journal (`command_seq`), whose state and outcome are read from
-- the journal, so they follow it until it settles. `seq` orders the log
-- and is its cursor. `user_id` names no foreign key and `user_name` keeps
-- the name it had: the log outlives the accounts it names. Kept forever,
-- like the journal.
CREATE TABLE audit (
    seq INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    user_id BLOB,
    user_name TEXT,
    token_id BLOB,
    action TEXT,
    command_seq INTEGER UNIQUE REFERENCES commands (seq) ON DELETE CASCADE,
    CHECK ((action IS NULL) != (command_seq IS NULL))
) STRICT;

CREATE INDEX audit_by_user ON audit (user_id, seq);

-- Every journal entry joins the log as it is recorded, in the same
-- statement, so no write reaches a feeder without its audit row. A row the
-- journal drops (a write that was never sent) leaves the log with it.
CREATE TRIGGER commands_audit AFTER INSERT ON commands
BEGIN
    INSERT INTO audit (at_ms, user_id, user_name, token_id, command_seq)
    VALUES (
        NEW.sent_ms,
        NEW.user_id,
        (SELECT name FROM users WHERE id = NEW.user_id),
        NEW.token_id,
        NEW.seq
    );
END;

-- The journal so far, in its order.
INSERT INTO audit (at_ms, user_id, user_name, token_id, command_seq)
SELECT commands.sent_ms, commands.user_id, users.name, commands.token_id, commands.seq
FROM commands LEFT JOIN users ON users.id = commands.user_id
ORDER BY commands.seq;
