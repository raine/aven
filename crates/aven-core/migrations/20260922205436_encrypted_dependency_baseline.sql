-- Local association state, excluded from portable snapshots and exports.
CREATE TABLE local_e2ee_dependency_baseline (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    association TEXT NOT NULL,
    sync_generation INTEGER NOT NULL,
    prefix_count INTEGER NOT NULL CHECK (prefix_count >= 0)
);

CREATE TABLE local_e2ee_dependency_edges (
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    depends_on_task_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, task_id, depends_on_task_id)
);
