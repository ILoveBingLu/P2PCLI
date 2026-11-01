//! A private, durable outbox. Only enqueue operations without execution side effects
//! are retried unattended; claims and execution updates require their caller.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Duration;

#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{Action, Reply, Request, Response, MAX_MESSAGE_BYTES};
use crate::node::Node;
use crate::store::now_secs;

static FILE_GATE: Mutex<()> = Mutex::new(());
const MAX_RECORDS: usize = 10_000;
const MAX_QUEUE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    id: String,
    target: Option<String>,
    request: Request,
    created_at: u64,
    updated_at: u64,
    state: String,
    attempts: u32,
    next_retry: u64,
    last_error: Option<String>,
    reply: Option<Reply>,
}

/// Never return session credentials or request bodies through queue inspection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub request_id: String,
    pub target: Option<String>,
    pub project_id: Option<String>,
    pub operation: String,
    pub state: String,
    pub attempts: u32,
    pub updated_at: u64,
    pub last_error: Option<String>,
}

fn automatic(action: &Action) -> bool {
    matches!(action, Action::TaskCreate { .. } | Action::MessageSend { .. })
}

fn permanent_transport_error(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}");
    message.contains("unsupported_capability") || message.contains("forbidden")
        || message.contains("对方拒绝") || message.contains("配对密码")
}

fn directory(data_dir: &Path) -> PathBuf { data_dir.join("collaboration-outbox") }

