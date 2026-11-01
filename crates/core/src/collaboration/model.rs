use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CAPABILITY: &str = "collaboration_v1";
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_EVENTS: usize = 100;

pub fn new_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub request_id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<SessionAuth>,
    pub action: Action,
}

impl Request {
    pub fn new(project_id: Option<String>, action: Action) -> Self {
        Self { request_id: new_id(), project_id, auth: None, action }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionAuth {
    pub session_id: String,
    pub token: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role { Admin, Dispatcher, Worker, Viewer }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode { ManualPull, ManagedAdapter }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRef {
    pub device_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalRef {
    pub host_device_id: String,
    pub owner_device_id: String,
    pub terminal_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub coordinator_id: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Member {
    pub device_id: String,
    pub role: Role,
    pub revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub device_id: String,
    pub name: String,
    pub mode: SessionMode,
    pub terminal: Option<TerminalRef>,
    pub cwd: Option<String>,
    pub available: bool,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued, Running, Blocked, ResultReady, Completed, Failed, CancelRequested, Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSpec {
    pub title: String,
    pub goal: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub inputs: Value,
    pub assignee: SessionRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskResult {
    pub summary: String,
    #[serde(default)]
    pub completed: Vec<String>,
    #[serde(default)]
    pub remaining: Vec<String>,
    #[serde(default)]
    pub verification: Vec<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    #[serde(default)]
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attempt {
    pub id: String,
    pub assignee: SessionRef,
    pub started_at: u64,
    pub last_seen: u64,
    pub stopped: bool,
    pub result: Option<TaskResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    pub project_id: String,
    pub creator_device: String,
    pub creator_session: Option<String>,
    pub spec: TaskSpec,
    pub state: TaskState,
    pub version: u64,
    pub attempts: Vec<Attempt>,
    pub progress: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub from: SessionRef,
    pub to: SessionRef,
    pub body: String,
    #[serde(default)]
    pub delivered: bool,
    pub consumed: bool,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub provider_device: String,
    pub name: String,
    pub size: u64,
    pub hash: String,
    pub received: bool,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub kind: String,
    pub entity_id: String,
    pub data: Value,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    ProjectCreate { name: String },
    ProjectsList,
    ProjectGet,
    MembersList,
    MemberSet { device_id: String, role: Role, revoked: bool },
    SessionRegister { name: String, mode: SessionMode, terminal: Option<TerminalRef>, cwd: Option<String> },
    SessionsList,
    SessionHeartbeat { available: bool },
    TaskCreate { spec: TaskSpec },
    TasksList,
    TaskGet { task_id: String },
    TaskAssign { task_id: String, assignee: SessionRef, expected_version: u64 },
    TaskClaim { task_id: String, expected_version: u64 },
    TaskProgress { task_id: String, attempt_id: String, message: String, blocked: bool, expected_version: u64 },
    TaskResult { task_id: String, attempt_id: String, result: TaskResult, expected_version: u64 },
    TaskCancel { task_id: String, expected_version: u64 },
    TaskStopAck { task_id: String, attempt_id: String, expected_version: u64 },
    TaskAccept { task_id: String, expected_version: u64 },
    TaskRequeue { task_id: String, expected_version: u64 },
    MessageSend { to: SessionRef, task_id: Option<String>, body: String },
    Inbox,
    MessageDelivered { message_id: String },
    MessageConsume { message_id: String },
    EventsRead { after: u64, limit: usize },
    ArtifactDeclare { task_id: String, attempt_id: String, name: String, size: u64, hash: String },
    /// Only the coordinator's local file verifier may acknowledge an artifact.
    ArtifactReceipt { artifact_id: String, local_path: String },
    ArtifactsList { task_id: String },
}

impl Action {
    pub fn is_read(&self) -> bool {
        matches!(self, Self::ProjectsList | Self::ProjectGet | Self::MembersList |
            Self::SessionsList | Self::TasksList | Self::TaskGet { .. } | Self::Inbox |
            Self::EventsRead { .. } | Self::ArtifactsList { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Queued { request_id: String, message: String },
    CommittedUnverified { request_id: String, message: String },
    Project { project: Project },
    Projects { projects: Vec<Project> },
    Members { members: Vec<Member> },
    Registered { session: AgentSession, token: String },
    Sessions { sessions: Vec<AgentSession> },
    Task { task: Task },
    Tasks { tasks: Vec<Task> },
    Message { message: Message },
    Messages { messages: Vec<Message> },
    Events { events: Vec<Event>, cursor: u64 },
    Artifact { artifact: Artifact },
    Artifacts { artifacts: Vec<Artifact> },
    Ack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Error {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reply {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

impl Reply {
    pub fn ok(response: Response) -> Self { Self { response: Some(response), error: None } }
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self { response: None, error: Some(Error { code: code.into(), message: message.into() }) }
    }
    pub fn into_result(self) -> anyhow::Result<Response> {
        match (self.response, self.error) {
            (Some(response), None) => Ok(response),
            (_, Some(error)) => anyhow::bail!("{}: {}", error.code, error.message),
            _ => anyhow::bail!("invalid collaboration response"),
        }
    }
}
