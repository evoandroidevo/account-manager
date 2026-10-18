-- Pending email address changes, confirmed by a link sent to the new address.
CREATE TABLE email_changes (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    new_email TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT
);

CREATE INDEX email_changes_user_id_idx ON email_changes (user_id);

-- Unix seconds of the last password change. Sessions created before it
-- are rejected, which signs out every other device.
ALTER TABLE users ADD COLUMN password_changed_at INTEGER;
