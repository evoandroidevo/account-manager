-- Local mirror of LLDAP accounts. LLDAP stays the source of truth for
-- email, display name, groups and passwords; this table only anchors
-- foreign keys for entitlements and service accounts.
CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_login_at TEXT,
    disabled_at TEXT
);

CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    occurred_at TEXT NOT NULL,
    actor TEXT,
    action TEXT NOT NULL,
    target TEXT,
    metadata_json TEXT
);

CREATE INDEX audit_log_occurred_at_idx ON audit_log (occurred_at);
