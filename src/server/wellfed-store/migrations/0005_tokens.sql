-- Bearer tokens for phones and the CLI. Like a session's, a token is
-- stored as the SHA-256 of its 32 random bytes, so a copied database
-- holds no live token. A token is its user's, not a role's: the role is
-- read from `users` on each use. Deleting the user deletes their tokens;
-- an account past its expiry keeps them, refused until an owner extends
-- it.
CREATE TABLE tokens (
    id BLOB PRIMARY KEY NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    user_id BLOB NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    last_used_ms INTEGER,
    expires_ms INTEGER
) STRICT;

CREATE INDEX tokens_by_user ON tokens (user_id);
