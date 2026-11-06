-- Notifications this device has queued for the platform's notification
-- service, and how far each got. `tag` is the platform identifier, which a
-- click hands back. A row in `submitting` may or may not have reached the
-- service; the next delivery checks before posting it again.
CREATE TABLE local_notification_outbox (
    event TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    tag TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN ('pending', 'submitting', 'submitted', 'failed')),
    reason TEXT,
    updated_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE INDEX local_notification_outbox_state
    ON local_notification_outbox(state, updated_at);
