//! 牵线服务器：给设备发 9 位设备码，记谁在线、在哪，按设备码查；
//! 两边都有公网 IPv6 时替连接方叫被连方同时发包。终端数据不经过这里。

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey};
use p2pcli_core::ALPN_RDV;
use p2pcli_core::proto::{read_json, write_json};
use p2pcli_core::rendezvous::{RdvPush, RdvRequest, RdvResponse, normalize_code};
use p2pcli_core::store::now_secs;
use rusqlite::{Connection as Db, OptionalExtension, params};
use tracing::{debug, info, warn};

/// 每台设备每分钟最多查几次设备码（含请求发包），防被人挨个试
const LOOKUPS_PER_MINUTE: usize = 20;
/// 转给被连方的地址最多几个
const MAX_PUNCH_ADDRS: usize = 6;

struct Online {
    conn: Connection,
    addr: Option<EndpointAddr>,
}

pub struct State {
    db: Mutex<Db>,
    online: Mutex<HashMap<EndpointId, Online>>,
    lookups: Mutex<HashMap<EndpointId, VecDeque<Instant>>>,
}

impl State {
    pub fn open(db_path: &Path) -> Result<Arc<Self>> {
        let db = Db::open(db_path).context("打开设备库失败")?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS devices (
                 endpoint_id TEXT PRIMARY KEY,
                 code TEXT NOT NULL UNIQUE,
                 name TEXT NOT NULL,
                 platform TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 last_seen INTEGER NOT NULL
             );",
        )?;
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            online: Mutex::new(HashMap::new()),
            lookups: Mutex::new(HashMap::new()),
        }))
    }
}

/// 起服务器的 iroh 节点；IPv6 端口绑不上就只用 IPv4
pub async fn bind(key: SecretKey, port: u16, relays: Vec<RelayUrl>) -> Result<Endpoint> {
    let base = || {
        let b = Endpoint::builder(presets::N0)
            .secret_key(key.clone())
            .alpns(vec![ALPN_RDV.to_vec()]);
        if relays.is_empty() {
            b
        } else {
            b.relay_mode(iroh::RelayMode::custom(relays.clone()))
        }
    };
    let dual = base()
        .bind_addr(format!("0.0.0.0:{port}").as_str())?
        .bind_addr(format!("[::]:{port}").as_str())?
        .bind()
        .await;
    match dual {
        Ok(ep) => Ok(ep),
        Err(e) => {
            warn!("IPv6 端口绑不上（{e}），只用 IPv4");
            Ok(base()
                .bind_addr(format!("0.0.0.0:{port}").as_str())?
                .bind()
                .await?)
        }
    }
}

/// 一直接收设备连接，直到节点关闭
pub async fn serve(endpoint: Endpoint, state: Arc<State>) {
    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            let conn = match incoming.accept() {
                Ok(a) => match a.await {
                    Ok(c) => c,
                    Err(e) => return debug!("握手失败：{e}"),
                },
                Err(e) => return debug!("拒收：{e}"),
            };
            if conn.alpn() == ALPN_RDV {
                serve_device(state, conn).await;
            }
        });
    }
}

async fn serve_device(state: Arc<State>, conn: Connection) {
    let remote = conn.remote_id();
    let conn_id = conn.stable_id();
    debug!(remote = %remote.fmt_short(), "设备连上");
    loop {
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(_) => break,
        };
        let req: RdvRequest = match read_json(&mut recv).await {
            Ok(r) => r,
            Err(e) => {
                debug!("请求格式不对：{e:#}");
                continue;
            }
        };
        let resp = handle(&state, &conn, req).await.unwrap_or_else(|e| {
            warn!("处理请求出错：{e:#}");
            RdvResponse::Error {
                message: "服务器出错".into(),
            }
        });
        let _ = write_json(&mut send, &resp).await;
        let _ = send.finish();
    }
    let mut online = state.online.lock().unwrap();
    // 同一台设备可能已经换了新连接，只删自己这条
    if online
        .get(&remote)
        .is_some_and(|o| o.conn.stable_id() == conn_id)
    {
        online.remove(&remote);
        info!(remote = %remote.fmt_short(), "设备离线");
    }
}

