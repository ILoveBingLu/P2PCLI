//! Foreground managed Claude Code adapter.
//!
//! Register a session with `collab session register --mode managed-adapter`,
//! then run `p2pcli collab worker --config worker.json`.
//!
//! Example:
//! ```json
//! {
//!   "coordinator": null,
//!   "project_id": "project-id",
//!   "credential_file": "/private/session.json",
//!   "cwd": "/Users/me/src/project",
//!   "program": "claude",
//!   "poll_interval_secs": 5
//! }
//! ```
//! The credential file uses the CLI agent's format:
//! `{"coordinator":null,"project_id":"...","auth":{"session_id":"...","token":"..."}}`.
//! `cwd` is always local configuration. This worker is a foreground process;
//! it does not promise app background startup.
//!
//! Claude Code official print mode is used with `-p`, `--output-format json`,
//! `--permission-mode default`, and `--permission-prompts none`. The task
//! prompt is written to stdin, never interpolated into a shell command.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use p2pcli_core::collaboration::model::{
    Action, Reply as CollabReply, Request as CollabRequest, Response, SessionAuth, SessionMode,
    Task, TaskResult, TaskState, new_id,
};
use p2pcli_core::control::{self, Reply as ControlReply, Request as ControlRequest};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const DEFAULT_PROGRAM: &str = "claude";
const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 6000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    #[serde(default)]
    pub coordinator: Option<String>,
    pub project_id: String,
    pub credential_file: PathBuf,
    pub cwd: PathBuf,
    #[serde(default = "default_program")]
    pub program: String,
    #[serde(default = "default_poll_interval", alias = "poll_interval")]
    pub poll_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
