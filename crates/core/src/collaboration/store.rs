use super::model::*;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    fmt,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicU64},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MAX_REPLY_BYTES: usize = 768 * 1024;
const MAX_ENTITY_BYTES: usize = 256 * 1024;

pub struct Store {
    connection: Mutex<Connection>,
    local_device: String,
    data_dir: PathBuf,
    pub delivery_epoch: AtomicU64,
}

#[derive(Debug)]
struct Rejection {
    code: &'static str,
    message: String,
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Rejection {}

fn reject<T>(code: &'static str, message: impl Into<String>) -> Result<T> {
    Err(Rejection {
        code,
        message: message.into(),
    }
    .into())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn encode(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn bounded(value: &impl Serialize) -> Result<String> {
    let data = encode(value)?;
    if data.len() > MAX_ENTITY_BYTES {
        return reject("payload_too_large", "entity exceeds 256 KiB");
    }
    Ok(data)
}

fn load<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Option<T>> {
    let data: Option<String> = conn.query_row(sql, args, |row| row.get(0)).optional()?;
    data.map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

fn rows<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare(sql)?;
    let mut result = Vec::new();
    let mut total = 0;
    let mut cursor = statement.query(args)?;
    while let Some(row) = cursor.next()? {
        let data: String = row.get(0)?;
        total += data.len();
        if total > MAX_REPLY_BYTES {
            return reject(
                "response_too_large",
                "result exceeds response limit; use an entity lookup or event cursor",
            );
        }
        result.push(serde_json::from_str(&data)?);
    }
    Ok(result)
}

impl Store {
    pub fn open(data_dir: &Path, local_device: &str) -> Result<Self> {
        anyhow::ensure!(
            !local_device.is_empty(),
            "local device identity cannot be empty"
        );
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("collaboration.sqlite");
        let mut connection = Connection::open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;",
        )?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: u32 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        anyhow::ensure!(
            version <= 1,
            "unsupported collaboration schema version {version}"
        );
        tx.execute_batch(include_str!("storage/schema.sql"))?;
        tx.commit()?;
        Ok(Self {
            connection: Mutex::new(connection),
            local_device: local_device.to_owned(),
            data_dir: data_dir.to_owned(),
            delivery_epoch: AtomicU64::new(0),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn process(&self, actor: &str, request: &Request) -> Reply {
        match self.process_inner(actor, request) {
            Ok(reply) => reply,
            Err(error) => match error.downcast_ref::<Rejection>() {
                Some(error) => Reply::error(error.code, &error.message),
                None => {
                    tracing::error!(error = %error, "collaboration database operation failed");
                    Reply::error(
                        "storage_error",
                        "collaboration operation could not be committed",
                    )
                }
            },
        }
    }

    fn process_inner(&self, actor: &str, request: &Request) -> Result<Reply> {
        if actor.is_empty()
            || actor.len() > 256
            || request.request_id.is_empty()
            || request.request_id.len() > 256
        {
            return reject(
                "invalid_request",
                "device and request identifiers must be nonempty and at most 256 bytes",
            );
        }
        let encoded = bounded(request)?;
        let digest = blake3::hash(encoded.as_bytes()).to_hex().to_string();
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("store mutex poisoned"))?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let role = self.authorize(&tx, actor, request)?;
        // Authorization deliberately precedes replay: revoked devices and sessions cannot
        // retrieve previously committed responses through an existing connection.
        if !request.action.is_read() {
            let receipt: Option<(String, String, String)> = tx.query_row(
                "SELECT digest, response, project_id FROM receipts WHERE actor = ?1 AND operation_id = ?2",
                params![actor, request.request_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            if let Some((old_digest, response, project)) = receipt {
                if old_digest != digest {
                    return reject(
                        "idempotency_conflict",
                        "request_id was already used with different content",
                    );
                }
                if !project.is_empty() {
                    member_role(&tx, &project, actor)?;
                }
                let reply: Reply = serde_json::from_str(&response)?;
                match &reply.response {
                    Some(Response::Task { task }) => {
                        let role = member_role(&tx, &project, actor)?;
                        visible_task(&tx, &project, actor, role, &task.id)?;
                    }
                    Some(Response::Registered { session, token }) => {
                        authenticated_session(
                            &tx,
                            &project,
                            actor,
                            Some(&SessionAuth {
                                session_id: session.id.clone(),
                                token: token.clone(),
                            }),
                        )?;
                    }
                    _ => {}
                }
                return Ok(reply);
            }
        }
        // A rejected action must not leave any partial writes, while its stable business
        // response is still retained so that retrying the operation cannot change meaning.
        tx.execute_batch("SAVEPOINT action")?;
        let reply = match self.dispatch(&tx, actor, request, role) {
            Ok(response) => Reply::ok(response),
            Err(error) => {
                tx.execute_batch("ROLLBACK TO action")?;
                match error.downcast_ref::<Rejection>() {
                    Some(error) => Reply::error(error.code, &error.message),
                    None => return Err(error),
                }
            }
        };
        let encoded_reply = encode(&reply)?;
        if encoded_reply.len() > MAX_REPLY_BYTES {
            return reject("response_too_large", "response exceeds 768 KiB");
        }
        tx.execute_batch("RELEASE action")?;
        if !request.action.is_read() {
            let project = match &reply.response {
                Some(Response::Project { project }) => project.id.as_str(),
                _ => request.project_id.as_deref().unwrap_or_default(),
            };
            tx.execute(
                "INSERT INTO receipts(actor, operation_id, project_id, digest, response) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![actor, request.request_id, project, digest, encoded_reply],
            )?;
        }
        tx.commit()?;
        Ok(reply)
    }

    fn authorize(&self, conn: &Connection, actor: &str, request: &Request) -> Result<Option<Role>> {
        match &request.action {
            Action::ProjectCreate { .. } => {
                if actor != self.local_device {
                    return reject("forbidden", "only the local device may create a project");
                }
                if request.project_id.is_some() || request.auth.is_some() {
                    return reject(
                        "invalid_request",
                        "project creation cannot reference an existing project or session",
                    );
                }
                return Ok(None);
            }
            Action::ProjectsList => {
                if request.project_id.is_some() || request.auth.is_some() {
                    return reject(
                        "invalid_request",
                        "project listing cannot reference a project or session",
                    );
                }
                return Ok(None);
            }
            _ => {}
        }
        let project = project_id(request)?;
        let role = member_role(conn, project, actor)?;
        match &request.action {
            Action::MemberSet { .. } if role != Role::Admin => {
                return reject("forbidden", "admin role required");
            }
            Action::TaskCreate { .. }
            | Action::TaskAssign { .. }
            | Action::TaskCancel { .. }
            | Action::TaskAccept { .. }
            | Action::TaskRequeue { .. }
                if !matches!(role, Role::Admin | Role::Dispatcher) =>
            {
                return reject("forbidden", "dispatcher or admin role required");
            }
            action if !action.is_read() && role == Role::Viewer => {
                return reject("forbidden", "viewer role is read-only");
            }
            _ => {}
        }
        if matches!(request.action, Action::ArtifactReceipt { .. }) && actor != self.local_device {
            return reject(
                "forbidden",
                "only the coordinator's local file verifier may acknowledge artifacts",
            );
        }
        let requires_session = matches!(
            request.action,
            Action::SessionHeartbeat { .. }
                | Action::TaskClaim { .. }
                | Action::TaskProgress { .. }
                | Action::TaskResult { .. }
                | Action::TaskStopAck { .. }
                | Action::MessageSend { .. }
                | Action::MessageDelivered { .. }
                | Action::MessageConsume { .. }
                | Action::Inbox
                | Action::ArtifactDeclare { .. }
        );
        if requires_session || request.auth.is_some() {
            authenticated_session(conn, project, actor, request.auth.as_ref())?;
        }
        Ok(Some(role))
    }

    fn dispatch(
        &self,
        tx: &Transaction<'_>,
        actor: &str,
        request: &Request,
        role: Option<Role>,
    ) -> Result<Response> {
        if let Action::ProjectCreate { name } = &request.action {
            nonempty(name, "project name")?;
            let project = Project {
                id: new_id(),
                name: name.clone(),
                coordinator_id: self.local_device.clone(),
                created_at: now(),
            };
            tx.execute(
                "INSERT INTO projects(id, data) VALUES (?1, ?2)",
                params![project.id, bounded(&project)?],
            )?;
            save_member(
                tx,
                &project.id,
                &Member {
                    device_id: actor.to_owned(),
                    role: Role::Admin,
                    revoked: false,
                },
            )?;
            event(
                tx,
                &project.id,
                "project_created",
                &project.id,
                None,
                None,
                json!(&project),
            )?;
            return Ok(Response::Project { project });
        }
        if matches!(request.action, Action::ProjectsList) {
            return Ok(Response::Projects {
                projects: rows(
                    tx,
                    "SELECT p.data FROM projects p JOIN members m ON p.id = m.project_id WHERE m.device_id = ?1 AND m.revoked = 0 ORDER BY p.id",
                    [actor],
                )?,
            });
        }
        let project = project_id(request)?;
        let role = role.context("missing checked role")?;
        match &request.action {
            Action::ProjectCreate { .. } | Action::ProjectsList => unreachable!(),
            Action::ProjectGet => Ok(Response::Project {
                project: load(tx, "SELECT data FROM projects WHERE id = ?1", [project])?
                    .context("project missing")?,
            }),
            Action::MembersList => Ok(Response::Members {
                members: rows(
                    tx,
                    "SELECT data FROM members WHERE project_id = ?1 ORDER BY device_id",
                    [project],
                )?,
            }),
            Action::MemberSet {
                device_id,
                role,
                revoked,
            } => {
                nonempty(device_id, "member device")?;
                let previous: Option<Member> = load(
                    tx,
                    "SELECT data FROM members WHERE project_id = ?1 AND device_id = ?2",
                    params![project, device_id],
                )?;
                if previous
                    .as_ref()
                    .is_some_and(|member| member.role == Role::Admin && !member.revoked)
                    && (*revoked || *role != Role::Admin)
                {
                    let admins: i64 = tx.query_row("SELECT count(*) FROM members WHERE project_id = ?1 AND role = 'admin' AND revoked = 0", [project], |row| row.get(0))?;
                    if admins <= 1 {
                        return reject("invalid_state", "cannot remove the last active admin");
                    }
                }
                let member = Member {
                    device_id: device_id.clone(),
                    role: *role,
                    revoked: *revoked,
                };
                save_member(tx, project, &member)?;
                if *revoked {
                    let sessions: Vec<AgentSession> = rows(
                        tx,
                        "SELECT data FROM agent_sessions WHERE project_id = ?1 AND device_id = ?2",
                        params![project, device_id],
                    )?;
                    for mut session in sessions {
                        session.available = false;
                        tx.execute("UPDATE agent_sessions SET token_hash = '', data = ?3 WHERE project_id = ?1 AND id = ?2", params![project, session.id, encode(&session)?])?;
                    }
                }
                event(
                    tx,
                    project,
                    "member_updated",
                    device_id,
                    None,
                    Some(device_id),
                    json!(member),
                )?;
                Ok(Response::Ack)
            }
            Action::SessionRegister {
                name,
                mode,
                terminal,
                cwd,
            } => {
                nonempty(name, "session name")?;
                if terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.host_device_id != actor)
                {
                    return reject(
                        "forbidden",
                        "terminal host must match the authenticated device",
                    );
                }
                let session = AgentSession {
                    id: new_id(),
                    project_id: project.to_owned(),
                    device_id: actor.to_owned(),
                    name: name.clone(),
                    mode: *mode,
                    terminal: terminal.clone(),
                    cwd: cwd.clone(),
                    available: true,
                    last_seen: now(),
                };
                let token = format!("{}{}", new_id(), new_id());
                let hash = blake3::hash(token.as_bytes()).to_hex().to_string();
                tx.execute("INSERT INTO agent_sessions(project_id, id, device_id, token_hash, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![project, session.id, actor, hash, bounded(&session)?])?;
                event(
                    tx,
                    project,
                    "session_registered",
                    &session.id,
                    None,
                    Some(actor),
                    json!(session),
                )?;
                Ok(Response::Registered { session, token })
            }
            Action::SessionsList => Ok(Response::Sessions {
                sessions: rows(
                    tx,
                    "SELECT s.data FROM agent_sessions s JOIN members m ON m.project_id = s.project_id AND m.device_id = s.device_id WHERE s.project_id = ?1 AND m.revoked = 0 ORDER BY s.id",
                    [project],
                )?,
            }),
            Action::SessionHeartbeat { available } => {
                let mut session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                session.available = *available;
                session.last_seen = now();
                tx.execute(
                    "UPDATE agent_sessions SET data = ?3 WHERE project_id = ?1 AND id = ?2",
                    params![project, session.id, bounded(&session)?],
                )?;
                event(
                    tx,
                    project,
                    "session_updated",
                    &session.id,
                    None,
                    Some(actor),
                    json!(session),
                )?;
                Ok(Response::Ack)
            }
            Action::TaskCreate { spec } => {
                nonempty(&spec.title, "task title")?;
                nonempty(&spec.goal, "task goal")?;
                target_session(tx, project, &spec.assignee)?;
                let task = Task {
                    id: new_id(),
                    project_id: project.to_owned(),
                    creator_device: actor.to_owned(),
                    creator_session: request.auth.as_ref().map(|auth| auth.session_id.clone()),
                    spec: spec.clone(),
                    state: TaskState::Queued,
                    version: 1,
                    attempts: Vec::new(),
                    progress: None,
                    created_at: now(),
                    updated_at: now(),
                };
                tx.execute("INSERT INTO tasks(project_id, id, version, status, device_id, session_id, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![project, task.id, i64::try_from(task.version)?, state_name(task.state), task.spec.assignee.device_id, task.spec.assignee.session_id, bounded(&task)?])?;
                task_event(tx, "task_created", &task)?;
                Ok(Response::Task { task })
            }
            Action::TasksList => {
                let tasks = if role == Role::Worker {
                    rows(
                        tx,
                        "SELECT data FROM tasks WHERE project_id = ?1 AND device_id = ?2 ORDER BY id",
                        params![project, actor],
                    )?
                } else {
                    rows(
                        tx,
                        "SELECT data FROM tasks WHERE project_id = ?1 ORDER BY id",
                        [project],
                    )?
                };
                Ok(Response::Tasks { tasks })
            }
            Action::TaskGet { task_id } => Ok(Response::Task {
                task: visible_task(tx, project, actor, role, task_id)?,
            }),
            Action::TaskAssign {
                task_id,
                assignee,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                check_version(&task, *expected_version)?;
                require_state(&task, &[TaskState::Queued])?;
                target_session(tx, project, assignee)?;
                task.spec.assignee = assignee.clone();
                save_task(tx, &mut task, "task_assigned")?;
                Ok(Response::Task { task })
            }
            Action::TaskClaim {
                task_id,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                let session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                require_assignee(&task, &session)?;
                check_version(&task, *expected_version)?;
                require_state(&task, &[TaskState::Queued])?;
                if !session.available {
                    return reject("session_unavailable", "session is not available");
                }
                let attempt = Attempt {
                    id: new_id(),
                    assignee: task.spec.assignee.clone(),
                    started_at: now(),
                    last_seen: now(),
                    stopped: false,
                    result: None,
                };
                task.attempts.push(attempt);
                task.state = TaskState::Running;
                save_task(tx, &mut task, "task_claimed")?;
                Ok(Response::Task { task })
            }
            Action::TaskProgress {
                task_id,
                attempt_id,
                message,
                blocked,
                expected_version,
            } => {
                text_limit(message)?;
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                execution_attempt(tx, &task, actor, request.auth.as_ref(), attempt_id)?;
                check_version(&task, *expected_version)?;
                require_state(&task, &[TaskState::Running, TaskState::Blocked])?;
                task.state = if *blocked {
                    TaskState::Blocked
                } else {
                    TaskState::Running
                };
                task.progress = Some(message.clone());
                task.attempts
                    .last_mut()
                    .context("attempt missing")?
                    .last_seen = now();
                save_task(tx, &mut task, "task_progress")?;
                Ok(Response::Task { task })
            }
            Action::TaskResult {
                task_id,
                attempt_id,
                result,
                expected_version,
            } => {
                nonempty(&result.summary, "result summary")?;
                bounded(result)?;
                let session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                let historical: Option<Task> = load(
                    tx,
                    "SELECT data FROM tasks WHERE project_id = ?1 AND id = ?2",
                    params![project, task_id],
                )?;
                if let Some(task) = historical {
                    if let Some(attempt) = task
                        .attempts
                        .iter()
                        .find(|attempt| attempt.id == *attempt_id)
                    {
                        if attempt.assignee.device_id != actor
                            || attempt.assignee.session_id != session.id
                        {
                            return reject("forbidden", "attempt belongs to another session");
                        }
                        if attempt.stopped
                            || task
                                .attempts
                                .last()
                                .is_none_or(|current| current.id != *attempt_id)
                        {
                            event(
                                tx,
                                project,
                                "task_late_result",
                                task_id,
                                Some(task_id),
                                Some(actor),
                                json!({ "task_id": task_id, "attempt_id": attempt_id, "result": result }),
                            )?;
                            return Ok(Response::Ack);
                        }
                    }
                }
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                execution_attempt(tx, &task, actor, request.auth.as_ref(), attempt_id)?;
                check_version(&task, *expected_version)?;
                require_state(
                    &task,
                    &[
                        TaskState::Running,
                        TaskState::Blocked,
                        TaskState::CancelRequested,
                    ],
                )?;
                for artifact_id in &result.artifact_ids {
                    let artifact = get_artifact(tx, project, artifact_id)?;
                    if artifact.task_id != task.id || artifact.attempt_id != *attempt_id {
                        return reject(
                            "invalid_artifact",
                            "result artifact must belong to this task and attempt",
                        );
                    }
                }
                let completed_after_cancel = task.state == TaskState::CancelRequested;
                let attempt = task.attempts.last_mut().context("attempt missing")?;
                attempt.result = Some(result.clone());
                attempt.stopped = true;
                attempt.last_seen = now();
                task.state = if result.failed {
                    TaskState::Failed
                } else {
                    TaskState::ResultReady
                };
                save_task(
                    tx,
                    &mut task,
                    if completed_after_cancel {
                        "task_result_after_cancel"
                    } else {
                        "task_result"
                    },
                )?;
                Ok(Response::Task { task })
            }
            Action::TaskCancel {
                task_id,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                check_version(&task, *expected_version)?;
                require_state(
                    &task,
                    &[TaskState::Queued, TaskState::Running, TaskState::Blocked],
                )?;
                task.state = if task.state == TaskState::Queued {
                    TaskState::Cancelled
                } else {
                    TaskState::CancelRequested
                };
                save_task(tx, &mut task, "task_cancel_requested")?;
                Ok(Response::Task { task })
            }
            Action::TaskStopAck {
                task_id,
                attempt_id,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                execution_attempt(tx, &task, actor, request.auth.as_ref(), attempt_id)?;
                check_version(&task, *expected_version)?;
                require_state(&task, &[TaskState::CancelRequested])?;
                task.state = TaskState::Cancelled;
                let attempt = task.attempts.last_mut().context("attempt missing")?;
                attempt.stopped = true;
                attempt.last_seen = now();
                save_task(tx, &mut task, "task_stopped")?;
                Ok(Response::Task { task })
            }
            Action::TaskAccept {
                task_id,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                check_version(&task, *expected_version)?;
                require_state(&task, &[TaskState::ResultReady])?;
                let attempt = task.attempts.last().context("attempt missing")?;
                let result = attempt.result.as_ref().context("result missing")?;
                for artifact_id in &result.artifact_ids {
                    let artifact = get_artifact(tx, project, artifact_id)?;
                    if !artifact.received
                        || artifact.local_path.is_none()
                        || artifact.task_id != task.id
                        || artifact.attempt_id != attempt.id
                    {
                        return reject(
                            "artifact_pending",
                            "all result artifacts must be received and verified before acceptance",
                        );
                    }
                    verify_artifact(
                        &artifact,
                        artifact
                            .local_path
                            .as_deref()
                            .context("artifact path missing")?,
                    )?;
                }
                task.state = TaskState::Completed;
                save_task(tx, &mut task, "task_accepted")?;
                Ok(Response::Task { task })
            }
            Action::TaskRequeue {
                task_id,
                expected_version,
            } => {
                let mut task = visible_task(tx, project, actor, role, task_id)?;
                check_version(&task, *expected_version)?;
                require_state(
                    &task,
                    &[
                        TaskState::Failed,
                        TaskState::Cancelled,
                        TaskState::ResultReady,
                    ],
                )?;
                if task.attempts.last().is_some_and(|attempt| !attempt.stopped) {
                    return reject(
                        "invalid_state",
                        "previous attempt has not confirmed stopping",
                    );
                }
                task.state = TaskState::Queued;
                task.progress = None;
                save_task(tx, &mut task, "task_requeued")?;
                Ok(Response::Task { task })
            }
            Action::MessageSend { to, task_id, body } => {
                text_limit(body)?;
                let session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                target_session(tx, project, to)?;
                if let Some(task_id) = task_id {
                    visible_task(tx, project, actor, role, task_id)?;
                    let target_role = member_role(tx, project, &to.device_id)?;
                    visible_task(tx, project, &to.device_id, target_role, task_id)?;
                }
                let (pending, bytes): (i64, i64) = tx.query_row("SELECT count(*), COALESCE(sum(length(CAST(data AS BLOB))), 0) FROM messages WHERE project_id = ?1", [project], |row| Ok((row.get(0)?, row.get(1)?)))?;
                if pending >= 10_000 || bytes + body.len() as i64 + 2048 > 100 * 1024 * 1024 {
                    return reject(
                        "queue_full",
                        "project message capacity is 10000 records or 100 MiB",
                    );
                }
                let message = Message {
                    id: new_id(),
                    project_id: project.to_owned(),
                    task_id: task_id.clone(),
                    from: SessionRef {
                        device_id: actor.to_owned(),
                        session_id: session.id,
                    },
                    to: to.clone(),
                    body: body.clone(),
                    delivered: false,
                    consumed: false,
                    created_at: now(),
                };
                tx.execute("INSERT INTO messages(project_id, id, source_device, target_device, target_session, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![project, message.id, actor, to.device_id, to.session_id, bounded(&message)?])?;
                event(
                    tx,
                    project,
                    "message_accepted",
                    &message.id,
                    task_id.as_deref(),
                    Some(&to.device_id),
                    json!(message),
                )?;
                Ok(Response::Message { message })
            }
            Action::Inbox => {
                let session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                Ok(Response::Messages {
                    messages: rows(
                        tx,
                        "SELECT data FROM messages WHERE project_id = ?1 AND target_device = ?2 AND target_session = ?3 ORDER BY rowid",
                        params![project, actor, session.id],
                    )?,
                })
            }
            Action::MessageDelivered { message_id } | Action::MessageConsume { message_id } => {
                let session = authenticated_session(tx, project, actor, request.auth.as_ref())?;
                let mut message: Message = match load(
                    tx,
                    "SELECT data FROM messages WHERE project_id = ?1 AND id = ?2",
                    params![project, message_id],
                )? {
                    Some(message) => message,
                    None => return reject("not_found", "message not found"),
                };
                if message.to.device_id != actor || message.to.session_id != session.id {
                    return reject(
                        "forbidden",
                        "only the addressed session may acknowledge this message",
                    );
                }
                let consuming = matches!(request.action, Action::MessageConsume { .. });
                if !message.delivered || (consuming && !message.consumed) {
                    message.delivered = true;
                    message.consumed |= consuming;
                    tx.execute(
                        "UPDATE messages SET data = ?3 WHERE project_id = ?1 AND id = ?2",
                        params![project, message_id, bounded(&message)?],
                    )?;
                    event(
                        tx,
                        project,
                        if consuming {
                            "message_consumed"
                        } else {
                            "message_delivered"
                        },
                        &message.id,
                        message.task_id.as_deref(),
                        Some(actor),
                        json!(message),
                    )?;
                }
                Ok(Response::Message { message })
            }
            Action::EventsRead { after, limit } => self.events_read(
                tx,
                project,
                actor,
                role,
                request.auth.as_ref(),
                *after,
                *limit,
            ),
            Action::ArtifactDeclare {
                task_id,
                attempt_id,
                name,
                size,
                hash,
            } => {
                nonempty(name, "artifact name")?;
                if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return reject(
                        "invalid_request",
                        "artifact hash must be a 64-character BLAKE3 digest",
                    );
                }
                let task = visible_task(tx, project, actor, role, task_id)?;
                execution_attempt(tx, &task, actor, request.auth.as_ref(), attempt_id)?;
                require_state(
                    &task,
                    &[
                        TaskState::Running,
                        TaskState::Blocked,
                        TaskState::CancelRequested,
                    ],
                )?;
                let artifact = Artifact {
                    id: new_id(),
                    task_id: task_id.clone(),
                    attempt_id: attempt_id.clone(),
                    provider_device: actor.to_owned(),
                    name: name.clone(),
                    size: *size,
                    hash: hash.to_ascii_lowercase(),
                    received: false,
                    local_path: None,
                };
                tx.execute("INSERT INTO artifacts(project_id, id, task_id, attempt_id, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![project, artifact.id, task_id, attempt_id, bounded(&artifact)?])?;
                event(
                    tx,
                    project,
                    "artifact_declared",
                    &artifact.id,
                    Some(task_id),
                    Some(actor),
                    json!(artifact),
                )?;
                Ok(Response::Artifact { artifact })
            }
            Action::ArtifactReceipt {
                artifact_id,
                local_path,
            } => {
                let mut artifact = get_artifact(tx, project, artifact_id)?;
                verify_artifact(&artifact, local_path)?;
                artifact.received = true;
                artifact.local_path = Some(local_path.clone());
                tx.execute(
                    "UPDATE artifacts SET data = ?3 WHERE project_id = ?1 AND id = ?2",
                    params![project, artifact_id, bounded(&artifact)?],
                )?;
                event(
                    tx,
                    project,
                    "artifact_received",
                    artifact_id,
                    Some(&artifact.task_id),
                    Some(&artifact.provider_device),
                    json!(artifact),
                )?;
                Ok(Response::Artifact { artifact })
            }
            Action::ArtifactsList { task_id } => {
                visible_task(tx, project, actor, role, task_id)?;
                Ok(Response::Artifacts {
                    artifacts: rows(
                        tx,
                        "SELECT data FROM artifacts WHERE project_id = ?1 AND task_id = ?2 ORDER BY id",
                        params![project, task_id],
                    )?,
                })
            }
        }
    }

    fn events_read(
        &self,
        conn: &Connection,
        project: &str,
        actor: &str,
        role: Role,
        auth: Option<&SessionAuth>,
        after: u64,
        limit: usize,
    ) -> Result<Response> {
        if after > i64::MAX as u64 || limit == 0 {
            return reject(
                "invalid_request",
                "event cursor is out of range or limit is zero",
            );
        }
        let limit = limit.min(MAX_EVENTS);
        let mut statement = conn.prepare("SELECT sequence, kind, entity_id, task_id, target_device, data FROM events WHERE project_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT 1000")?;
        let mut rows = statement.query(params![
            project,
            i64::try_from(after).context("event cursor is too large")?
        ])?;
        let mut events = Vec::new();
        let mut cursor = after;
        let mut bytes = 128;
        while let Some(row) = rows.next()? {
            let seq: u64 = u64::try_from(row.get::<_, i64>(0)?)?;
            let kind: String = row.get(1)?;
            let entity_id: String = row.get(2)?;
            let task_id: Option<String> = row.get(3)?;
            let target_device: Option<String> = row.get(4)?;
            let data: String = row.get(5)?;
            let visible = if kind.starts_with("message_") {
                let message: Option<Message> = load(
                    conn,
                    "SELECT data FROM messages WHERE project_id = ?1 AND id = ?2",
                    params![project, entity_id],
                )?;
                message.is_some_and(|message| {
                    let is_sender = message.from.device_id == actor
                        && auth.is_none_or(|auth| auth.session_id == message.from.session_id);
                    let is_target = message.to.device_id == actor
                        && auth.is_none_or(|auth| auth.session_id == message.to.session_id);
                    is_sender || is_target
                })
            } else if role == Role::Worker {
                if let Some(task_id) = task_id {
                    let assigned: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE project_id = ?1 AND id = ?2 AND device_id = ?3)", params![project, task_id, actor], |row| row.get(0))?;
                    assigned && target_device.as_deref() == Some(actor)
                } else {
                    target_device.as_deref() == Some(actor)
                }
            } else {
                true
            };
            if visible {
                if bytes + data.len() + 1 > MAX_REPLY_BYTES - 128 {
                    break;
                }
                bytes += data.len() + 1;
                events.push(serde_json::from_str(&data)?);
                cursor = seq;
                if events.len() == limit {
                    break;
                }
            } else {
                cursor = seq;
            }
        }
        Ok(Response::Events { events, cursor })
    }
}

fn project_id(request: &Request) -> Result<&str> {
    match request.project_id.as_deref() {
        Some(project) if !project.is_empty() && project.len() <= 256 => Ok(project),
        _ => reject("invalid_request", "project_id is required"),
    }
}

fn member_role(conn: &Connection, project: &str, actor: &str) -> Result<Role> {
    let member: Option<Member> = load(
        conn,
        "SELECT data FROM members WHERE project_id = ?1 AND device_id = ?2 AND revoked = 0",
        params![project, actor],
    )?;
    match member {
        Some(member) => Ok(member.role),
        None => reject(
            "forbidden",
            "device is not an active member of this project",
        ),
    }
}

fn save_member(conn: &Connection, project: &str, member: &Member) -> Result<()> {
    let role = match member.role {
        Role::Admin => "admin",
        Role::Dispatcher => "dispatcher",
        Role::Worker => "worker",
        Role::Viewer => "viewer",
    };
    conn.execute("INSERT INTO members(project_id, device_id, role, revoked, data) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(project_id, device_id) DO UPDATE SET role = excluded.role, revoked = excluded.revoked, data = excluded.data",
        params![project, member.device_id, role, member.revoked, bounded(member)?])?;
    Ok(())
}

fn authenticated_session(
    conn: &Connection,
    project: &str,
    actor: &str,
    auth: Option<&SessionAuth>,
) -> Result<AgentSession> {
    let Some(auth) = auth else {
        return reject("forbidden", "a registered session and token are required");
    };
    let row: Option<(String, String)> = conn.query_row(
        "SELECT data, token_hash FROM agent_sessions WHERE project_id = ?1 AND id = ?2 AND device_id = ?3",
        params![project, auth.session_id, actor], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let Some((data, token_hash)) = row else {
        return reject(
            "forbidden",
            "session does not belong to authenticated device",
        );
    };
    if token_hash.is_empty() || token_hash != blake3::hash(auth.token.as_bytes()).to_hex().as_str()
    {
        return reject("forbidden", "invalid or revoked session credential");
    }
    Ok(serde_json::from_str(&data)?)
}

fn target_session(conn: &Connection, project: &str, target: &SessionRef) -> Result<AgentSession> {
    let role = member_role(conn, project, &target.device_id)?;
    if role == Role::Viewer {
        return reject(
            "forbidden",
            "viewer sessions cannot execute tasks or receive work",
        );
    }
    let session: Option<AgentSession> = load(
        conn,
        "SELECT data FROM agent_sessions WHERE project_id = ?1 AND id = ?2 AND device_id = ?3 AND token_hash != ''",
        params![project, target.session_id, target.device_id],
    )?;
    match session {
        Some(session) if session.available => Ok(session),
        _ => reject("session_unavailable", "target session is unavailable"),
    }
}

fn visible_task(
    conn: &Connection,
    project: &str,
    actor: &str,
    role: Role,
    id: &str,
) -> Result<Task> {
    let task: Option<Task> = load(
        conn,
        "SELECT data FROM tasks WHERE project_id = ?1 AND id = ?2",
        params![project, id],
    )?;
    let Some(task) = task else {
        return reject("not_found", "task not found");
    };
    if role == Role::Worker && task.spec.assignee.device_id != actor {
        return reject(
            "forbidden",
            "worker may only access tasks assigned to its device",
        );
    }
    Ok(task)
}

fn check_version(task: &Task, expected: u64) -> Result<()> {
    if task.version != expected {
        return reject(
            "version_conflict",
            format!("current_version={}", task.version),
        );
    }
    Ok(())
}

fn require_state(task: &Task, allowed: &[TaskState]) -> Result<()> {
    if !allowed.contains(&task.state) {
        return reject(
            "invalid_state",
            format!("operation is not allowed in {}", state_name(task.state)),
        );
    }
    Ok(())
}

fn require_assignee(task: &Task, session: &AgentSession) -> Result<()> {
    if task.spec.assignee.device_id != session.device_id
        || task.spec.assignee.session_id != session.id
    {
        return reject(
            "forbidden",
            "only the assigned session may execute this task",
        );
    }
    Ok(())
}

fn execution_attempt(
    conn: &Connection,
    task: &Task,
    actor: &str,
    auth: Option<&SessionAuth>,
    id: &str,
) -> Result<()> {
    let session = authenticated_session(conn, &task.project_id, actor, auth)?;
    require_assignee(task, &session)?;
    let Some(attempt) = task.attempts.last() else {
        return reject("stale_attempt", "task has no current attempt");
    };
    if attempt.id != id || attempt.stopped {
        return reject("stale_attempt", "attempt is no longer executing");
    }
    if attempt.assignee.device_id != actor || attempt.assignee.session_id != session.id {
        return reject("forbidden", "attempt belongs to another session");
    }
    Ok(())
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "queued",
        TaskState::Running => "running",
        TaskState::Blocked => "blocked",
        TaskState::ResultReady => "result_ready",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::CancelRequested => "cancel_requested",
        TaskState::Cancelled => "cancelled",
    }
}

fn save_task(conn: &Connection, task: &mut Task, kind: &str) -> Result<()> {
    let previous = task.version;
    task.version = task
        .version
        .checked_add(1)
        .filter(|version| *version <= i64::MAX as u64)
        .context("task version overflow")?;
    task.updated_at = now();
    let changed = conn.execute(
        "UPDATE tasks SET version = ?3, status = ?4, device_id = ?5, session_id = ?6, data = ?7 WHERE project_id = ?1 AND id = ?2 AND version = ?8",
        params![task.project_id, task.id, i64::try_from(task.version)?, state_name(task.state), task.spec.assignee.device_id, task.spec.assignee.session_id, bounded(task)?, i64::try_from(previous)?],
    )?;
    if changed != 1 {
        return reject("version_conflict", "task changed concurrently");
    }
    for attempt in &task.attempts {
        conn.execute("INSERT INTO attempts(project_id, id, task_id, data) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(project_id, id) DO UPDATE SET data = excluded.data",
            params![task.project_id, attempt.id, task.id, bounded(attempt)?])?;
    }
    task_event(conn, kind, task)
}

fn event(
    conn: &Connection,
    project: &str,
    kind: &str,
    entity: &str,
    task: Option<&str>,
    target: Option<&str>,
    data: Value,
) -> Result<()> {
    let seq: u64 = u64::try_from(conn.query_row::<i64, _, _>(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE project_id = ?1",
        [project],
        |row| row.get(0),
    )?)?;
    let event = Event {
        seq,
        kind: kind.to_owned(),
        entity_id: entity.to_owned(),
        data,
        created_at: now(),
    };
    conn.execute("INSERT INTO events(project_id, sequence, kind, entity_id, task_id, target_device, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project, i64::try_from(seq)?, kind, entity, task, target, bounded(&event)?])?;
    Ok(())
}

fn task_event(conn: &Connection, kind: &str, task: &Task) -> Result<()> {
    event(
        conn,
        &task.project_id,
        kind,
        &task.id,
        Some(&task.id),
        Some(&task.spec.assignee.device_id),
        json!(task),
    )
}

fn get_artifact(conn: &Connection, project: &str, id: &str) -> Result<Artifact> {
    match load(
        conn,
        "SELECT data FROM artifacts WHERE project_id = ?1 AND id = ?2",
        params![project, id],
    )? {
        Some(artifact) => Ok(artifact),
        None => reject("not_found", "artifact not found"),
    }
}

fn verify_artifact(artifact: &Artifact, path: &str) -> Result<()> {
    if !Path::new(path).is_absolute() {
        return reject(
            "invalid_artifact",
            "artifact receipt requires an absolute coordinator-local path",
        );
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return reject("artifact_pending", "artifact file is unavailable"),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != artifact.size {
        return reject(
            "artifact_mismatch",
            "artifact size or file type does not match declaration",
        );
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut bytes = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > artifact.size {
            return reject("artifact_mismatch", "artifact changed during verification");
        }
        hasher.update(&buffer[..read]);
    }
    if bytes != artifact.size || hasher.finalize().to_hex().as_str() != artifact.hash {
        return reject(
            "artifact_mismatch",
            "artifact BLAKE3 digest does not match declaration",
        );
    }
    Ok(())
}

fn text_limit(value: &str) -> Result<()> {
    if value.len() > MAX_MESSAGE_BYTES {
        return reject("payload_too_large", "text exceeds 64 KiB");
    }
    Ok(())
}

fn nonempty(value: &str, name: &str) -> Result<()> {
    if value.trim().is_empty() {
        return reject("invalid_request", format!("{name} must not be empty"));
    }
    text_limit(value)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
