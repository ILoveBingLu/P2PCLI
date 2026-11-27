//! 端口转发（对方电脑上的端口接到本机）：按设备记，不跟着哪条连接走。
//!
//! 关了终端标签、网络断了，转发都不停：本机端口一直听着，下一个连接进来时现找一条连接，
//! 没有就重连（和命令行 p2pcli exec 一样用连接历史里的认证）。勾了「记住」的写进 forwards.json，
//! 本机端口固定下来，下次打开软件自动恢复，那时不先连对方，有连接进来才连。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use p2pcli_core::client::RemoteHost;
use p2pcli_core::ports::{Forward, RemoteSource};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tracing::{info, warn};

use crate::AppState;

/// 一个正在进行的转发
pub(crate) struct Entry {
    endpoint_id: String,
    name: String,
    forward: Forward,
    /// 记住了：下次打开软件自动恢复
    saved: bool,
}

/// 记在 forwards.json 里的一条
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Saved {
    endpoint_id: String,
    name: String,
    remote_port: u16,
    local_port: u16,
}

#[derive(Serialize)]
pub(crate) struct ForwardView {
    id: u64,
    endpoint_id: String,
    name: String,
    remote_port: u16,
    local_port: u16,
    /// 正在转的连接数
    connections: usize,
    saved: bool,
    /// 本机端口被占、上一次没连上对方
    problem: Option<String>,
}

fn view(id: u64, e: &Entry) -> ForwardView {
    ForwardView {
        id,
        endpoint_id: e.endpoint_id.clone(),
        name: e.name.clone(),
        remote_port: e.forward.remote_port,
        local_port: e.forward.local_port,
        connections: e.forward.connections(),
        saved: e.saved,
        problem: e.forward.problem(),
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join("forwards.json")
}

fn load(dir: &Path) -> Vec<Saved> {
    std::fs::read(file(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 把记住的那些写回文件
fn store(state: &AppState) -> Result<(), String> {
    let mut list: Vec<(u64, Saved)> = state
        .forwards
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, e)| e.saved)
        .map(|(id, e)| {
            (*id, Saved { endpoint_id: e.endpoint_id.clone(), name: e.name.clone(), remote_port: e.forward.remote_port, local_port: e.forward.local_port })
        })
        .collect();
    list.sort_by_key(|(id, _)| *id);
    let list: Vec<Saved> = list.into_iter().map(|(_, s)| s).collect();
    let bytes = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
    p2pcli_core::store::write_private(&file(&state.data_dir), &bytes).map_err(|e| format!("保存端口转发失败：{e:#}"))
}

/// 这台设备现在界面里开着的、最新的那条连接
fn open_connection(state: &AppState, endpoint_id: &str) -> Option<(u64, Arc<RemoteHost>)> {
    state
        .remotes
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, r)| r.info.endpoint_id == endpoint_id && !r.is_closed())
        .max_by_key(|(id, _)| **id)
        .map(|(id, r)| (*id, r.clone()))
}

/// 转发用的连接：界面里开着的优先；没有就按连接历史重连。新连上的先等一会儿直连（中继只兜底）
fn source(app: AppHandle, endpoint_id: String) -> RemoteSource {
    let last: Arc<Mutex<Option<Arc<RemoteHost>>>> = Default::default();
    Arc::new(move || {
        let (app, id, last) = (app.clone(), endpoint_id.clone(), last.clone());
        Box::pin(async move {
            let state = app.state::<AppState>();
            let remote = match open_connection(&state, &id) {
                Some((_, r)) => r,
                None => {
                    let node = state.node();
                    let device = node.find_device(&id)?;
                    node.remote_for(&device).await?
                }
            };
            let fresh = !last.lock().unwrap().as_ref().is_some_and(|old| Arc::ptr_eq(old, &remote));
            if fresh {
                remote.wait_direct(p2pcli_core::path::DIRECT_WAIT).await;
                *last.lock().unwrap() = Some(remote.clone());
            }
            Ok(remote)
        })
    })
}

/// 打开软件时把记住的转发恢复起来：先在本机端口上听着，有连接进来才去连对方
pub(crate) fn restore(app: &AppHandle) {
    let state = app.state::<AppState>();
    let saved = load(&state.data_dir);
    if saved.is_empty() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        for s in saved {
            let forward = Forward::keep(source(app.clone(), s.endpoint_id.clone()), s.remote_port, s.local_port);
            let id = state.next();
            state.forwards.lock().unwrap().insert(id, Entry { endpoint_id: s.endpoint_id, name: s.name, forward, saved: true });
        }
        info!("恢复了记住的端口转发");
    });
}

