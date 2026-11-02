-- Change-log retention (openapi.yaml /sync/changes 410, DESIGN §7.1).
--
-- Purged change-log rows would be silently invisible to a cursor-based
-- client: it would believe itself up to date while an entire prefix of its
-- history never arrived. The watermark closes that hole. purge_expired()
-- deletes expired rows and records, per user, the highest seq it removed;
-- GET /sync/changes compares non-zero cursors against it and answers 410
-- when the cursor fell inside the purged range.

CREATE TABLE change_retention (
    user_id           BIGINT PRIMARY KEY REFERENCES users (id),
    -- high-water mark: every change_log row with seq <= this value is purged
    purged_through_seq BIGINT NOT NULL DEFAULT 0,
    purged_at         TIMESTAMPTZ
);

CREATE INDEX idx_changelog_user_occurred ON change_log (user_id, occurred_at);
