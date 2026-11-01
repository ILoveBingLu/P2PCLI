//! Persist a recipient snapshot before acknowledging delivery to its coordinator.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, atomic::{AtomicU64, Ordering}};

use anyhow::{Context, Result, bail};
use futures_util::{stream, StreamExt};
use serde::{Deserialize, Serialize};

use super::{Action, Message, Reply, Request, Response, delivery};
use crate::{node::Node, store::now_secs};

static GATE: Mutex<()> = Mutex::new(());

pub(crate) fn advance_epoch(current: &AtomicU64) -> u64 {
    let _guard = GATE.lock().unwrap_or_else(|error| error.into_inner());
    current.fetch_add(1, Ordering::SeqCst) + 1
}

#[derive(Serialize, Deserialize)]
struct Inbox {
    target: Option<String>,
    request: Request,
    messages: Vec<Message>,
    updated_at: u64,
    #[serde(default)]
    retry_after: u64,
    active: bool,
}

fn root(data_dir: &Path) -> PathBuf { data_dir.join("collaboration-inboxes") }

fn snapshot_path(data_dir: &Path, target: Option<&str>, request: &Request) -> Result<PathBuf> {
    let auth = request.auth.as_ref().context("session_required: inbox requires a registered session")?;
    let key = serde_json::to_vec(&(target, &request.project_id, &auth.session_id))?;
    Ok(root(data_dir).join(format!("{}.json", blake3::hash(&key).to_hex())))
}

fn persist_snapshot(
    data_dir: &Path,
    target: Option<String>,
    request: &Request,
    messages: &[Message],
    generation: Option<(&AtomicU64, u64)>,
) -> Result<()> {
    let _guard = GATE.lock().map_err(|_| anyhow::anyhow!("inbox lock poisoned"))?;
    if generation.is_some_and(|(current, expected)| current.load(Ordering::SeqCst) != expected) {
        bail!("obsolete inbox synchronization");
    }
    let dir = root(data_dir);
    delivery::private_directory(&dir)?;
    let path = snapshot_path(data_dir, target.as_deref(), request)?;
    if !path.exists() && std::fs::read_dir(&dir)?.count() >= 100 {
        bail!("inbox_limit: at most 100 registered inbox subscriptions");
    }
    let inbox = Inbox { target, request: request.clone(), messages: messages.to_vec(), updated_at: now_secs(), retry_after: 0, active: true };
    delivery::persist(&path, &inbox)
}

fn deactivate(data_dir: &Path, target: Option<&str>, request: &Request) -> Result<()> {
    let path = snapshot_path(data_dir, target, request)?;
    let _guard = GATE.lock().map_err(|_| anyhow::anyhow!("inbox lock poisoned"))?;
    if path.exists() {
        let mut inbox: Inbox = serde_json::from_slice(&std::fs::read(&path)?)?;
        inbox.active = false;
        inbox.messages.clear();
        delivery::persist(&path, &inbox)?;
    }
    Ok(())
}

fn authorization_error(reply: &Reply) -> bool {
    reply.error.as_ref().is_some_and(|e| matches!(e.code.as_str(), "forbidden" | "session_required"))
}

pub async fn receive(node: &Node, target: Option<String>, request: Request) -> Result<Reply> {
    receive_at_epoch(node, target, request, None).await
}

async fn receive_at_epoch(
    node: &Node,
    target: Option<String>,
    request: Request,
    epoch: Option<u64>,
) -> Result<Reply> {
    snapshot_path(node.host.collaboration.data_dir(), target.as_deref(), &request)?;
    let reply = node.collaborate(target.as_deref(), request.clone()).await?;
    if epoch.is_some_and(|expected| {
        node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != expected
    }) {
        return Ok(reply);
    }
    if let Some(Response::Messages { messages }) = &reply.response {
        persist_snapshot(
            node.host.collaboration.data_dir(), target.clone(), &request, messages,
            epoch.map(|expected| (&node.host.collaboration.delivery_epoch, expected)),
        )?;
        for message in messages.iter().filter(|m| !m.delivered).take(100) {
            if epoch.is_some_and(|expected| node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != expected) {
                break;
            }
            let identity = serde_json::to_vec(&(&request.project_id, &message.id, "delivered"))?;
            let ack = Request {
                request_id: format!("delivered-{}", blake3::hash(&identity).to_hex()),
                project_id: request.project_id.clone(), auth: request.auth.clone(),
                action: Action::MessageDelivered { message_id: message.id.clone() },
            };
            // Failure is retried from the persisted inbox subscription. Never consume for the AI.
            match node.collaborate(target.as_deref(), ack).await {
                Ok(ack) if authorization_error(&ack) => {
                    deactivate(node.host.collaboration.data_dir(), target.as_deref(), &request)?;
                    return Ok(ack);
                }
                Ok(ack) if ack.error.is_none() => {}
                _ => break,
            }
        }
    } else if authorization_error(&reply) {
        deactivate(node.host.collaboration.data_dir(), target.as_deref(), &request)?;
    }
    Ok(reply)
}