struct CredentialFile {
    coordinator: Option<String>,
    project_id: String,
    auth: SessionAuth,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AttemptStatus {
    Claiming,
    Claimed,
    Started,
    Unknown,
    Finished,
    Blocked,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptRecord {
    task_id: String,
    attempt_id: Option<String>,
    version: u64,
    status: AttemptStatus,
    result: Option<TaskResult>,
    pending: Option<CollabRequest>,
    updated_at: u64,
    last_error: Option<String>,
    #[serde(default)]
    reported: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum QueueState {
    Pending,
    Delivered,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueRecord {
    request: CollabRequest,
    state: QueueState,
    attempts: u32,
    last_error: Option<String>,
    updated_at: u64,
}

#[derive(Debug)]
struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Debug)]
struct ProcessOutcome {
    status: std::process::ExitStatus,
    stdout: CapturedOutput,
    stderr: CapturedOutput,
    cancelled: bool,
}

#[derive(Debug, Deserialize)]
struct ClaudeResult {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    result: Option<String>,
    is_error: bool,
    #[serde(default)]
    permission_denials: Vec<Value>,
    #[serde(default)]
    errors: Vec<String>,
}

fn default_program() -> String {
    DEFAULT_PROGRAM.into()
}

fn default_poll_interval() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Run until Ctrl-C. The caller owns CLI parsing and process lifetime.
pub async fn run(config_path: &Path, data_dir: &Path) -> Result<()> {
    let config_path = fs::canonicalize(config_path)?;
    let config_path = config_path.as_path();
    let config = load_config(config_path)?;
    let credential = load_credentials(&config, config_path)?;
    let cwd = fs::canonicalize(resolve_local_path(&config.cwd, config_path)?)?;
    ensure!(cwd.is_dir(), "worker cwd must be a directory");
    let session_id = &credential.auth.session_id;
    ensure!(
        !session_id.is_empty()
            && session_id.len() <= 128
            && session_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid worker session identifier"
    );
    let scoped = data_dir.join("managed-sessions").join(session_id);
    private_dir(&scoped)?;
    let paths = WorkerPaths::new(&scoped)?;
    let lock_path = paths.root.join("worker.lock");
    if !lock_path.exists() {
        drop(private_new(&lock_path)?);
    }
    check_private(&lock_path)?;
    let lock = OpenOptions::new().read(true).write(true).open(lock_path)?;
    lock.try_lock()
        .context("another worker owns this session")?;
    let binding_path = paths.root.join("binding.json");
    let binding = serde_json::json!({
        "coordinator": normalize_coordinator(config.coordinator.as_deref()),
        "project_id": config.project_id, "cwd": cwd, "program": config.program,
    });
    if binding_path.exists() {
        ensure!(
            read_json::<Value>(&binding_path)? == binding,
            "worker binding changed; register a new session"
        );
    } else {
        persist(&binding_path, &binding)?;
    }
    let worker = Worker {
        config,
        cwd,
        target: credential
            .coordinator
            .clone()
            .unwrap_or_else(|| "local".into()),
        auth: credential.auth,
        data_dir: data_dir.to_owned(),
        paths,
    };
    worker.recover_started().await?;
    let signal = tokio::signal::ctrl_c();
    tokio::pin!(signal);
    loop {
        tokio::select! {
            stopped = &mut signal => {
                stopped?;
                return Ok(());
            }
            result = worker.tick() => result?,
        }
        tokio::select! {
            stopped = &mut signal => { stopped?; return Ok(()); }
            _ = tokio::time::sleep(worker.poll_interval()) => {}
        }
    }
}

struct Worker {
    config: WorkerConfig,
    cwd: PathBuf,
    target: String,
    auth: SessionAuth,
    data_dir: PathBuf,
    paths: WorkerPaths,
}

#[derive(Clone)]
struct WorkerPaths {
    root: PathBuf,
    attempts: PathBuf,
    results: PathBuf,
    logs: PathBuf,
    outbox: PathBuf,
}

impl WorkerPaths {
    fn new(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("managed-worker");
        let paths = Self {
            attempts: root.join("attempts"),
            results: root.join("results"),
            logs: root.join("logs"),
            outbox: root.join("outbox"),
            root,
        };
        for path in [
            &paths.root,
            &paths.attempts,
            &paths.results,
            &paths.logs,
            &paths.outbox,
        ] {
            private_dir(path)?;
        }
        Ok(paths)
    }
}

fn load_config(path: &Path) -> Result<WorkerConfig> {
    let config: WorkerConfig = read_json(path)?;
    ensure!(
        !config.project_id.trim().is_empty(),
        "project_id is required"
    );
    ensure!(!config.program.trim().is_empty(), "program is required");
    ensure!(
        (1..=3600).contains(&config.poll_interval_secs),
        "poll_interval_secs must be between 1 and 3600"
    );
    Ok(config)
}

fn load_credentials(config: &WorkerConfig, config_path: &Path) -> Result<CredentialFile> {
    let path = resolve_local_path(&config.credential_file, config_path)?;
    check_private(&path)?;
    let credential: CredentialFile = read_json(&path)?;
    ensure!(
        credential.project_id == config.project_id,
        "credential project_id does not match config"
    );
    ensure!(
        normalize_coordinator(credential.coordinator.as_deref())
            == normalize_coordinator(config.coordinator.as_deref()),
        "credential coordinator does not match config"
    );
    ensure!(
        !credential.auth.session_id.trim().is_empty() && !credential.auth.token.trim().is_empty(),
        "credential auth must contain session_id and token"
    );
    Ok(credential)
}

fn resolve_local_path(path: &Path, config_path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    };
    Ok(path)
}

fn normalize_coordinator(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("local"))
        .map(str::to_owned)
}

