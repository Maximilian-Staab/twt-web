-- twt-cloud schema v1 (design §5, grounded in app MySQLiteHelper DATABASE_VERSION 6)
--
-- Units (from DESIGN.md / DAO):
--   events.time        : epoch SECONDS (app stores seconds; DAO truncates to minutes only on read)
--   events.zone_offset : UTC offset in MINUTES (DAO: getTotalSeconds()/60)
--   targets.time       : epoch DAYS (a LocalDate)
--   tasks.ordering     : nullable
--   targets.value      : target work time in MINUTES

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- projected from IdP; no credentials stored
CREATE TABLE users (
    id         BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sub        TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- registered device installations; client_id is the sync-era identity
CREATE TABLE clients (
    client_id    UUID PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users (id),
    label        TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);

-- per-user change log; seq is allocated from a per-user counter so cursors
-- are dense and monotonic per user (see change_seq table below)
CREATE TABLE change_log (
    user_id     BIGINT NOT NULL REFERENCES users (id),
    seq         BIGINT NOT NULL,
    entity      TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    op          TEXT NOT NULL CHECK (op IN ('upsert', 'delete')),
    payload     JSONB NOT NULL,
    origin      UUID,
    occurred_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, seq)
);

CREATE TYPE event_type AS ENUM ('CLOCK_IN', 'CLOCK_OUT');
CREATE TYPE target_type AS ENUM ('DAY_SET', 'DAY_GRANT', 'DAY_IGNORE', 'FLEXI_SET', 'FLEXI_ADD');

-- Tasks: mirrors app tasks(_id INTEGER PK AUTOINCREMENT, name, active, ordering, isdefault)
-- UUID is canonical cross-device identity; the app's integer _id stays local to each device.
CREATE TABLE tasks (
    id         UUID PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users (id),
    name       TEXT NOT NULL,
    active     BOOLEAN NOT NULL DEFAULT FALSE,
    ordering   INTEGER,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ,
    -- integer id used by the app's backup CSV (`tasks._id`); assigned on CSV
    -- import, NULL for tasks created via the JSON API / sync
    local_id   INTEGER,
    UNIQUE (user_id, local_id)
);

-- Events: time epoch seconds, zone_offset MINUTES (never seconds).
-- task is a loose FK mirroring DAO.deleteTask's no-cascade behaviour; the app
-- tolerates dangling references and so do we (integrity converges via sync).
CREATE TABLE events (
    id          UUID PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users (id),
    time        BIGINT NOT NULL,
    zone_offset INTEGER NOT NULL,
    type        event_type NOT NULL,
    task        UUID NULL,
    comment     TEXT,
    updated_at  TIMESTAMPTZ NOT NULL,
    deleted_at  TIMESTAMPTZ
);

-- Targets: time is epoch DAYS (a LocalDate), value is target minutes.
CREATE TABLE targets (
    id         UUID PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users (id),
    time       INTEGER NOT NULL,
    type       target_type NOT NULL,
    value      INTEGER,
    comment    TEXT,
    updated_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ
);

-- Preferences: JSONB value mirrors the typed CSV (string, boolean, integer, ...).
CREATE TABLE preferences (
    user_id    BIGINT NOT NULL REFERENCES users (id),
    key        TEXT NOT NULL,
    value      JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, key)
);

-- Push idempotency ledger: (client_id, op_id) seen => duplicate.
CREATE TABLE processed_ops (
    client_id UUID NOT NULL,
    op_id     TEXT NOT NULL,
    result    TEXT NOT NULL,
    PRIMARY KEY (client_id, op_id)
);

CREATE INDEX idx_events_user_time ON events (user_id, time);
CREATE INDEX idx_events_user_deleted ON events (user_id, deleted_at);
CREATE INDEX idx_targets_user_time ON targets (user_id, time);
CREATE INDEX idx_changelog_user_seq ON change_log (user_id, seq);

-- Per-user change-log head. `UPDATE ... RETURNING` allocates the next seq
-- atomically under concurrent pushes for the same user.
CREATE TABLE change_seq (
    user_id BIGINT PRIMARY KEY REFERENCES users (id),
    seq     BIGINT NOT NULL DEFAULT 0
);
-- Events and targets also carry a stable UUID as PK; their per-user integer
-- `local_id` is only reconstructable from the app's backup CSV.