async fn handle(state: &State, conn: &Connection, req: RdvRequest) -> Result<RdvResponse> {
    let remote = conn.remote_id();
    match req {
        RdvRequest::Register { name, platform } => {
            let name: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
            let platform: String = platform.chars().filter(|c| !c.is_control()).take(16).collect();
            let code = register(&state.db.lock().unwrap(), &remote, &name, &platform)?;
            state.online.lock().unwrap().insert(
                remote,
                Online {
                    conn: conn.clone(),
                    addr: None,
                },
            );
            info!(remote = %remote.fmt_short(), %code, %name, "设备登记");
            Ok(RdvResponse::Registered { code })
        }
        RdvRequest::Addr { addr } => {
            if addr.id != remote {
                return Ok(RdvResponse::Error {
                    message: "只能报告自己的地址".into(),
                });
            }
            {
                let mut online = state.online.lock().unwrap();
                match online.get_mut(&remote) {
                    Some(o) if o.conn.stable_id() == conn.stable_id() => o.addr = Some(addr),
                    _ => {
                        return Ok(RdvResponse::Error {
                            message: "请先登记".into(),
                        });
                    }
                }
            }
            state.db.lock().unwrap().execute(
                "UPDATE devices SET last_seen = ?1 WHERE endpoint_id = ?2",
                params![now_secs() as i64, remote.to_string()],
            )?;
            Ok(RdvResponse::Ok)
        }
        RdvRequest::Lookup { code } => {
            if !allow_lookup(state, remote) {
                return Ok(too_often());
            }
            let Some((endpoint_id, name, platform)) = find(state, &code)? else {
                return Ok(RdvResponse::NotFound);
            };
            let online = state.online.lock().unwrap();
            let entry = online.get(&endpoint_id);
            Ok(RdvResponse::Found {
                endpoint_id,
                name,
                platform,
                online: entry.is_some(),
                addr: entry.and_then(|o| o.addr.clone()),
            })
        }
        RdvRequest::Punch { code, addrs } => {
            if !allow_lookup(state, remote) {
                return Ok(too_often());
            }
            let Some((target, ..)) = find(state, &code)? else {
                return Ok(RdvResponse::NotFound);
            };
            let Some(target_conn) = state
                .online
                .lock()
                .unwrap()
                .get(&target)
                .map(|o| o.conn.clone())
            else {
                return Ok(RdvResponse::Error {
                    message: "对方不在线".into(),
                });
            };
            let addrs: Vec<SocketAddr> = addrs
                .into_iter()
                .filter(|a| a.is_ipv6())
                .take(MAX_PUNCH_ADDRS)
                .collect();
            let push = RdvPush::Punch {
                from: remote,
                addrs,
            };
            // 推送失败不影响连接方，它还会走常规流程
            let sent = tokio::time::timeout(Duration::from_secs(3), async {
                let mut s = target_conn.open_uni().await?;
                write_json(&mut s, &push).await?;
                s.finish()?;
                anyhow::Ok(())
            })
            .await;
            debug!(from = %remote.fmt_short(), to = %target.fmt_short(), ok = matches!(sent, Ok(Ok(()))), "转达发包请求");
            Ok(RdvResponse::Ok)
        }
    }
}

fn too_often() -> RdvResponse {
    RdvResponse::Error {
        message: "查询太频繁，请稍后再试".into(),
    }
}

fn find(state: &State, code: &str) -> Result<Option<(EndpointId, String, String)>> {
    let Some(code) = normalize_code(code) else {
        return Ok(None);
    };
    let row: Option<(String, String, String)> = state
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT endpoint_id, name, platform FROM devices WHERE code = ?1",
            params![code],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    match row {
        Some((id, name, platform)) => Ok(Some((id.parse()?, name, platform))),
        None => Ok(None),
    }
}

/// 同一把钥匙永远对应同一个码；新设备随机发一个没用过的
fn register(db: &Db, id: &EndpointId, name: &str, platform: &str) -> Result<String> {
    let now = now_secs() as i64;
    let existing: Option<String> = db
        .query_row(
            "SELECT code FROM devices WHERE endpoint_id = ?1",
            params![id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(code) = existing {
        db.execute(
            "UPDATE devices SET name = ?1, platform = ?2, last_seen = ?3 WHERE endpoint_id = ?4",
            params![name, platform, now, id.to_string()],
        )?;
        return Ok(code);
    }
    for _ in 0..20 {
        let code = rand::random_range(100_000_000u32..1_000_000_000).to_string();
        let inserted = db.execute(
            "INSERT OR IGNORE INTO devices (endpoint_id, code, name, platform, created_at, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id.to_string(), code, name, platform, now],
        )?;
        if inserted == 1 {
            return Ok(code);
        }
    }
    anyhow::bail!("发设备码失败")
}

fn allow_lookup(state: &State, remote: EndpointId) -> bool {
    let now = Instant::now();
    let mut map = state.lookups.lock().unwrap();
    let q = map.entry(remote).or_default();
    while q
        .front()
        .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
    {
        q.pop_front();
    }
    if q.len() >= LOOKUPS_PER_MINUTE {
        return false;
    }
    q.push_back(now);
    true
}