impl Worker {
    fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.config.poll_interval_secs)
    }

    fn request(&self, action: Action) -> CollabRequest {
        CollabRequest {
            request_id: new_id(),
            project_id: Some(self.config.project_id.clone()),
            auth: Some(self.auth.clone()),
            action,
        }
    }

    async fn send(&self, request: CollabRequest) -> Result<CollabReply> {
        control_collab(&self.data_dir, Some(self.target.clone()), request).await
    }

    async fn read(&self, action: Action) -> Result<Response> {
        self.send(self.request(action)).await?.into_result()
    }

    async fn tick(&self) -> Result<()> {
        self.flush_outbox().await;
        let sessions = match self.read(Action::SessionsList).await {
            Ok(Response::Sessions { sessions }) => sessions,
            Ok(_) => bail!("expected sessions response"),
            Err(_) => return Ok(()),
        };
        let session = sessions
            .into_iter()
            .find(|session| session.id == self.auth.session_id)
            .context("registered worker session not found")?;
        ensure!(
            session.mode == SessionMode::ManagedAdapter
                && session.project_id == self.config.project_id,
            "session is not registered as managed-adapter"
        );
        if self
            .read(Action::SessionHeartbeat { available: true })
            .await
            .is_err()
        {
            return Ok(());
        }
        for path in self.journal_paths()? {
            self.drive(&path).await?;
        }
        // A recovered unknown process may still touch cwd. Do not overlap it.
        for path in self.journal_paths()? {
            let record: AttemptRecord = read_json(&path)?;
            if matches!(
                record.status,
                AttemptStatus::Unknown
                    | AttemptStatus::Claiming
                    | AttemptStatus::Claimed
                    | AttemptStatus::Started
            ) || record.pending.is_some()
            {
                return Ok(());
            }
        }
        let tasks = match self.read(Action::TasksList).await {
            Ok(Response::Tasks { tasks }) => tasks,
            Ok(_) => bail!("expected tasks response"),
            Err(_) => return Ok(()),
        };
        for task in tasks {
            if !self.is_assigned(&task, &session) || task.state != TaskState::Queued {
                continue;
            }
            if self.has_journal_for(&task.id, task.version)? {
                continue;
            }
            self.claim(task).await?;
            break;
        }
        for path in self.journal_paths()? {
            self.drive(&path).await?;
        }
        Ok(())
    }

    fn is_assigned(
        &self,
        task: &Task,
        session: &p2pcli_core::collaboration::model::AgentSession,
    ) -> bool {
        task.project_id == self.config.project_id
            && task.spec.assignee.device_id == session.device_id
            && task.spec.assignee.session_id == session.id
    }

    fn journal_paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.paths.attempts)? {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                check_private(&path)?;
                paths.push(path);
            }
        }
        Ok(paths)
    }

    fn has_journal_for(&self, task_id: &str, version: u64) -> Result<bool> {
        for path in self.journal_paths()? {
            let record: AttemptRecord = read_json(&path)?;
            if record.task_id == task_id && record.version >= version {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn claim(&self, task: Task) -> Result<()> {
        let request = self.request(Action::TaskClaim {
            task_id: task.id.clone(),
            expected_version: task.version,
        });
        let path = self
            .paths
            .attempts
            .join(format!("{}.json", request.request_id));
        persist(
            &path,
            &AttemptRecord {
                task_id: task.id,
                attempt_id: None,
                version: task.version,
                status: AttemptStatus::Claiming,
                result: None,
                pending: Some(request),
                updated_at: now_secs(),
                last_error: None,
                reported: false,
            },
        )?;
        Ok(())
    }

    async fn recover_started(&self) -> Result<()> {
        for path in self.journal_paths()? {
            let mut record: AttemptRecord = read_json(&path)?;
            if record.status == AttemptStatus::Started {
                record.status = AttemptStatus::Unknown;
                record.last_error = Some(
                    "started attempt has no durable result after restart; execution is unknown and will not be rerun"
                        .into(),
                );
                record.updated_at = now_secs();
                persist(&path, &record)?;
            }
        }
        Ok(())
    }

    async fn drive(&self, path: &Path) -> Result<()> {
        let mut record: AttemptRecord = read_json(path)?;
        if record.status == AttemptStatus::Rejected {
            return Ok(());
        }
        if let Some(request) = record.pending.clone() {
            match self.send(request.clone()).await {
                Ok(reply) if reply.error.is_none() => {
                    if record.status == AttemptStatus::Claiming {
                        let Some(Response::Task { task }) = reply.response else {
                            bail!("claim response did not contain a task")
                        };
                        let attempt = task
                            .attempts
                            .last()
                            .context("claim response missing attempt")?;
                        ensure!(
                            attempt.assignee.session_id == self.auth.session_id
                                && task.id == record.task_id
                                && task.project_id == self.config.project_id
                                && attempt.assignee == task.spec.assignee
                                && task.state == TaskState::Running,
                            "claim response belongs to another session"
                        );
                        record.attempt_id = Some(attempt.id.clone());
                        record.version = task.version;
                        record.status = AttemptStatus::Claimed;
                    } else {
                        ensure!(
                            matches!(reply.response, Some(Response::Task { .. } | Response::Ack)),
                            "missing execution acknowledgement"
                        );
                        record.reported = true;
                    }
                    record.pending = None;
                    record.updated_at = now_secs();
                    persist(path, &record)?;
                }
                Ok(reply) => {
                    let error = reply.error.context("invalid error reply")?;
                    record.last_error = Some(format!("{}: {}", error.code, short(&error.message)));
                    if error.code == "storage_error" {
                        return Ok(());
                    }
                    record.pending = None;
                    // Receipt has definitively rejected the old request. Only now
                    // may a refreshed CAS request use a new ID with new contents.
                    if error.code != "version_conflict" || record.status == AttemptStatus::Claiming
                    {
                        record.status = AttemptStatus::Rejected;
                    }
                    persist(path, &record)?;
                    if record.status == AttemptStatus::Rejected {
                        return Ok(());
                    }
                }
                Err(_) => return Ok(()),
            }
        }
        if record.reported
            && !matches!(
                record.status,
                AttemptStatus::Blocked | AttemptStatus::Unknown
            )
        {
            return Ok(());
        }
        let task = match self
            .read(Action::TaskGet {
                task_id: record.task_id.clone(),
            })
            .await
        {
            Ok(Response::Task { task }) => task,
            _ => return Ok(()),
        };
        let attempt_id = record
            .attempt_id
            .clone()
            .context("journal has no attempt")?;
        let current = task.attempts.last().is_some_and(|attempt| {
            attempt.id == attempt_id && attempt.assignee.session_id == self.auth.session_id
        });
        if !current
            || task.project_id != self.config.project_id
            || task.id != record.task_id
            || task.spec.assignee.session_id != self.auth.session_id
            || matches!(
                task.state,
                TaskState::Completed
                    | TaskState::Failed
                    | TaskState::ResultReady
                    | TaskState::Cancelled
            )
        {
            record.status = AttemptStatus::Rejected;
            record.last_error =
                Some("task version or attempt changed; result retained locally".into());
            persist(path, &record)?;
            return Ok(());
        }
        record.version = task.version;
        if record.status == AttemptStatus::Claimed {
            if task.state == TaskState::CancelRequested {
                record.status = AttemptStatus::Cancelled;
                record.pending = Some(self.request(Action::TaskStopAck {
                    task_id: task.id,
                    attempt_id,
                    expected_version: task.version,
                }));
                persist(path, &record)?;
                return Ok(());
            }
            ensure!(
                task.state == TaskState::Running,
                "claimed task is no longer running"
            );
            record.status = AttemptStatus::Started;
            persist(path, &record)?;
            let prompt = build_prompt(&task)?;
            match run_process(self, path, &prompt).await {
                Ok(outcome) => {
                    record.status = if outcome.cancelled {
                        AttemptStatus::Cancelled
                    } else {
                        AttemptStatus::Finished
                    };
                    if !outcome.cancelled {
                        let denied = serde_json::from_slice::<ClaudeResult>(&outcome.stdout.bytes)
                            .is_ok_and(|r| r.kind == "result" && !r.permission_denials.is_empty());
                        if denied {
                            record.status = AttemptStatus::Blocked;
                        }
                        record.result = Some(parse_result(&outcome).unwrap_or_else(|_| {
                            failed_result("Invalid Claude JSON result; inspect private logs")
                        }));
                    }
                }
                Err(_) => {
                    record.status = AttemptStatus::Unknown;
                    record.last_error = Some(
                        "Process supervision failed; execution unknown, never automatically rerun"
                            .into(),
                    );
                }
            }
            record.updated_at = now_secs();
            persist(path, &record)?;
        }
        // Refresh after process exit: cancellation may have incremented the CAS
        // version while Claude was running. A further race is handled above.
        let task = match self
            .read(Action::TaskGet {
                task_id: record.task_id.clone(),
            })
            .await
        {
            Ok(Response::Task { task }) => task,
            _ => return Ok(()),
        };
        if task.attempts.last().is_none_or(|a| a.id != attempt_id) {
            return Ok(());
        }
        let action = match (record.status, task.state) {
            (
                AttemptStatus::Finished,
                TaskState::Running | TaskState::Blocked | TaskState::CancelRequested,
            ) => Action::TaskResult {
                task_id: task.id,
                attempt_id,
                result: record.result.clone().context("finished result missing")?,
                expected_version: task.version,
            },
            (AttemptStatus::Cancelled | AttemptStatus::Blocked, TaskState::CancelRequested) => {
                Action::TaskStopAck {
                    task_id: task.id,
                    attempt_id,
                    expected_version: task.version,
                }
            }
            (
                AttemptStatus::Unknown | AttemptStatus::Blocked,
                TaskState::Running | TaskState::Blocked,
            ) if !record.reported => Action::TaskProgress {
                task_id: task.id,
                attempt_id,
                expected_version: task.version,
                blocked: true,
                message: if record.status == AttemptStatus::Unknown {
                    "Execution unknown after runner interruption; not rerun. Inspect local processes and side effects before manual recovery.".into()
                } else {
                    "Claude permission denied; process exited. Inspect logs and configure permissions locally before a new attempt.".into()
                },
            },
            _ => return Ok(()), // Unknown cannot acknowledge cancellation.
        };
        record.version = task.version;
        record.reported = false;
        record.pending = Some(self.request(action));
        persist(path, &record)?;
        Ok(())
    }

    async fn flush_outbox(&self) {
        let Ok(entries) = fs::read_dir(&self.paths.outbox) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(mut record) = read_json::<QueueRecord>(&path) else {
                continue;
            };
            if record.state != QueueState::Pending {
                continue;
            }
            match self.send(record.request.clone()).await {
                Ok(reply) if reply.error.is_none() => {
                    record.state = QueueState::Delivered;
                    record.updated_at = now_secs();
                    let _ = persist(&path, &record);
                }
                Ok(reply) => {
                    record.state = QueueState::Rejected;
                    record.last_error = reply.error.map(|error| error.message);
                    record.updated_at = now_secs();
                    let _ = persist(&path, &record);
                }
                Err(error) => {
                    record.attempts = record.attempts.saturating_add(1);
                    record.last_error = Some(format!("{error:#}"));
                    record.updated_at = now_secs();
                    let _ = persist(&path, &record);
                }
            }
        }
    }
}

async fn control_collab(
    data_dir: &Path,
    target: Option<String>,
    request: CollabRequest,
) -> Result<CollabReply> {
    ensure!(
        serde_json::to_vec(&request)?.len() <= p2pcli_core::collaboration::model::MAX_MESSAGE_BYTES,
        "collaboration request exceeds wire budget"
    );
    let control_request = ControlRequest::Collab { target, request };
    let mut result = None;
    let mut error = None;
    tokio::time::timeout(
        Duration::from_secs(5),
        control::request_while(data_dir, &control_request, |reply| match reply {
            ControlReply::Collab { reply } => {
                result = Some(reply);
                false
            }
            ControlReply::Error { message } => {
                error = Some(message);
                false
            }
            _ => true,
        }),
    )
    .await
    .context("worker control request timed out")??;
    if let Some(error) = error {
        bail!(error);
    }
    result.context("control socket returned no collaboration reply")
}

async fn run_process(worker: &Worker, path: &Path, prompt: &str) -> Result<ProcessOutcome> {
    ensure!(
        prompt.len() <= 10 * 1024 * 1024,
        "task prompt exceeds 10 MiB"
    );
    let mut command = Command::new(&worker.config.program);
    command
        .current_dir(&worker.cwd)
        .args([
            "-p",
            "Complete the managed task provided on stdin.",
            "--output-format",
            "json",
            "--permission-mode",
            "default",
            "--permission-prompts",
            "none",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .context("failed to start local AI program")?;
    let pid = child.id();
    let mut guard = ProcessGuard { pid, armed: true };
    let stdout = child.stdout.take().context("AI process has no stdout")?;
    let stderr = child.stderr.take().context("AI process has no stderr")?;
    let stdout_path = path.with_extension("stdout.log");
    let stderr_path = path.with_extension("stderr.log");
    let stdout_task = tokio::spawn(capture_output(stdout, stdout_path, MAX_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(capture_output(stderr, stderr_path, MAX_OUTPUT_BYTES));
    let mut stdin = child.stdin.take().context("AI process has no stdin")?;
    let input = prompt.as_bytes().to_vec();
    let input_task = tokio::spawn(async move {
        stdin.write_all(&input).await?;
        stdin.shutdown().await?;
        drop(stdin);
        Ok::<_, std::io::Error>(())
    });
    let mut cancelled = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let task = read_task_for_process(worker, path).await;
        if task
            .as_ref()
            .is_some_and(|task| task.state == TaskState::CancelRequested)
        {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            cancelled = true;
            terminate_process(pid);
            let status = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(status) => status?,
                Err(_) => {
                    kill_process_tree(pid);
                    child.start_kill()?;
                    tokio::time::timeout(Duration::from_secs(5), child.wait()).await??
                }
            };
            kill_process_tree(pid);
            break status;
        }
        let _ = worker
            .read(Action::SessionHeartbeat { available: false })
            .await;
        tokio::time::sleep(worker.poll_interval()).await;
    };
    kill_process_tree(pid);
    guard.armed = false;
    let stdout = tokio::time::timeout(Duration::from_secs(5), stdout_task).await???;
    let stderr = tokio::time::timeout(Duration::from_secs(5), stderr_task).await???;
    if !cancelled {
        tokio::time::timeout(Duration::from_secs(5), input_task).await???;
    } else {
        input_task.abort();
    }
    Ok(ProcessOutcome {
        status,
        stdout,
        stderr,
        cancelled,
    })
}

struct ProcessGuard {
    pid: Option<u32>,
    armed: bool,
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_process_tree(self.pid);
        }
    }
}

fn kill_process_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    let _ = std::process::Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill.exe")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
async fn read_task_for_process(worker: &Worker, path: &Path) -> Option<Task> {
    let record = read_json::<AttemptRecord>(path).ok()?;
    match worker
        .read(Action::TaskGet {
            task_id: record.task_id,
        })
        .await
        .ok()?
    {
        Response::Task { task } => Some(task),
        _ => None,
    }
}

async fn capture_output<R: AsyncRead + Unpin>(
    mut reader: R,
    path: PathBuf,
    limit: usize,
) -> Result<CapturedOutput> {
    let temp = path.with_extension(format!("{}.tmp", new_id()));
    let mut file = private_new(&temp)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let keep = read.min(limit.saturating_sub(bytes.len()));
        file.write_all(&buffer[..keep])?;
        file.sync_data()?;
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, &path)?;
    sync_parent(&path)?;
    Ok(CapturedOutput { bytes, truncated })
}