/// 主窗口不要了的这条连接，转发还要不要：这台设备有转发、它又是转发正在用的那条（最新的）就留着
pub(crate) fn needs(state: &AppState, conn_id: u64) -> bool {
    let Some(endpoint_id) = state.remotes.lock().unwrap().get(&conn_id).filter(|r| !r.is_closed()).map(|r| r.info.endpoint_id.clone()) else {
        return false;
    };
    let any = state.forwards.lock().unwrap().values().any(|e| e.endpoint_id == endpoint_id);
    any && open_connection(state, &endpoint_id).is_some_and(|(id, _)| id == conn_id)
}

/// 这台设备的转发停了：主窗口早先要断、为转发留着的那些连接再看一遍。
/// 有了新连接时不急着断旧的：旧连接上可能还有正在转的连接（比如开发服务器的热更新）
pub(crate) fn release_device(state: &AppState, endpoint_id: &str) {
    let held: Vec<u64> = state.held.lock().unwrap().iter().copied().collect();
    let conns: Vec<u64> = {
        let remotes = state.remotes.lock().unwrap();
        held.into_iter().filter(|id| remotes.get(id).is_some_and(|r| r.info.endpoint_id == endpoint_id)).collect()
    };
    for conn_id in conns {
        crate::release_if_unused(state, conn_id);
    }
}

/// 把对方电脑上的端口接到本机；同一台设备的同一个端口已经在转发就直接用它。
/// local_port 指定了就只用它；keep 为真时记住，下次打开软件自动恢复
#[tauri::command]
pub(crate) async fn forward_port(
    app: AppHandle,
    state: State<'_, AppState>,
    conn_id: u64,
    remote_port: u16,
    local_port: Option<u16>,
    keep: Option<bool>,
) -> Result<ForwardView, String> {
    let remote = state.remote(conn_id)?;
    let endpoint_id = remote.info.endpoint_id.clone();
    if let Some((id, e)) = state.forwards.lock().unwrap().iter().find(|(_, e)| e.endpoint_id == endpoint_id && e.forward.remote_port == remote_port) {
        return Ok(view(*id, e));
    }
    let source = source(app.clone(), endpoint_id.clone());
    let forward = Forward::start_via(remote.clone(), source, remote_port, local_port).await.map_err(|e| format!("{e:#}"))?;
    let id = state.next();
    let entry = Entry { endpoint_id, name: remote.info.name.clone(), forward, saved: keep.unwrap_or(false) };
    let out = view(id, &entry);
    state.forwards.lock().unwrap().insert(id, entry);
    if out.saved {
        store(&state)?;
    }
    Ok(out)
}

/// 记住（或不再记住）一个转发：记住以后本机端口固定，下次打开软件自动恢复
#[tauri::command]
pub(crate) fn keep_forward(state: State<'_, AppState>, id: u64, keep: bool) -> Result<ForwardView, String> {
    let out = {
        let mut map = state.forwards.lock().unwrap();
        let e = map.get_mut(&id).ok_or("这个转发已经停了")?;
        e.saved = keep;
        view(id, e)
    };
    store(&state)?;
    Ok(out)
}

/// 停止转发；记住的也一起忘掉
#[tauri::command]
pub(crate) fn stop_forward(state: State<'_, AppState>, id: u64) {
    let Some(entry) = state.forwards.lock().unwrap().remove(&id) else { return };
    if entry.saved
        && let Err(e) = store(&state)
    {
        warn!("{e}");
    }
    drop(entry.forward);
    release_device(&state, &entry.endpoint_id);
}

#[tauri::command]
pub(crate) fn port_forwards(state: State<'_, AppState>) -> Vec<ForwardView> {
    let mut list: Vec<_> = state.forwards.lock().unwrap().iter().map(|(id, e)| view(*id, e)).collect();
    list.sort_by_key(|f| f.id);
    list
}

/// 在浏览器里打开转发过来的端口；地址只能是本机的转发端口，界面只给路径
#[tauri::command]
pub(crate) fn open_forward(state: State<'_, AppState>, id: u64, path: String, https: Option<bool>) -> Result<(), String> {
    let port = state.forwards.lock().unwrap().get(&id).map(|e| e.forward.local_port).ok_or("这个转发已经停了")?;
    crate::links::open_localhost(port, &path, https.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("p2pcli-forwards-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir).is_empty(), "没有文件时是空的");
        let list = vec![Saved { endpoint_id: "abc".into(), name: "Mac".into(), remote_port: 5173, local_port: 5174 }];
        p2pcli_core::store::write_private(&file(&dir), &serde_json::to_vec(&list).unwrap()).unwrap();
        assert_eq!(load(&dir), list);
        std::fs::write(file(&dir), b"not json").unwrap();
        assert!(load(&dir).is_empty(), "坏文件当成没有");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