pub async fn sync(node: &Node, epoch: u64) -> Result<()> {
    let dir = root(node.host.collaboration.data_dir());
    if !dir.exists() { return Ok(()); }
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => { tracing::warn!(%error, "无法读取协作收件箱记录"); continue; }
        };
        if path.extension().is_none_or(|e| e != "json") { continue; }
        let loaded = (|| -> Result<Inbox> {
            if std::fs::metadata(&path)?.len() > crate::proto::MAX_FRAME as u64 * 2 { bail!("inbox record too large"); }
            Ok(serde_json::from_slice(&std::fs::read(&path)?)?)
        })();
        match loaded {
            Ok(inbox) if inbox.active && inbox.retry_after <= now_secs()
                && now_secs().saturating_sub(inbox.updated_at) >= 15 => candidates.push((path, inbox)),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "跳过无效协作收件箱记录"),
        }
    }
    candidates.sort_by_key(|(_, inbox)| inbox.updated_at);
    stream::iter(candidates).for_each_concurrent(4, |(path, inbox)| async move {
        if node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let result = receive_at_epoch(node, inbox.target, inbox.request, Some(epoch)).await;
        if let Err(error) = result {
            tracing::debug!(%error, "协作收件箱暂不可用");
            let reschedule = (|| -> Result<()> {
                let _guard = GATE.lock().map_err(|_| anyhow::anyhow!("inbox lock poisoned"))?;
                if node.host.collaboration.delivery_epoch.load(Ordering::SeqCst) != epoch { return Ok(()); }
                let mut current: Inbox = serde_json::from_slice(&std::fs::read(&path)?)?;
                current.retry_after = now_secs() + 30;
                delivery::persist(&path, &current)
            })();
            if let Err(error) = reschedule { tracing::debug!(%error, "无法更新收件箱重试时间"); }
        }
    }).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collaboration::{SessionAuth, SessionRef, new_id};

    #[test]
    fn snapshot_is_durable_before_delivery_and_bound_to_recipient() {
        let dir = std::env::temp_dir().join(format!("p2pcli-inbox-{}", new_id()));
        let mut request = Request::new(Some("project".into()), Action::Inbox);
        request.auth = Some(SessionAuth { session_id: "recipient".into(), token: "private".into() });
        let message = Message {
            id: "m".into(), project_id: "project".into(), task_id: None,
            from: SessionRef { device_id: "a".into(), session_id: "sender".into() },
            to: SessionRef { device_id: "b".into(), session_id: "recipient".into() },
            body: "work".into(), delivered: false, consumed: false, created_at: 1,
        };
        persist_snapshot(&dir, Some("a".into()), &request, &[message], None).unwrap();
        let path = snapshot_path(&dir, Some("a"), &request).unwrap();
        let inbox: Inbox = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(inbox.messages.len(), 1);
        assert!(!inbox.messages[0].consumed);
        let generation = AtomicU64::new(2);
        assert!(persist_snapshot(&dir, Some("a".into()), &request, &[], Some((&generation, 1))).is_err());
        let preserved: Inbox = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(preserved.messages.len(), 1);
        deactivate(&dir, Some("a"), &request).unwrap();
        let revoked: Inbox = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!revoked.active);
        assert!(revoked.messages.is_empty());
        let mut another = request.clone();
        another.auth.as_mut().unwrap().session_id = "another".into();
        assert_ne!(snapshot_path(&dir, Some("a"), &request).unwrap(), snapshot_path(&dir, Some("a"), &another).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn acknowledgement_authorization_errors_are_not_success() {
        assert!(authorization_error(&Reply::error("forbidden", "revoked")));
        assert!(authorization_error(&Reply::error("session_required", "expired")));
        assert!(!authorization_error(&Reply::error("storage_error", "retry")));
        assert!(!authorization_error(&Reply::ok(Response::Ack)));
    }
}