fn parse_result(outcome: &ProcessOutcome) -> Result<TaskResult> {
    if outcome.stdout.truncated {
        return Ok(failed_result(
            "Claude stdout exceeded the capture limit; inspect private logs",
        ));
    }
    let parsed: ClaudeResult = serde_json::from_slice(&outcome.stdout.bytes)
        .context("Claude stdout is not an official result JSON object")?;
    ensure!(parsed.kind == "result", "Claude JSON type is not result");
    let denied = !parsed.permission_denials.is_empty();
    let success = outcome.status.success()
        && parsed.subtype.as_deref() == Some("success")
        && !parsed.is_error
        && !denied
        && parsed.errors.is_empty()
        && parsed
            .result
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let mut remaining = Vec::new();
    if !success {
        remaining.push(
            "operator review required; see private stdout/stderr and permission policy".into(),
        );
    }
    if denied {
        remaining.push(format!(
            "{} permission request(s) were denied",
            parsed.permission_denials.len()
        ));
    }
    if outcome.stderr.truncated {
        remaining.push("stderr exceeded the capture limit".into());
    }
    Ok(TaskResult {
        summary: short(
            parsed
                .result
                .as_deref()
                .unwrap_or("Claude did not return a successful result"),
        ),
        completed: if success {
            vec!["Claude returned a success result JSON object".into()]
        } else {
            Vec::new()
        },
        remaining,
        verification: vec![format!(
            "Claude subtype={:?}, exit={:?}; model report was not independently verified",
            parsed.subtype.as_deref().map(short),
            outcome.status.code()
        )],
        artifact_ids: Vec::new(),
        failed: !success,
    })
}

