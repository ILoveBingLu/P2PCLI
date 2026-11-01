//! Collaboration CLI. Credentials and exact retry requests stay in private files.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::{Args, Subcommand, ValueEnum};
use p2pcli_core::collaboration::model::*;
use p2pcli_core::control;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

#[derive(Args)]
pub struct Cli {
    /// Coordinator device (prefer its full public key); defaults to local
    #[arg(long, global = true)]
    coordinator: Option<String>,
    /// Explicit project ID; credentials never select another project implicitly
    #[arg(long, global = true)]
    project: Option<String>,
    /// Private session credential file; register writes this file
    #[arg(long, global = true)]
    session_file: Option<PathBuf>,
    /// Stable operation ID for retrying the identical request
    #[arg(long, global = true)]
    request_id: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Project { #[command(subcommand)] command: ProjectCommand },
    Member { #[command(subcommand)] command: MemberCommand },
    Session { #[command(subcommand)] command: SessionCommand },
    Task { #[command(subcommand)] command: TaskCommand },
    Message { #[command(subcommand)] command: MessageCommand },
    /// Messages addressed to this authenticated session
    Inbox,
    /// Durable local collaboration delivery records; this command has no project/session scope
    Deliveries,
    /// Read events after a persisted cursor (save the returned cursor after processing)
    Events {
        #[arg(long, default_value_t = 0)] after: u64,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    Artifact { #[command(subcommand)] command: ArtifactCommand },
    /// Send a full Request JSON or replay an exact private pending request file
    Request { #[arg(long)] file: PathBuf },
    /// Run a managed adapter using its explicit configuration
    Worker { #[arg(long)] config: PathBuf },
}

#[derive(Subcommand)]
enum ProjectCommand {
    Create { #[arg(long)] name: String },
    List,
    Show,
}

#[derive(Clone, Copy, ValueEnum)]
enum MemberRole { Admin, Dispatcher, Worker, Viewer }

impl From<MemberRole> for Role {
    fn from(role: MemberRole) -> Self {
        match role {
            MemberRole::Admin => Self::Admin, MemberRole::Dispatcher => Self::Dispatcher,
            MemberRole::Worker => Self::Worker, MemberRole::Viewer => Self::Viewer,
        }
    }
}

#[derive(Subcommand)]
enum MemberCommand {
    List,
    Set { device: String, #[arg(long, value_enum)] role: MemberRole },
    Revoke { device: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    #[value(alias = "manual_pull")] ManualPull,
    #[value(alias = "managed_adapter")] ManagedAdapter,
}

#[derive(Subcommand)]
enum SessionCommand {
    Register {
        #[arg(long)] name: String,
        #[arg(long, value_enum, default_value = "manual-pull")] mode: Mode,
        #[arg(long)] cwd: Option<String>,
        /// Optional TerminalRef JSON file
        #[arg(long)] terminal_file: Option<PathBuf>,
    },
    List,
    Heartbeat {
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        available: bool,
    },
}

#[derive(Args)]
struct TaskVersion {
    task_id: String,
    /// Expected version; omitted means read once, then compare-and-swap
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    version: Option<u64>,
}

#[derive(Args)]
struct Assignee {
    #[arg(long)] device: String,
    #[arg(long)] session: String,
}

impl Assignee {
    fn reference(self) -> SessionRef { SessionRef { device_id: self.device, session_id: self.session } }
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Text {
    #[arg(long, alias = "summary", alias = "body")] message: Option<String>,
    /// UTF-8 text file
    #[arg(long)] file: Option<PathBuf>,
}

impl Text {
    fn read(self) -> Result<String> {
        match (self.message, self.file) {
            (Some(text), None) => Ok(text),
            (None, Some(path)) => String::from_utf8(read_bytes(&path)?).context("text file must be UTF-8"),
            _ => bail!("supply either --message or --file"),
        }
    }
}

#[derive(Subcommand)]
enum TaskCommand {
    Create {
        /// TaskSpec JSON; alternatively use title, goal and assignee flags
        #[arg(long, conflicts_with_all = ["title", "goal", "device", "session", "acceptance", "inputs_file"])]
        file: Option<PathBuf>,
        #[arg(long, required_unless_present = "file")] title: Option<String>,
        #[arg(long, required_unless_present = "file")] goal: Option<String>,
        #[arg(long, required_unless_present = "file")] device: Option<String>,
        #[arg(long, required_unless_present = "file")] session: Option<String>,
        #[arg(long)] acceptance: Option<String>,
        /// JSON inputs, including repository and baseline commit
        #[arg(long)] inputs_file: Option<PathBuf>,
    },
    List,
    Show { task_id: String },
    Claim(TaskVersion),
    Progress {
        #[command(flatten)] task: TaskVersion,
        #[arg(long)] attempt: String,
        #[command(flatten)] text: Text,
        #[arg(long)] blocked: bool,
    },
    Result {
        #[command(flatten)] task: TaskVersion,
        #[arg(long)] attempt: String,
        /// UTF-8 summary text, or TaskResult JSON
        #[command(flatten)] text: Text,
        #[arg(long)] completed: Vec<String>,
        #[arg(long)] remaining: Vec<String>,
        #[arg(long)] verification: Vec<String>,
        #[arg(long)] artifact: Vec<String>,
        #[arg(long)] failed: bool,
    },
    Cancel(TaskVersion),
    Stop {
        #[command(flatten)] task: TaskVersion,
        #[arg(long)] attempt: String,
    },
    Accept(TaskVersion),
    Requeue(TaskVersion),
    Assign { #[command(flatten)] task: TaskVersion, #[command(flatten)] assignee: Assignee },
    /// Wait for result_ready or a terminal state; timeout never cancels the task
    Wait {
        task_id: String,
        #[arg(long, default_value_t = 60)] timeout: u64,
        #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u64).range(1..))]
        poll_ms: u64,
    },
}

#[derive(Subcommand)]
enum MessageCommand {
    Send {
        #[command(flatten)] to: Assignee,
        #[arg(long)] task: Option<String>,
        #[command(flatten)] text: Text,
    },
    Consume { message_id: String },
}

#[derive(Subcommand)]
enum ArtifactCommand {
    List { task_id: String },
    Declare {
        task_id: String,
        #[arg(long)] attempt: String,
        #[arg(long)] name: String,
        #[arg(long)] size: u64,
        /// BLAKE3 digest of the artifact bytes
        #[arg(long)] hash: String,
    },
    /// On the coordinator only: verify an already received file
    Receipt { artifact_id: String, #[arg(long)] file: PathBuf },
}

#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub coordinator: Option<String>,
    pub project_id: String,
    pub auth: SessionAuth,
}

pub fn normalize_coordinator(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("local")).map(str::to_owned)
}

/// Exact scope matching is intentional: device aliases cannot silently redirect a token.
pub fn load_credentials(path: &Path, coordinator: Option<&str>, project_id: &str) -> Result<SessionAuth> {
    check_private(path)?;
    let credentials: Credentials = read_json(path)?;
    ensure!(credentials.project_id == project_id, "credential project does not match --project");
    ensure!(normalize_coordinator(credentials.coordinator.as_deref()) == normalize_coordinator(coordinator),
        "credential coordinator does not match --coordinator; use the same device identifier as registration");
    ensure!(!credentials.auth.session_id.is_empty() && !credentials.auth.token.is_empty(), "invalid session credential");
    Ok(credentials.auth)
}

fn check_private(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).context("cannot inspect private file")?;
    ensure!(meta.is_file() && !meta.file_type().is_symlink(), "private file must be a regular file");
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        ensure!(meta.permissions().mode() & 0o077 == 0, "private file must not be accessible by group or others; use chmod 600");
    }
    #[cfg(windows)]
    windows_private(path, false)?;
    Ok(())
}

#[cfg(windows)]
fn windows_private(path: &Path, set: bool) -> Result<()> {
    // Set the ACL before writing any secret bytes. Reject inherited broad access on reads.
    let script = if set {
        "$p=$env:P2PCLI_PRIVATE_PATH; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $a=Get-Acl -LiteralPath $p; $a.SetAccessRuleProtection($true,$false); foreach($r in @($a.Access)){$a.RemoveAccessRuleSpecific($r)}; $r=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','Allow'); $a.AddAccessRule($r); Set-Acl -LiteralPath $p -AclObject $a"
    } else {
        "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a=Get-Acl -LiteralPath $env:P2PCLI_PRIVATE_PATH; foreach($r in $a.Access){if($r.AccessControlType -eq 'Allow' -and $r.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value -notin @($sid,'S-1-5-18','S-1-5-32-544')){exit 1}}"
    };
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &format!("$ErrorActionPreference='Stop'; {script}")])
        .env("P2PCLI_PRIVATE_PATH", path)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null()).status()?;
    ensure!(status.success(), "private file ACL could not be secured or verified");
    Ok(())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).context("cannot open input file")?
        .take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1024 * 1024, "input file exceeds 1 MiB");
    Ok(bytes)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    // serde errors can echo attacker-provided values, including credentials.
    serde_json::from_slice(&read_bytes(path)?).map_err(|_| anyhow!("invalid JSON file or unsupported fields"))
}

/// Atomically install a private file without replacing a different session/request.
fn save_private(path: &Path, value: &impl Serialize) -> Result<()> {
    let path = std::path::absolute(path)?;
    let parent = path.parent().context("private file needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".{}.tmp", new_id()));
    let outcome = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        #[cfg(windows)]
        windows_private(&tmp, true)?;
        file.write_all(&serde_json::to_vec_pretty(value)?)?;
        file.sync_all()?;
        drop(file);
        // hard_link publishes complete content and fails atomically if the name exists.
        std::fs::hard_link(&tmp, &path).context("private file already exists or cannot be saved")?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(tmp);
    outcome
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
struct Pending {
    coordinator: Option<String>,
    project_id: Option<String>,
    /// The intent before fetching a task version (zero means unspecified).
    intent: Request,
    request: Request,
    credential_file: Option<PathBuf>,
}

fn pending_path(dir: &Path, request_id: &str) -> Result<PathBuf> {
    ensure!(!request_id.is_empty() && request_id.len() <= 128 &&
        request_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "request ID must contain 1-128 ASCII letters, digits, '-' or '_'");
    Ok(dir.join("collaboration-pending").join(format!("{request_id}.json")))
}

fn needs_auth(action: &Action) -> bool {
    matches!(action, Action::SessionHeartbeat { .. } | Action::TaskClaim { .. } |
        Action::TaskProgress { .. } | Action::TaskResult { .. } | Action::TaskStopAck { .. } |
        Action::MessageSend { .. } | Action::MessageDelivered { .. } | Action::MessageConsume { .. } | Action::Inbox |
        Action::ArtifactDeclare { .. })
}

fn task_version(action: &mut Action) -> Option<(&str, &mut u64)> {
    match action {
        Action::TaskAssign { task_id, expected_version, .. } |
        Action::TaskClaim { task_id, expected_version } |
        Action::TaskProgress { task_id, expected_version, .. } |
        Action::TaskResult { task_id, expected_version, .. } |
        Action::TaskCancel { task_id, expected_version } |
        Action::TaskStopAck { task_id, expected_version, .. } |
        Action::TaskAccept { task_id, expected_version } |
        Action::TaskRequeue { task_id, expected_version } => Some((task_id, expected_version)),
        _ => None,
    }
}

pub async fn send_request(dir: &Path, coordinator: Option<&str>, request: Request) -> Result<Reply> {
    super::ask(dir, control::Request::Collab { target: normalize_coordinator(coordinator), request }, |r| {
        match r { control::Reply::Collab { reply } => Some(reply), _ => None }
    }).await
}

fn check_pending(saved: &Pending, coordinator: Option<&str>, intent: &Request) -> Result<()> {
    ensure!(saved.coordinator == normalize_coordinator(coordinator) &&
        saved.project_id == intent.project_id && saved.intent == *intent,
        "idempotency_conflict: request ID is already bound to a different scope or operation");
    ensure!(saved.request.project_id == saved.project_id &&
        saved.request.request_id == intent.request_id && saved.request.auth == intent.auth,
        "invalid pending request scope");
    let mut original = saved.request.action.clone();
    let mut desired = intent.action.clone();
    if let (Some((_, old)), Some((_, new))) = (task_version(&mut original), task_version(&mut desired)) {
        if *new == 0 { *old = 0; }
    }
    ensure!(original == desired, "invalid pending request content");
    Ok(())
}

async fn prepare_request(
    dir: &Path, coordinator: Option<&str>, mut request: Request, credential_file: Option<PathBuf>,
) -> Result<(Request, Option<PathBuf>)> {
    if request.action.is_read() { return Ok((request, None)); }
    let path = pending_path(dir, &request.request_id)?;
    let intent = request.clone();
    if path.try_exists()? {
        check_private(&path)?;
        let saved: Pending = read_json(&path)?;
        check_pending(&saved, coordinator, &intent)?;
        return Ok((saved.request, Some(path)));
    }
    if let Some((task, version)) = task_version(&mut request.action) {
        if *version == 0 {
            let mut read = Request::new(request.project_id.clone(), Action::TaskGet { task_id: task.to_owned() });
            read.auth = request.auth.clone();
            match send_request(dir, coordinator, read).await?.into_result()? {
                Response::Task { task } => *version = task.version,
                _ => bail!("coordinator did not return the task version"),
            }
        }
    }
    let pending = Pending {
        coordinator: normalize_coordinator(coordinator), project_id: request.project_id.clone(),
        intent, request: request.clone(), credential_file,
    };
    if let Err(error) = save_private(&path, &pending) {
        // Another CLI may have published the same operation while the read was in flight.
        if !path.try_exists()? { return Err(error); }
        check_private(&path)?;
        let saved: Pending = read_json(&path)?;
        check_pending(&saved, coordinator, &pending.intent)?;
        return Ok((saved.request, Some(path)));
    }
    Ok((request, Some(path)))
}

fn output_value(reply: &Reply, credential_file: Option<&Path>, request_id: &str, pending: Option<&Path>) -> Result<Value> {
    let mut value = match &reply.response {
        Some(Response::Queued { request_id, message }) => json!({
            "delivery": "queued_local",
            "request_id": request_id,
            "message": message,
        }),
        Some(Response::CommittedUnverified { request_id, message }) => json!({
            "delivery": "committed_unverified",
            "request_id": request_id,
            "message": message,
        }),
        Some(Response::Registered { session, .. }) => json!({
            "response": { "type": "registered", "session": session, "credential_file": credential_file }
        }),
        _ => serde_json::to_value(reply)?,
    };
    value["request_id"] = json!(request_id);
    if let Some(path) = pending { value["request_file"] = json!(path); }
    Ok(value)
}

fn print_output(value: &Value, json_output: bool) {
    emit(value, json_output, &mut std::io::stdout().lock(), &mut std::io::stderr().lock())
        .expect("cannot write collaboration output");
}

fn emit(value: &Value, json_output: bool, output: &mut impl Write, errors: &mut impl Write) -> Result<()> {
    if json_output { writeln!(output, "{}", serde_json::to_string_pretty(value)?)?; }
    else {
        if let Some(error) = value.get("error") {
            writeln!(errors, "{}: {}", error["code"].as_str().unwrap_or("error"), error["message"].as_str().unwrap_or(""))?;
        } else if value.get("delivery").and_then(Value::as_str) == Some("queued_local") {
            writeln!(output, "已保存到本机协作发件队列，等待协调设备恢复连接；目标尚未领取或消费。")?;
        } else if value.get("delivery").and_then(Value::as_str) == Some("committed_unverified") {
            writeln!(output, "本机已记录这次请求的提交结果；权限和远端状态待核实。请在连接恢复后使用同一请求 ID 核实，再判断任务是否完成。")?;
        } else if let Some(response) = value.get("response") {
            // Structured pretty output keeps task inputs, attempts and verification intact.
            writeln!(output, "{}", serde_json::to_string_pretty(response)?)?;
        }
        if let Some(path) = value.get("request_file").and_then(Value::as_str) {
            writeln!(errors, "Request {} saved at {path}", value["request_id"].as_str().unwrap_or(""))?;
        }
    }
    Ok(())
}

fn registered_file(dir: &Path, options: Option<&Path>, session: &AgentSession) -> Result<PathBuf> {
    match options {
        Some(path) => Ok(std::path::absolute(path)?),
        None => {
            ensure!(session.id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'), "invalid session ID");
            Ok(std::path::absolute(dir.join("collaboration-sessions").join(format!("{}.json", session.id)))?)
        }
    }
}

fn store_credentials(path: &Path, credentials: &Credentials) -> Result<()> {
    if path.try_exists()? {
        let existing = load_credentials(path, credentials.coordinator.as_deref(), &credentials.project_id)?;
        ensure!(existing == credentials.auth, "credential file already belongs to another session; choose a new --session-file");
        return Ok(());
    }
    save_private(path, credentials)
}

async fn execute(
    dir: &Path, coordinator: Option<&str>, request: Request,
    session_file: Option<&Path>, json_output: bool,
) -> Result<ExitCode> {
    let (request, pending) = prepare_request(dir, coordinator, request, session_file.map(Path::to_path_buf)).await?;
    let id = request.request_id.clone();
    let reply = match send_request(dir, coordinator, request.clone()).await {
        Ok(reply) => reply,
        Err(error) => {
            let mut message = format!("{error:#}");
            if let Some(auth) = &request.auth { message = message.replace(&auth.token, "[redacted]"); }
            print_output(&json!({
                "error": { "code": "transport_error", "message": message },
                "delivery": "unknown", "request_id": id, "request_file": pending,
            }), json_output);
            return Ok(ExitCode::from(if message.contains("unsupported_capability") { 3 } else { 1 }));
        }
    };
    let credential_file = if let Some(Response::Registered { session, token }) = &reply.response {
        ensure!(request.project_id.as_deref() == Some(session.project_id.as_str()), "registration returned a different project");
        let path = registered_file(dir, session_file, session)?;
        let credentials = Credentials {
            coordinator: normalize_coordinator(coordinator), project_id: session.project_id.clone(),
            auth: SessionAuth { session_id: session.id.clone(), token: token.clone() },
        };
        store_credentials(&path, &credentials).with_context(|| format!(
            "credential could not be saved; retry the same request ID {id} using its pending request file"
        ))?;
        Some(path)
    } else { None };
    let mut value = output_value(&reply, credential_file.as_deref(), &id, pending.as_deref())?;
    if let Some(auth) = &request.auth {
        // Errors from a peer must not echo our credential into stdout/stderr.
        if let Some(message) = value.pointer_mut("/error/message") {
            if let Some(text) = message.as_str() { *message = json!(text.replace(&auth.token, "[redacted]")); }
        }
    }
    print_output(&value, json_output);
    Ok(ExitCode::from(match &reply.error {
        Some(error) if error.code == "unsupported_capability" => 3,
        Some(_) => 1, None => 0,
    }))
}

pub async fn run(cli: Cli, dir: &Path, json_output: bool) -> Result<ExitCode> {
    let coordinator = normalize_coordinator(cli.coordinator.as_deref());
    let mut project = cli.project;
    let mut session_file = cli.session_file.map(std::path::absolute).transpose()?;
    let mut supplied_id = cli.request_id;
    let action = match cli.command {
        Command::Deliveries => {
            ensure!(coordinator.is_none() && project.is_none() && session_file.is_none() && supplied_id.is_none(),
                "collab deliveries does not accept coordinator, project, session-file, or request-id");
            let mut result = None;
            let mut error = None;
            control::request(dir, &control::Request::CollabDeliveries, |reply| match reply {
                control::Reply::CollabDeliveries { deliveries } => result = Some(deliveries),
                control::Reply::Error { message } => error = Some(message),
                _ => {}
            }).await?;
            if let Some(error) = error { bail!("transport: {error}"); }
            let deliveries = result.context("P2PCLI 没有回投递记录")?;
            if json_output { super::print_json(&deliveries); } else {
                for delivery in deliveries {
                    println!("{}  {}  {}  {}  {}",
                        delivery.state, delivery.operation, delivery.request_id,
                        delivery.target.as_deref().unwrap_or("local"),
                        delivery.last_error.as_deref().unwrap_or(""));
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::Worker { config } => {
            ensure!(coordinator.is_none() && project.is_none() && session_file.is_none() && supplied_id.is_none(),
                "worker scope is selected by --config; do not pass collaboration scope flags");
            crate::worker::run(&config, dir).await?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Request { file } => {
            let value: Value = read_json(&file)?;
            if value.get("request").is_some() {
                check_private(&file)?;
                let saved: Pending = serde_json::from_value(value).map_err(|_| anyhow!("invalid pending request file"))?;
                ensure!(coordinator == saved.coordinator, "pending request coordinator does not match --coordinator");
                if let Some(project) = &project { ensure!(saved.project_id.as_ref() == Some(project), "pending request project mismatch"); }
                if let Some(id) = &supplied_id { ensure!(id == &saved.request.request_id, "pending request ID mismatch"); }
                check_pending(&saved, coordinator.as_deref(), &saved.intent)?;
                if let Some(file) = &session_file {
                    let auth = load_credentials(file, coordinator.as_deref(), saved.project_id.as_deref().context("project required")?)?;
                    ensure!(saved.request.auth.as_ref() == Some(&auth), "pending request session mismatch");
                }
                return execute(dir, coordinator.as_deref(), saved.intent,
                    session_file.as_deref().or(saved.credential_file.as_deref()), json_output).await;
            }
            let request: Request = serde_json::from_value(value).map_err(|_| anyhow!("invalid request JSON"))?;
            ensure!(request.auth.is_none(), "raw requests must omit auth; use --session-file");
            if let Some(id) = &supplied_id { ensure!(id == &request.request_id, "request ID mismatch"); }
            if let Some(id) = &project { ensure!(request.project_id.as_ref() == Some(id), "request project mismatch"); }
            project = request.project_id;
            supplied_id = Some(request.request_id);
            request.action
        }
        Command::Project { command } => match command {
            ProjectCommand::Create { name } => Action::ProjectCreate { name },
            ProjectCommand::List => Action::ProjectsList,
            ProjectCommand::Show => Action::ProjectGet,
        },
        Command::Member { command } => match command {
            MemberCommand::List => Action::MembersList,
            MemberCommand::Set { device, role } => Action::MemberSet { device_id: device, role: role.into(), revoked: false },
            MemberCommand::Revoke { device } => {
                // Revocation preserves the existing role; retry intent remains stable.
                let existing = if let Some(id) = &supplied_id {
                    let path = pending_path(dir, id)?;
                    if path.try_exists()? {
                        check_private(&path)?;
                        let pending: Pending = read_json(&path)?;
                        ensure!(pending.coordinator == coordinator && pending.project_id == project, "pending request scope mismatch");
                        match pending.intent.action {
                            Action::MemberSet { device_id, role, revoked: true } if device_id == device => Some(role),
                            _ => bail!("idempotency_conflict: request ID belongs to another operation"),
                        }
                    } else { None }
                } else { None };
                let role = match existing {
                    Some(role) => role,
                    None => {
                        ensure!(project.is_some(), "--project is required");
                        let req = Request::new(project.clone(), Action::MembersList);
                        match send_request(dir, coordinator.as_deref(), req).await?.into_result()? {
                            Response::Members { members } => members.into_iter().find(|m| m.device_id == device)
                                .context("member not found")?.role,
                            _ => bail!("coordinator did not return members"),
                        }
                    }
                };
                Action::MemberSet { device_id: device, role, revoked: true }
            }
        },
        Command::Session { command } => match command {
            SessionCommand::Register { name, mode, cwd, terminal_file } => {
                if let Some(path) = &session_file {
                    if path.try_exists()? {
                        ensure!(supplied_id.is_some(), "credential file already exists; use a new path for a new session");
                    }
                }
                Action::SessionRegister { name, mode: match mode {
                    Mode::ManualPull => SessionMode::ManualPull, Mode::ManagedAdapter => SessionMode::ManagedAdapter,
                }, cwd, terminal: terminal_file.as_deref().map(read_json).transpose()? }
            }
            SessionCommand::List => Action::SessionsList,
            SessionCommand::Heartbeat { available } => Action::SessionHeartbeat { available },
        },
        Command::Task { command } => match command {
            TaskCommand::Create { file, title, goal, device, session, acceptance, inputs_file } => {
                let spec = match file {
                    Some(file) => read_json(&file)?,
                    None => TaskSpec {
                        title: title.context("--title is required")?, goal: goal.context("--goal is required")?,
                        acceptance: acceptance.unwrap_or_default(),
                        inputs: inputs_file.as_deref().map(read_json).transpose()?.unwrap_or(Value::Null),
                        assignee: SessionRef { device_id: device.context("--device is required")?, session_id: session.context("--session is required")? },
                    },
                };
                Action::TaskCreate { spec }
            }
            TaskCommand::List => Action::TasksList,
            TaskCommand::Show { task_id } => Action::TaskGet { task_id },
            TaskCommand::Claim(task) => Action::TaskClaim { task_id: task.task_id, expected_version: task.version.unwrap_or(0) },
            TaskCommand::Progress { task, attempt, text, blocked } => Action::TaskProgress {
                task_id: task.task_id, attempt_id: attempt, message: text.read()?, blocked, expected_version: task.version.unwrap_or(0),
            },
            TaskCommand::Result { task, attempt, text, completed, remaining, verification, artifact, failed } => {
                let from_file = text.file.is_some();
                let summary = text.read()?;
                let result = if from_file && summary.trim_start().starts_with('{') {
                    ensure!(completed.is_empty() && remaining.is_empty() && verification.is_empty() && artifact.is_empty() && !failed,
                        "TaskResult JSON cannot be combined with result detail flags");
                    serde_json::from_str(&summary).map_err(|_| anyhow!("invalid TaskResult JSON"))?
                } else { TaskResult { summary, completed, remaining, verification, artifact_ids: artifact, failed } };
                Action::TaskResult { task_id: task.task_id, attempt_id: attempt, result, expected_version: task.version.unwrap_or(0) }
            }
            TaskCommand::Cancel(task) => Action::TaskCancel { task_id: task.task_id, expected_version: task.version.unwrap_or(0) },
            TaskCommand::Stop { task, attempt } => Action::TaskStopAck {
                task_id: task.task_id, attempt_id: attempt, expected_version: task.version.unwrap_or(0),
            },
            TaskCommand::Accept(task) => Action::TaskAccept { task_id: task.task_id, expected_version: task.version.unwrap_or(0) },
            TaskCommand::Requeue(task) => Action::TaskRequeue { task_id: task.task_id, expected_version: task.version.unwrap_or(0) },
            TaskCommand::Assign { task, assignee } => Action::TaskAssign {
                task_id: task.task_id, assignee: assignee.reference(), expected_version: task.version.unwrap_or(0),
            },
            TaskCommand::Wait { task_id, timeout, poll_ms } => {
                let mut req = Request::new(project.clone(), Action::TaskGet { task_id });
                attach_auth(&mut req, coordinator.as_deref(), session_file.as_deref())?;
                return wait(dir, coordinator.as_deref(), req, timeout, poll_ms, json_output).await;
            }
        },
        Command::Message { command } => match command {
            MessageCommand::Send { to, task, text } => Action::MessageSend { to: to.reference(), task_id: task, body: text.read()? },
            MessageCommand::Consume { message_id } => Action::MessageConsume { message_id },
        },
        Command::Inbox => Action::Inbox,
        Command::Events { after, limit } => Action::EventsRead { after, limit: limit.into() },
        Command::Artifact { command } => match command {
            ArtifactCommand::List { task_id } => Action::ArtifactsList { task_id },
            ArtifactCommand::Declare { task_id, attempt, name, size, hash } => Action::ArtifactDeclare {
                task_id, attempt_id: attempt, name, size, hash,
            },
            ArtifactCommand::Receipt { artifact_id, file } => {
                ensure!(coordinator.is_none(), "artifact receipt must run locally on the coordinator");
                Action::ArtifactReceipt { artifact_id, local_path: std::path::absolute(file)?.to_string_lossy().into_owned() }
            }
        },
    };
    let registering = matches!(action, Action::SessionRegister { .. });
    let mut request = Request::new(project, action);
    if let Some(id) = supplied_id { request.request_id = id; }
    if registering {
        ensure!(request.project_id.as_ref().is_some_and(|p| !p.is_empty()), "--project is required");
    } else { attach_auth(&mut request, coordinator.as_deref(), session_file.as_deref())?; }
    if registering {
        // The output path is part of the pending record even before credentials exist.
        session_file = session_file.map(std::path::absolute).transpose()?;
    }
    execute(dir, coordinator.as_deref(), request, session_file.as_deref(), json_output).await
}

fn attach_auth(request: &mut Request, coordinator: Option<&str>, session_file: Option<&Path>) -> Result<()> {
    if matches!(request.action, Action::ProjectCreate { .. } | Action::ProjectsList) {
        ensure!(request.project_id.is_none() && session_file.is_none(), "project create/list do not accept --project or --session-file");
        return Ok(());
    }
    let project = request.project_id.as_deref().filter(|p| !p.is_empty()).context("--project is required")?;
    if let Some(file) = session_file { request.auth = Some(load_credentials(file, coordinator, project)?); }
    ensure!(!needs_auth(&request.action) || request.auth.is_some(), "this operation requires --session-file");
    Ok(())
}

async fn wait(dir: &Path, coordinator: Option<&str>, request: Request, seconds: u64, poll_ms: u64, json_output: bool) -> Result<ExitCode> {
    let deadline = tokio::time::Instant::now().checked_add(Duration::from_secs(seconds)).context("timeout is too large")?;
    loop {
        let reply = match tokio::time::timeout_at(deadline, send_request(dir, coordinator, request.clone())).await {
            Ok(reply) => reply?,
            Err(_) => {
                print_output(&json!({"error":{"code":"wait_timeout","message":"wait timed out; task was not cancelled"}}), json_output);
                return Ok(ExitCode::from(4));
            }
        };
        if reply.error.is_some() {
            print_output(&output_value(&reply, None, &request.request_id, None)?, json_output);
            return Ok(ExitCode::FAILURE);
        }
        match &reply.response {
            Some(Response::Task { task }) if matches!(task.state,
                TaskState::ResultReady | TaskState::Completed | TaskState::Failed | TaskState::Cancelled) => {
                print_output(&output_value(&reply, None, &request.request_id, None)?, json_output);
                return Ok(ExitCode::SUCCESS);
            }
            Some(Response::Task { .. }) => {},
            _ => bail!("coordinator did not return a task"),
        }
        tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + Duration::from_millis(poll_ms))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self { Self(std::env::temp_dir().join(format!("p2pcli-cli-{}", new_id()))) }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn committed_unverified_text_reports_pending_verification_and_keeps_retry_reference() {
        let reply = Reply::ok(Response::CommittedUnverified {
            request_id: "retry-1".into(),
            message: "coordinator unreachable; cached commit awaits revalidation".into(),
        });
        let value = output_value(&reply, None, "retry-1", Some(Path::new("pending.json"))).unwrap();
        let mut output = Vec::new();
        let mut errors = Vec::new();
        emit(&value, false, &mut output, &mut errors).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("本机已记录这次请求的提交结果"));
        assert!(text.contains("权限和远端状态待核实"));
        assert!(text.contains("同一请求 ID"));
        for misleading in ["失败", "已完成", "queued_local", "目标尚未领取"] {
            assert!(!text.contains(misleading), "{text}");
        }
        assert_eq!(String::from_utf8(errors).unwrap(), "Request retry-1 saved at pending.json\n");
    }

    #[test]
    fn committed_unverified_json_preserves_delivery_without_claiming_task_outcome() {
        let message = "coordinator unreachable; cached commit awaits revalidation";
        let reply = Reply::ok(Response::CommittedUnverified {
            request_id: "retry-1".into(), message: message.into(),
        });
        let value = output_value(&reply, None, "retry-1", Some(Path::new("pending.json"))).unwrap();
        let mut output = Vec::new();
        let mut errors = Vec::new();
        emit(&value, true, &mut output, &mut errors).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), json!({
            "delivery": "committed_unverified",
            "request_id": "retry-1",
            "message": message,
            "request_file": "pending.json",
        }));
        assert!(errors.is_empty());
        assert!(reply.error.is_none());
    }

    #[test]
    fn emit_keeps_queued_errors_and_normal_responses_distinct() {
        for (value, expected_output, expected_error) in [
            (
                output_value(&Reply::ok(Response::Queued {
                    request_id: "queued-1".into(), message: "queued".into(),
                }), None, "queued-1", None).unwrap(),
                "已保存到本机协作发件队列，等待协调设备恢复连接；目标尚未领取或消费。\n",
                "",
            ),
            (
                output_value(&Reply::error("forbidden", "access revoked"), None, "r", None).unwrap(),
                "",
                "forbidden: access revoked\n",
            ),
            (
                output_value(&Reply::ok(Response::Ack), None, "r", None).unwrap(),
                "{\n  \"type\": \"ack\"\n}\n",
                "",
            ),
        ] {
            let mut output = Vec::new();
            let mut errors = Vec::new();
            emit(&value, false, &mut output, &mut errors).unwrap();
            assert_eq!(String::from_utf8(output).unwrap(), expected_output);
            assert_eq!(String::from_utf8(errors).unwrap(), expected_error);
        }
    }

    #[test]
    fn command_tree_and_existing_commands_parse() {
        crate::Cli::command().debug_assert();
        for args in [
            vec!["collab", "project", "create", "--name", "demo"],
            vec!["collab", "session", "register", "--project", "p", "--name", "worker", "--mode", "managed_adapter"],
            vec!["collab", "task", "create", "--project", "p", "--title", "t", "--goal", "g", "--device", "d", "--session", "s"],
            vec!["collab", "task", "claim", "t", "--project", "p", "--session-file", "s.json"],
            vec!["collab", "task", "progress", "t", "--attempt", "a", "--file", "progress.txt", "--version", "2"],
            vec!["collab", "task", "result", "t", "--attempt", "a", "--summary", "done", "--verification", "tests passed"],
            vec!["collab", "worker", "--config", "worker.json"],
            vec!["collab", "--coordinator", "remote", "request", "--file", "pending.json"],
            vec!["exec", "device", "--", "echo", "hello"],
            vec!["send", "device", "file.txt", "--to", "~/work"],
        ] {
            assert!(crate::Cli::try_parse_from(std::iter::once("p2pcli").chain(args.clone())).is_ok(), "{args:?}");
        }
        assert!(crate::Cli::try_parse_from(["p2pcli", "collab", "task", "create", "--title", "missing"]).is_err());
        assert!(crate::Cli::try_parse_from(["p2pcli", "collab", "message", "send", "--device", "d", "--session", "s", "--message", "m", "--file", "f"]).is_err());
    }

    #[test]
    fn credentials_are_private_bound_and_never_rendered() {
        let dir = Temp::new();
        let path = dir.0.join("session.json");
        let credentials = Credentials {
            coordinator: None, project_id: "project-a".into(),
            auth: SessionAuth { session_id: "session-a".into(), token: "secret-do-not-print".into() },
        };
        save_private(&path, &credentials).unwrap();
        assert_eq!(load_credentials(&path, Some("LOCAL"), "project-a").unwrap(), credentials.auth);
        assert!(load_credentials(&path, None, "project-b").is_err());
        assert!(load_credentials(&path, Some("remote"), "project-a").is_err());
        assert!(save_private(&path, &credentials).is_err());
        let reply = Reply::ok(Response::Registered {
            session: AgentSession { id: "session-a".into(), project_id: "project-a".into(), device_id: "device".into(),
                name: "worker".into(), mode: SessionMode::ManagedAdapter, terminal: None, cwd: None, available: true, last_seen: 1 },
            token: credentials.auth.token.clone(),
        });
        let output = output_value(&reply, Some(&path), "r", None).unwrap().to_string();
        assert!(!output.contains(&credentials.auth.token));
        assert!(!output.contains("\"token\""));
        assert!(output.contains("credential_file"));
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(load_credentials(&path, None, "project-a").is_err());
        }
    }

    #[tokio::test]
    async fn retries_keep_original_cas_without_a_live_daemon() {
        let dir = Temp::new();
        let mut intent = Request::new(Some("p".into()), Action::TaskClaim { task_id: "t".into(), expected_version: 0 });
        intent.auth = Some(SessionAuth { session_id: "s".into(), token: "private".into() });
        let mut request = intent.clone();
        *task_version(&mut request.action).unwrap().1 = 7;
        let saved = Pending { coordinator: Some("remote".into()), project_id: Some("p".into()), intent: intent.clone(), request: request.clone(), credential_file: None };
        let path = pending_path(&dir.0, &intent.request_id).unwrap();
        save_private(&path, &saved).unwrap();
        let (retry, _) = prepare_request(&dir.0, Some("remote"), intent.clone(), None).await.unwrap();
        assert_eq!(retry, request);
        assert!(prepare_request(&dir.0, Some("other"), intent.clone(), None).await.is_err());
        intent.project_id = Some("different".into());
        assert!(prepare_request(&dir.0, Some("remote"), intent, None).await.is_err());
    }

    #[tokio::test]
    async fn writes_are_durable_before_transport() {
        let dir = Temp::new();
        let request = Request::new(None, Action::ProjectCreate { name: "demo".into() });
        let (_, path) = prepare_request(&dir.0, None, request.clone(), None).await.unwrap();
        let path = path.unwrap();
        check_private(&path).unwrap();
        let saved: Pending = read_json(&path).unwrap();
        assert_eq!(saved.request, request);
        assert_eq!(saved.coordinator, None);
        assert_eq!(saved.project_id, None);
    }
}
