CREATE TABLE invites (
    id TEXT PRIMARY KEY NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    groups_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    consumed_by TEXT,
    revoked_at TEXT
);

CREATE INDEX invites_expires_at_idx ON invites (expires_at);

CREATE TABLE invite_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invite_id TEXT NOT NULL REFERENCES invites (id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    actor TEXT,
    metadata_json TEXT
);

CREATE INDEX invite_events_invite_id_idx ON invite_events (invite_id, id);