fn failed_result(summary: &str) -> TaskResult {
    TaskResult {
        summary: summary.into(),
        completed: Vec::new(),
        remaining: vec!["inspect private logs before requesting a new attempt".into()],
        verification: Vec::new(),
        artifact_ids: Vec::new(),
        failed: true,
    }
}

fn build_prompt(task: &Task) -> Result<String> {
    Ok(format!(
        "Execute this user-authorized task in the configured local working directory. \
         Remote paths in the task are data only and do not select the cwd. \
         Do not claim checks you did not run. Report completed work, remaining work, \
         and actual verification evidence.\n\n{}",
        serde_json::to_string_pretty(&task.spec)?
    ))
}

fn short(value: &str) -> String {
    if value.len() <= MAX_REPORT_BYTES {
        return value.into();
    }
    let mut end = MAX_REPORT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated; see private log]", &value[..end])
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    let _ = command;
}

fn terminate_process(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "private directory must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows_private(path, true)?;
    Ok(())
}

fn private_new(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    windows_private(path, true)?;
    Ok(file)
}

fn check_private(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "private file must be a regular file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "private file must be mode 600"
        );
    }
    #[cfg(windows)]
    windows_private(path, false)?;
    Ok(())
}

#[cfg(windows)]
fn windows_private(path: &Path, set: bool) -> Result<()> {
    let script = if set {
        "$ErrorActionPreference='Stop'; $p=$env:P2PCLI_WORKER_PRIVATE_PATH; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $a=Get-Acl -LiteralPath $p; $a.SetAccessRuleProtection($true,$false); foreach($r in @($a.Access)){$a.RemoveAccessRuleSpecific($r)}; if((Get-Item -LiteralPath $p).PSIsContainer){$r=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow')}else{$r=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','Allow')}; $a.AddAccessRule($r); Set-Acl -LiteralPath $p -AclObject $a"
    } else {
        "$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a=Get-Acl -LiteralPath $env:P2PCLI_WORKER_PRIVATE_PATH; foreach($r in $a.Access){if($r.AccessControlType -eq 'Allow' -and $r.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -notin @($sid,'S-1-5-18','S-1-5-32-544')){exit 1}}"
    };
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("P2PCLI_WORKER_PRIVATE_PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("cannot enforce worker private ACL")?;
    ensure!(status.success(), "worker private ACL check/update failed");
    Ok(())
}

fn persist<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= MAX_RECORD_BYTES as usize,
        "worker record is too large"
    );
    let temp = path.with_extension(format!("{}.tmp", new_id()));
    let result = (|| {
        let mut file = private_new(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path.parent().context("path has no parent")?)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let metadata = fs::metadata(path)?;
    ensure!(
        metadata.len() <= MAX_RECORD_BYTES,
        "worker file is too large"
    );
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid worker JSON"))
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod worker_tests;
