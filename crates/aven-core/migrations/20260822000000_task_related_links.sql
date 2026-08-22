CREATE TABLE task_related_links (
    workspace_id TEXT NOT NULL,
    task_a_id TEXT NOT NULL,
    task_b_id TEXT NOT NULL,
    linked INTEGER NOT NULL CHECK (linked IN (0, 1)),
    last_change_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_a_id, task_b_id),
    CHECK (task_a_id < task_b_id),
    FOREIGN KEY (task_a_id) REFERENCES tasks(id),
    FOREIGN KEY (task_b_id) REFERENCES tasks(id),
    FOREIGN KEY (last_change_id) REFERENCES changes(change_id)
);

CREATE INDEX idx_task_related_links_reverse
    ON task_related_links(workspace_id, task_b_id, linked);
