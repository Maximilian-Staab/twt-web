-- Tie-break for same-time events.
--
-- The app's DAO reads events ORDER BY (time, _id) (DAO.getEventsWithParameters),
-- and worked-time computation is order-sensitive when two events share a
-- timestamp: a stray OUT is ignored while a stray IN keeps the session open.
-- The backend therefore needs a deterministic per-user insertion order that
-- matches the app's _id order.
--
-- change_log.seq is allocated from the per-user monotonic counter (change_seq)
-- at push time, in the app's _id order, so MIN(seq) per event reproduces the
-- app's insertion order for rows that arrived via sync. Every INSERT site
-- allocates a fresh seq at insert time (see routes/{events,sync,backup,snapshot}.rs).

ALTER TABLE events ADD COLUMN insertion_seq BIGINT;

UPDATE events e
SET insertion_seq = (
    SELECT MIN(c.seq) FROM change_log c
    WHERE c.user_id = e.user_id AND c.entity = 'event' AND c.entity_id = e.id::text
);

-- Fails loudly if any event has no change_log row (none exist as of 2026-10-14).
ALTER TABLE events ALTER COLUMN insertion_seq SET NOT NULL;

DROP INDEX idx_events_user_time;
CREATE INDEX idx_events_user_time ON events (user_id, time, insertion_seq);
