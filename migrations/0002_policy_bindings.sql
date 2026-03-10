-- Policy bindings: maps a Keycloak group or role to a Matrix room or space.
CREATE TABLE IF NOT EXISTS policy_bindings (
    id              TEXT    PRIMARY KEY NOT NULL,
    subject_type    TEXT    NOT NULL CHECK (subject_type IN ('group', 'role')),
    subject_value   TEXT    NOT NULL,
    target_type     TEXT    NOT NULL CHECK (target_type IN ('room', 'space')),
    target_room_id  TEXT    NOT NULL,
    power_level     INTEGER,
    allow_remove    INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    UNIQUE (subject_type, subject_value, target_room_id)
);

-- Cached room metadata for the policy UI. Reconciliation uses room_id only.
CREATE TABLE IF NOT EXISTS policy_targets_cache (
    room_id         TEXT    PRIMARY KEY NOT NULL,
    name            TEXT,
    canonical_alias TEXT,
    parent_space_id TEXT,
    is_space        INTEGER NOT NULL DEFAULT 0,
    last_seen_at    TEXT    NOT NULL
);

-- Tracks one-time bootstrap import from GROUP_MAPPINGS env/file.
CREATE TABLE IF NOT EXISTS policy_bootstrap_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    bootstrap_source    TEXT    NOT NULL,
    bootstrap_version   INTEGER NOT NULL,
    bootstrapped_at     TEXT    NOT NULL
);