fn key(target: Option<&str>, request_id: &str) -> String {
    let bytes = serde_json::to_vec(&(target, request_id)).expect("string serialization");
    blake3::hash(&bytes).to_hex().to_string()
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn persist(path: &Path, record: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension(format!("{}.tmp", super::new_id()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)] {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec(record)?)?;
        file.sync_all()?;
        replace_file(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(path.parent().context("outbox path has no parent")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(tmp); }
    result
}

fn replace_file(tmp: &Path, path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }

        let from: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let to: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

fn read(path: &Path) -> Result<Record> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > crate::proto::MAX_FRAME as u64 * 2 { bail!("outbox record too large"); }
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn paths(data_dir: &Path) -> Result<Vec<PathBuf>> {
    let dir = directory(data_dir);
    if !dir.exists() { return Ok(Vec::new()); }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") { result.push(path); }
    }
    Ok(result)
}

pub fn list(data_dir: &Path) -> Result<Vec<Delivery>> {
    let _gate = FILE_GATE.lock().map_err(|_| anyhow::anyhow!("outbox lock poisoned"))?;
    let mut result = Vec::new();
    for path in paths(data_dir)? {
        let r = read(&path)?;
        let operation = serde_json::to_value(&r.request.action)?["type"].as_str().unwrap_or("unknown").to_owned();
        let last_error = r.last_error.map(|error| {
            r.request.auth.as_ref().map_or(error.clone(), |auth| error.replace(&auth.token, "[redacted]"))
        });
        result.push(Delivery {
            id: r.id, request_id: r.request.request_id, target: r.target,
            project_id: r.request.project_id, operation, state: r.state,
            attempts: r.attempts, updated_at: r.updated_at, last_error,
        });
    }
    result.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
    Ok(result)
}

fn prepare(data_dir: &Path, target: Option<String>, request: &Request) -> Result<(PathBuf, Record)> {
    if request.request_id.is_empty() || request.request_id.len() > 128 { bail!("request_id must contain 1-128 bytes"); }
    if serde_json::to_vec(request)?.len() > MAX_MESSAGE_BYTES { bail!("collaboration request exceeds 64 KiB"); }
    let _gate = FILE_GATE.lock().map_err(|_| anyhow::anyhow!("outbox lock poisoned"))?;
    let dir = directory(data_dir);
    private_directory(&dir)?;
    let id = key(target.as_deref(), &request.request_id);
    let path = dir.join(format!("{id}.json"));
    if path.exists() {
        let old = read(&path)?;
        if old.request != *request || old.target != target { bail!("idempotency_conflict: request ID already has different content"); }
        return Ok((path, old));
    }
    let existing = paths(data_dir)?;
    let mut bytes = 0u64;
    for path in &existing { bytes = bytes.saturating_add(std::fs::metadata(path)?.len()); }
    if existing.len() >= MAX_RECORDS || bytes + serde_json::to_vec(request)?.len() as u64 > MAX_QUEUE_BYTES {
        bail!("outbox capacity reached; inspect delivery records before submitting more work");
    }
    let now = now_secs();
    let record = Record {
        id, target, request: request.clone(), created_at: now, updated_at: now,
        state: "pending".into(), attempts: 0, next_retry: now, last_error: None, reply: None,
    };
    persist(&path, &record)?;
    Ok((path, record))
}

fn mark_attempt(path: &Path) -> Result<Record> {
    let _gate = FILE_GATE.lock().map_err(|_| anyhow::anyhow!("outbox lock poisoned"))?;
    let mut record = read(path)?;
    if record.reply.is_none() {
        record.attempts = record.attempts.saturating_add(1);
        record.updated_at = now_secs();
        record.next_retry = record.updated_at + 30;
        persist(path, &record)?;
    }
    Ok(record)
}

fn finish(path: &Path, result: &Result<Reply>) -> Result<()> {
    let _gate = FILE_GATE.lock().map_err(|_| anyhow::anyhow!("outbox lock poisoned"))?;
    let mut record = read(path)?;
    // A late transport failure must never replace another caller's durable response.
    if record.reply.is_some() { return Ok(()); }
    record.updated_at = now_secs();
    match result {
        Ok(reply) => {
            let transient_storage = reply
                .error
                .as_ref()
                .is_some_and(|error| error.code == "storage_error");
            record.state = if transient_storage {
                if automatic(&record.request.action) { "pending" } else { "needs_retry" }
            } else if reply.error.is_some() {
                "rejected"
            } else {
                "delivered"
            }
            .into();
            record.last_error = reply
                .error
                .as_ref()
                .map(|e| format!("{}: {}", e.code, e.message));
            if !transient_storage {
                record.reply = Some(reply.clone());
            }
        }
        Err(error) => {
            let message = format!("{error:#}");
            let permanent = permanent_transport_error(error);
            record.state = if automatic(&record.request.action) && !permanent { "pending" } else { "needs_retry" }.into();
            record.last_error = Some(message);
            record.next_retry = now_secs() + (2u64.saturating_pow(record.attempts.min(8))).min(300);
        }
    }
    persist(path, &record)
}

/// Stable IDs survive connection loss. A queued response never implies execution.
pub async fn dispatch(node: &Node, target: Option<&str>, request: Request) -> Result<Reply> {
    let target = match target.filter(|s| !s.trim().is_empty() && *s != "local") {
        Some(t) if t != node.id().to_string() => Some(node.find_device(t)?.endpoint_id),
        _ => None,
    };
    if request.action.is_read() {
        if matches!(request.action, Action::Inbox) {
            return super::inbox::receive(node, target, request).await;
        }
        return node.collaborate(target.as_deref(), request).await;
    }
    let (path, _) = prepare(node.host.collaboration.data_dir(), target, &request)?;
    let record = mark_attempt(&path)?;
    if record.reply.is_some() {
        // Revalidate current permissions at the coordinator even for cached writes.
        // This also prevents cached credentials being disclosed after membership revocation.
        return match node.collaborate(record.target.as_deref(), request).await {
            Ok(reply) => Ok(reply),
            Err(error) if record.reply.as_ref().is_some_and(|reply| reply.error.is_none())
                && !permanent_transport_error(&error) => Ok(Reply::ok(Response::CommittedUnverified {
                request_id: record.request.request_id,
                message: format!(
                    "协调设备暂时不可达；本机已记录这次请求的提交结果，但尚未重新核对权限和远端状态：{error:#}"
                ),
            })),
            Err(error) => Err(error),
        };
    }
    let result = node.collaborate(record.target.as_deref(), request).await;
    finish(&path, &result)?;
    match result {
        Err(error) if automatic(&record.request.action) => {
            let current = read(&path)?;
            if current.state == "pending" {
                Ok(Reply::ok(Response::Queued {
                    request_id: current.request.request_id,
                    message: "已保存在本机发件队列，连接恢复后重试；目标会话尚未领取".into(),
                }))
            } else { Err(error) }
        }
        other => other,
    }
}

pub fn start(node: &Arc<Node>) {
    let epoch = super::inbox::advance_epoch(&node.host.collaboration.delivery_epoch);
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let Some(node) = weak.upgrade() else { break };
            if node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != epoch { break; }
            if let Err(error) = super::inbox::sync(&node, epoch).await {
                tracing::warn!(%error, "协作收件同步失败");
            }
            let candidates = match paths(node.host.collaboration.data_dir()) {
                Ok(paths) => paths,
                Err(error) => { tracing::warn!(%error, "无法读取协作发件队列"); continue; }
            };
            let mut sent = 0;
            for path in candidates {
                if node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != epoch { break; }
                let Ok(record) = read(&path) else { continue };
                if record.state != "pending" || record.next_retry > now_secs() || !automatic(&record.request.action) { continue; }
                let Ok(record) = mark_attempt(&path) else { continue };
                if record.reply.is_some() { continue; }
                let result = node.collaborate(record.target.as_deref(), record.request).await;
                if node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != epoch {
                    continue;
                }
                if let Err(error) = finish(&path, &result) { tracing::warn!(%error, "无法保存协作投递状态"); }
                sent += 1;
                if sent >= 8 { break; }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_retry_identity_and_private_queue_inspection() {
        let dir = std::env::temp_dir().join(format!("p2pcli-delivery-{}", super::super::new_id()));
        let request = Request::new(None, Action::ProjectCreate { name: "test".into() });
        let (path, record) = prepare(&dir, None, &request).unwrap();
        assert_eq!(prepare(&dir, None, &request).unwrap().1.id, record.id);
        let mut conflict = request.clone();
        conflict.action = Action::ProjectCreate { name: "other".into() };
        assert!(prepare(&dir, None, &conflict).is_err());
        mark_attempt(&path).unwrap();
        finish(&path, &Ok(Reply::ok(Response::Ack))).unwrap();
        let committed = std::fs::read(&path).unwrap();
        finish(&path, &Err(anyhow::anyhow!("late disconnect"))).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        finish(&path, &Ok(Reply::error("storage_error", "late storage failure"))).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        assert_eq!(read(&path).unwrap().state, "delivered");
        let rendered = serde_json::to_string(&list(&dir).unwrap()).unwrap();
        assert!(!rendered.contains("\"action\""));
        assert!(!rendered.contains("\"auth\""));
        assert_eq!(list(&dir).unwrap()[0].attempts, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn storage_errors_remain_retryable_for_automatic_and_manual_operations() {
        let dir = std::env::temp_dir().join(format!("p2pcli-delivery-storage-{}", super::super::new_id()));
        let assignee = super::super::SessionRef {
            device_id: "device".into(),
            session_id: "session".into(),
        };
        let cases = [
            (Action::TaskCreate {
                spec: super::super::TaskSpec {
                    title: "test".into(),
                    goal: "verify retry".into(),
                    acceptance: String::new(),
                    inputs: serde_json::Value::Null,
                    assignee: assignee.clone(),
                },
            }, "pending"),
            (Action::MessageSend {
                to: assignee,
                task_id: None,
                body: "message".into(),
            }, "pending"),
            (Action::TaskClaim { task_id: "task".into(), expected_version: 1 }, "needs_retry"),
            (Action::TaskCancel { task_id: "task".into(), expected_version: 1 }, "needs_retry"),
        ];

        for (action, expected_state) in cases {
            let request = Request::new(Some("project".into()), action);
            let target = Some("target".to_owned());
            let (path, initial) = prepare(&dir, target.clone(), &request).unwrap();
            // Reopening the same request must not return a permanently cached storage failure.
            for attempts in 1..=2 {
                let attempted = mark_attempt(&path).unwrap();
                finish(&path, &Ok(Reply::error("storage_error", "database temporarily unavailable"))).unwrap();
                let (retry_path, retry) = prepare(&dir, target.clone(), &request).unwrap();
                assert_eq!(retry_path, path);
                assert_eq!(retry.id, initial.id);
                assert_eq!(retry.request, request);
                assert_eq!(retry.state, expected_state, "{:?}", request.action);
                assert_eq!(retry.attempts, attempts);
                assert_eq!(retry.next_retry, attempted.next_retry);
                assert_eq!(retry.last_error.as_deref(), Some("storage_error: database temporarily unavailable"));
                assert!(retry.reply.is_none(), "storage errors must not be cached");
                let listed = list(&dir).unwrap().into_iter().find(|item| item.id == initial.id).unwrap();
                assert_eq!(listed.state, expected_state);
                assert_eq!(listed.attempts, attempts);
            }

            mark_attempt(&path).unwrap();
            let success = Reply::ok(Response::Ack);
            finish(&path, &Ok(success.clone())).unwrap();
            let recovered = prepare(&dir, target, &request).unwrap().1;
            assert_eq!(recovered.state, "delivered");
            assert_eq!(recovered.attempts, 3);
            assert_eq!(recovered.reply, Some(success));
            assert!(recovered.last_error.is_none());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn business_rejection_stays_cached_despite_late_storage_or_transport_errors() {
        let dir = std::env::temp_dir().join(format!("p2pcli-delivery-rejected-{}", super::super::new_id()));
        let request = Request::new(Some("project".into()), Action::MessageSend {
            to: super::super::SessionRef { device_id: "device".into(), session_id: "session".into() },
            task_id: None,
            body: "message".into(),
        });
        let (path, _) = prepare(&dir, Some("target".into()), &request).unwrap();
        mark_attempt(&path).unwrap();
        let rejection = Reply::error("forbidden", "membership revoked");
        finish(&path, &Ok(rejection.clone())).unwrap();
        let rejected = read(&path).unwrap();
        assert_eq!(rejected.state, "rejected");
        assert_eq!(rejected.reply, Some(rejection));
        let committed = std::fs::read(&path).unwrap();

        finish(&path, &Ok(Reply::error("storage_error", "late storage failure"))).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        finish(&path, &Err(anyhow::anyhow!("late disconnect"))).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        mark_attempt(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), committed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delivery_listing_redacts_auth_tokens_from_transport_errors() {
        let dir = std::env::temp_dir().join(format!("p2pcli-delivery-redaction-{}", super::super::new_id()));
        let mut request = Request::new(Some("project".into()), Action::MessageSend {
            to: super::super::SessionRef { device_id: "device".into(), session_id: "session".into() },
            task_id: None,
            body: "message".into(),
        });
        request.auth = Some(super::super::SessionAuth {
            session_id: "sender".into(),
            token: "secret-token".into(),
        });
        let (path, _) = prepare(&dir, Some("target".into()), &request).unwrap();
        mark_attempt(&path).unwrap();
        finish(&path, &Err(anyhow::anyhow!("transport failed with secret-token"))).unwrap();
        let rendered = serde_json::to_string(&list(&dir).unwrap()).unwrap();
        assert!(!rendered.contains("secret-token"));
        assert!(!rendered.contains("\"auth\""));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn execution_mutations_are_never_retried_without_the_caller() {
        assert!(!automatic(&Action::TaskClaim { task_id: "t".into(), expected_version: 1 }));
        assert!(!automatic(&Action::TaskCancel { task_id: "t".into(), expected_version: 1 }));
        assert!(automatic(&Action::MessageSend { to: super::super::SessionRef {
            device_id: "d".into(), session_id: "s".into(),
        }, task_id: None, body: "hello".into() }));
    }
}
