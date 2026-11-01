CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS members (
    project_id TEXT NOT NULL REFERENCES projects(id),
    device_id TEXT NOT NULL,
    role TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, device_id)
);
CREATE TABLE IF NOT EXISTS agent_sessions (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);
CREATE TABLE IF NOT EXISTS tasks (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    status TEXT NOT NULL,
    device_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);
CREATE TABLE IF NOT EXISTS attempts (
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, id),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id)
);
CREATE TABLE IF NOT EXISTS messages (
    project_id TEXT NOT NULL REFERENCES projects(id),
    id TEXT NOT NULL,
    source_device TEXT NOT NULL,
    target_device TEXT NOT NULL,
    target_session TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, id)
);
CREATE TABLE IF NOT EXISTS artifacts (
    project_id TEXT NOT NULL,
    id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, id),
    FOREIGN KEY (project_id, task_id) REFERENCES tasks(project_id, id),
    FOREIGN KEY (project_id, attempt_id) REFERENCES attempts(project_id, id)
);
CREATE TABLE IF NOT EXISTS receipts (
    actor TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    digest TEXT NOT NULL,
    response TEXT NOT NULL,
    PRIMARY KEY (actor, operation_id)
);
CREATE TABLE IF NOT EXISTS events (
    project_id TEXT NOT NULL REFERENCES projects(id),
    sequence INTEGER NOT NULL,
    kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    task_id TEXT,
    target_device TEXT,
    data TEXT NOT NULL,
    PRIMARY KEY (project_id, sequence)
);
CREATE INDEX IF NOT EXISTS tasks_by_device ON tasks(project_id, device_id);
CREATE INDEX IF NOT EXISTS sessions_by_device ON agent_sessions(project_id, device_id);
CREATE INDEX IF NOT EXISTS messages_by_target ON messages(project_id, target_device, target_session);
CREATE INDEX IF NOT EXISTS attempts_by_task ON attempts(project_id, task_id);
CREATE INDEX IF NOT EXISTS artifacts_by_task ON artifacts(project_id, task_id);
PRAGMA user_version = 1;
