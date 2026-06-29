-- FTS5 task search index using external content table.
-- task_search_documents holds one row per (workspace_id, task_id).
-- task_search_fts indexes that content for full-text search.

CREATE TABLE IF NOT EXISTS task_search_documents (
    doc_id INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    workspace_token TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    labels TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    project_key TEXT NOT NULL DEFAULT '',
    project_name TEXT NOT NULL DEFAULT '',
    project_prefix TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    priority TEXT NOT NULL DEFAULT '',
    UNIQUE(workspace_id, task_id)
);

CREATE VIRTUAL TABLE IF NOT EXISTS task_search_fts USING fts5(
    workspace_token,
    title,
    description,
    labels,
    notes,
    project_key,
    project_name,
    project_prefix,
    status,
    priority,
    content = 'task_search_documents',
    content_rowid = 'doc_id',
    tokenize = 'unicode61'
);

-- Backfill existing tasks
INSERT INTO task_search_documents(
    workspace_id, task_id, workspace_token, title, description,
    labels, notes, project_key, project_name, project_prefix, status, priority
)
SELECT
    t.workspace_id, t.id, t.workspace_id, t.title, t.description,
    COALESCE((
        SELECT group_concat(label, ' ')
        FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)
    ), ''),
    COALESCE((
        SELECT group_concat(body, ' ')
        FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)
    ), ''),
    p.key, p.name, p.prefix, t.status, t.priority
FROM tasks t
JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id;

INSERT INTO task_search_fts(task_search_fts) VALUES('rebuild');

-- Sync content table inserts to the FTS index
CREATE TRIGGER IF NOT EXISTS task_search_documents_ai AFTER INSERT ON task_search_documents BEGIN
    INSERT INTO task_search_fts(rowid, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    VALUES (new.doc_id, new.workspace_token, new.title, new.description, new.labels, new.notes, new.project_key, new.project_name, new.project_prefix, new.status, new.priority);
END;

-- Sync content table deletes to the FTS index
CREATE TRIGGER IF NOT EXISTS task_search_documents_bd BEFORE DELETE ON task_search_documents BEGIN
    DELETE FROM task_search_fts WHERE rowid = old.doc_id;
END;

-- Sync content table updates to the FTS index
CREATE TRIGGER IF NOT EXISTS task_search_documents_au AFTER UPDATE ON task_search_documents BEGIN
    INSERT INTO task_search_fts(
        task_search_fts, rowid, workspace_token, title, description,
        labels, notes, project_key, project_name, project_prefix, status,
        priority
    )
    VALUES (
        'delete', old.doc_id, old.workspace_token, old.title, old.description,
        old.labels, old.notes, old.project_key, old.project_name,
        old.project_prefix, old.status, old.priority
    );
    INSERT INTO task_search_fts(rowid, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    VALUES (new.doc_id, new.workspace_token, new.title, new.description, new.labels, new.notes, new.project_key, new.project_name, new.project_prefix, new.status, new.priority);
END;

-- Refresh document when a task is created
CREATE TRIGGER IF NOT EXISTS tasks_ai AFTER INSERT ON tasks BEGIN
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    VALUES (new.workspace_id, new.id, new.workspace_id, new.title, new.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = new.workspace_id AND tl.task_id = new.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = new.workspace_id AND n.task_id = new.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        IFNULL((SELECT key FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        IFNULL((SELECT name FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        IFNULL((SELECT prefix FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        new.status, new.priority);
END;

-- Refresh document when task search-relevant fields change
CREATE TRIGGER IF NOT EXISTS tasks_au AFTER UPDATE OF title, description, project_id, status, priority, deleted ON tasks BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    VALUES (new.workspace_id, new.id, new.workspace_id, new.title, new.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = new.workspace_id AND tl.task_id = new.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = new.workspace_id AND n.task_id = new.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        IFNULL((SELECT key FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        IFNULL((SELECT name FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        IFNULL((SELECT prefix FROM projects WHERE workspace_id = new.workspace_id AND id = new.project_id), ''),
        new.status, new.priority);
END;

-- Clean up document when a task is deleted
CREATE TRIGGER IF NOT EXISTS tasks_ad AFTER DELETE ON tasks BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.id;
END;

-- Refresh document when a label is added to a task
CREATE TRIGGER IF NOT EXISTS task_labels_ai AFTER INSERT ON task_labels BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = new.workspace_id AND task_id = new.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = new.workspace_id AND t.id = new.task_id;
END;

-- Refresh document when a label is removed from a task
CREATE TRIGGER IF NOT EXISTS task_labels_ad AFTER DELETE ON task_labels BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = old.workspace_id AND t.id = old.task_id;
END;

-- Refresh document when a label assignment is updated
CREATE TRIGGER IF NOT EXISTS task_labels_au AFTER UPDATE OF workspace_id, task_id, label ON task_labels BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = old.workspace_id AND t.id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = new.workspace_id AND t.id = new.task_id
      AND (new.workspace_id != old.workspace_id OR new.task_id != old.task_id);
END;

-- Refresh document when a note is added
CREATE TRIGGER IF NOT EXISTS notes_ai AFTER INSERT ON notes BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = new.workspace_id AND task_id = new.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = new.workspace_id AND t.id = new.task_id;
END;

-- Refresh document when a note is deleted
CREATE TRIGGER IF NOT EXISTS notes_ad AFTER DELETE ON notes BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = old.workspace_id AND t.id = old.task_id;
END;

-- Refresh document when a note body is updated
CREATE TRIGGER IF NOT EXISTS notes_au AFTER UPDATE OF workspace_id, task_id, body ON notes BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = old.workspace_id AND t.id = old.task_id;
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        p.key, p.name, p.prefix, t.status, t.priority
    FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
    WHERE t.workspace_id = new.workspace_id AND t.id = new.task_id
      AND (new.workspace_id != old.workspace_id OR new.task_id != old.task_id);
END;

-- Refresh all documents for a project when project metadata changes
CREATE TRIGGER IF NOT EXISTS projects_au AFTER UPDATE OF key, name, prefix ON projects BEGIN
    DELETE FROM task_search_documents WHERE task_id IN (SELECT id FROM tasks WHERE workspace_id = old.workspace_id AND project_id = old.id);
    INSERT INTO task_search_documents(workspace_id, task_id, workspace_token, title, description, labels, notes, project_key, project_name, project_prefix, status, priority)
    SELECT t.workspace_id, t.id, t.workspace_id, t.title, t.description,
        COALESCE((SELECT group_concat(label, ' ') FROM (SELECT label FROM task_labels tl WHERE tl.workspace_id = t.workspace_id AND tl.task_id = t.id ORDER BY tl.label)), ''),
        COALESCE((SELECT group_concat(body, ' ') FROM (SELECT body FROM notes n WHERE n.workspace_id = t.workspace_id AND n.task_id = t.id ORDER BY n.created_at DESC, n.id DESC)), ''),
        new.key, new.name, new.prefix, t.status, t.priority
    FROM tasks t
    WHERE t.workspace_id = new.workspace_id AND t.project_id = new.id;
END;
