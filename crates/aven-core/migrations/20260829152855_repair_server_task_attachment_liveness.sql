DELETE FROM server_task_tombstones;

INSERT INTO server_task_tombstones(workspace_id, task_id, deleted)
SELECT workspace_id, task_id, deleted
FROM (
    SELECT
        json_extract(payload, '$.workspace_id') AS workspace_id,
        entity_id AS task_id,
        CASE json_extract(payload, '$.value') WHEN '1' THEN 1 ELSE 0 END AS deleted,
        ROW_NUMBER() OVER (
            PARTITION BY json_extract(payload, '$.workspace_id'), entity_id
            ORDER BY server_seq DESC
        ) AS deletion_rank
    FROM changes
    WHERE server_seq IS NOT NULL
      AND entity_type = 'task'
      AND op_type IN ('set_field', 'resolve_field')
      AND field = 'deleted'
      AND json_type(payload, '$.workspace_id') = 'text'
)
WHERE deletion_rank = 1;

UPDATE blob_lifecycle
SET unreferenced_at = NULL
WHERE EXISTS (
    SELECT 1
    FROM task_attachments ta
    JOIN tasks t
      ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
    WHERE ta.sha256 = blob_lifecycle.sha256
      AND ta.deleted = 0
      AND t.deleted = 0
) OR EXISTS (
    SELECT 1
    FROM server_blob_references sbr
    LEFT JOIN server_task_tombstones st
      ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
    WHERE sbr.sha256 = blob_lifecycle.sha256
      AND sbr.deleted = 0
      AND COALESCE(st.deleted, 0) = 0
);

UPDATE blob_lifecycle
SET unreferenced_at = COALESCE(
    unreferenced_at,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
)
WHERE NOT (
    EXISTS (
        SELECT 1
        FROM task_attachments ta
        JOIN tasks t
          ON t.workspace_id = ta.workspace_id AND t.id = ta.task_id
        WHERE ta.sha256 = blob_lifecycle.sha256
          AND ta.deleted = 0
          AND t.deleted = 0
    ) OR EXISTS (
        SELECT 1
        FROM server_blob_references sbr
        LEFT JOIN server_task_tombstones st
          ON st.workspace_id = sbr.workspace_id AND st.task_id = sbr.task_id
        WHERE sbr.sha256 = blob_lifecycle.sha256
          AND sbr.deleted = 0
          AND COALESCE(st.deleted, 0) = 0
    )
);
