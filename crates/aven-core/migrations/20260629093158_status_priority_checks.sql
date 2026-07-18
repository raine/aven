DROP TRIGGER IF EXISTS tasks_ai;
DROP TRIGGER IF EXISTS tasks_au;
DROP TRIGGER IF EXISTS tasks_ad;
DROP TRIGGER IF EXISTS task_labels_ai;
DROP TRIGGER IF EXISTS task_labels_ad;
DROP TRIGGER IF EXISTS task_labels_au;
DROP TRIGGER IF EXISTS notes_ai;
DROP TRIGGER IF EXISTS notes_ad;
DROP TRIGGER IF EXISTS notes_au;
DROP TRIGGER IF EXISTS projects_au;

CREATE TABLE tasks_new (
    workspace_id TEXT NOT NULL DEFAULT '0000000000000000',
    id TEXT NOT NULL PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    project_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('inbox', 'backlog', 'todo', 'active', 'done', 'canceled')),
    priority TEXT NOT NULL CHECK (priority IN ('none', 'low', 'medium', 'high', 'urgent')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    queue_activity_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z',
    deleted INTEGER NOT NULL DEFAULT 0
);

INSERT INTO tasks_new (
    workspace_id, id, title, description, project_id, status, priority,
    created_at, updated_at, queue_activity_at, deleted
)
SELECT workspace_id, id, title, description, project_id, status, priority,
       created_at, updated_at, queue_activity_at, deleted
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;

CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project
ON tasks(workspace_id, project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_updated
ON tasks(workspace_id, deleted, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_status_updated
ON tasks(workspace_id, deleted, status, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_deleted_priority_updated
ON tasks(workspace_id, deleted, priority, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_updated
ON tasks(workspace_id, project_id, deleted, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace_project_deleted_status
ON tasks(workspace_id, project_id, deleted, status);

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

CREATE TRIGGER IF NOT EXISTS tasks_ad AFTER DELETE ON tasks BEGIN
    DELETE FROM task_search_documents WHERE workspace_id = old.workspace_id AND task_id = old.id;
END;

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
