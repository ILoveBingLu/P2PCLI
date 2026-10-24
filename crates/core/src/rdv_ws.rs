//! 牵线的 WebSocket 通道：和 UDP（iroh）那条用同一套消息，只是外面包一层 JSON 帧。
//! 有了它，牵线服务可以放到 Cloudflare Workers 这类只支持 HTTP/WebSocket 的地方。
//!
//! 连接时在请求头 `x-p2pcli-auth` 里带 `设备公钥.时间戳.签名`，签的是
//! `p2pcli-rdv-ws-v1|主机名|设备公钥|时间戳`，服务器验签认出是哪台设备；
//! 时间戳前后 5 分钟内有效，主机名写进去防止拿去别的服务器重放。
//!
//! 帧格式（文本帧）：
//! - 设备 → 服务器 `{"id": 1, "req": {...}}`
//! - 服务器 → 设备 `{"id": 1, "resp": {...}}`，或主动推送 `{"push": {...}}`

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use iroh::{EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{Connector, connect_async_tls_with_config};
use tracing::debug;

use crate::rendezvous::{RdvPush, RdvRequest, RdvResponse};

pub const WS_PATH: &str = "/v1/ws";
pub const AUTH_HEADER: &str = "x-p2pcli-auth";
/// 鉴权时间戳允许的偏差（秒）
pub const AUTH_MAX_SKEW: u64 = 300;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WsFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub req: Option<RdvRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp: Option<RdvResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<RdvPush>,
}

fn auth_message(host: &str, id: &EndpointId, ts: u64) -> String {
    format!("p2pcli-rdv-ws-v1|{}|{id}|{ts}", host.to_ascii_lowercase())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// 生成鉴权头
pub fn auth_header(key: &SecretKey, host: &str, ts: u64) -> String {
    let id = key.public();
    let sig = key.sign(auth_message(host, &id, ts).as_bytes());
    format!("{id}.{ts}.{}", hex(&sig.to_bytes()))
}

/// 服务器验鉴权头，成功返回设备公钥
pub fn verify_auth(header: &str, host: &str, now: u64) -> Result<EndpointId> {
    let mut parts = header.trim().split('.');
    let (Some(id), Some(ts), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        bail!("鉴权头格式不对");
    };
    let id: EndpointId = id.parse().context("设备公钥格式不对")?;
    let ts: u64 = ts.parse().context("时间戳格式不对")?;
    if ts.abs_diff(now) > AUTH_MAX_SKEW {
        bail!("时间戳过期，检查两边的系统时间");
    }
    let sig = unhex(sig).ok_or_else(|| anyhow!("签名格式不对"))?;
    let sig = Signature::try_from(sig.as_slice()).map_err(|_| anyhow!("签名格式不对"))?;
    id.verify(auth_message(host, &id, ts).as_bytes(), &sig)
        .map_err(|_| anyhow!("签名不对"))?;
    Ok(id)
}

/// 用 iroh 已经在用的 ring 做 TLS，根证书用内置的 webpki 列表
fn tls_connector() -> Result<Connector> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(cfg)))
}

/// 一条连到牵线服务器的 WebSocket
pub struct WsLink {
    out: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RdvResponse>>>>,
    next: AtomicU64,
    closed: watch::Receiver<Option<String>>,
}

impl WsLink {
    pub async fn connect(url: &str, key: &SecretKey, pushes: mpsc::UnboundedSender<RdvPush>) -> Result<Arc<Self>> {
        let mut req = url.into_client_request().context("牵线服务器地址格式不对")?;
        let host = req
            .uri()
            .authority()
            .map(|a| a.as_str().to_string())
            .ok_or_else(|| anyhow!("牵线服务器地址里没有主机名"))?;
        let header = auth_header(key, &host, crate::store::now_secs());
        req.headers_mut().insert(AUTH_HEADER, header.parse()?);
        let connector = if url.starts_with("wss://") {
            tls_connector()?
        } else {
            Connector::Plain
        };
        let (ws, _) = connect_async_tls_with_config(req, None, true, Some(connector))
            .await
            .map_err(|e| anyhow!("连不上牵线服务器：{e}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out, mut rx) = mpsc::unbounded_channel::<String>();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<RdvResponse>>>> = Arc::default();
        let (closed_tx, closed) = watch::channel(None);

        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let pend = pending.clone();
        tokio::spawn(async move {
            let reason = loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<WsFrame>(&t) {
                        Ok(WsFrame { id: Some(id), resp: Some(resp), .. }) => {
                            if let Some(tx) = pend.lock().unwrap().remove(&id) {
                                let _ = tx.send(resp);
                            }
                        }
                        Ok(WsFrame { push: Some(push), .. }) => {
                            let _ = pushes.send(push);
                        }
                        Ok(_) => {}
                        Err(e) => debug!("牵线服务器发来看不懂的消息：{e}"),
                    },
                    Some(Ok(Message::Close(f))) => {
                        break f.map(|f| f.reason.to_string()).filter(|r| !r.is_empty()).unwrap_or_else(|| "服务器关闭了连接".into());
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break format!("连接出错：{e}"),
                    None => break "连接已断开".into(),
                }
            };
            pend.lock().unwrap().clear();
            let _ = closed_tx.send(Some(reason));
        });

        Ok(Arc::new(Self {
            out,
            pending,
            next: AtomicU64::new(1),
            closed,
        }))
    }

    pub async fn request(&self, req: &RdvRequest) -> Result<RdvResponse> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let frame = WsFrame {
            id: Some(id),
            req: Some(req.clone()),
            ..Default::default()
        };
        self.out
            .send(serde_json::to_string(&frame)?)
            .map_err(|_| anyhow!("和牵线服务器的连接已断开"))?;
        match tokio::time::timeout(Duration::from_secs(15), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => bail!("和牵线服务器的连接已断开"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                bail!("牵线服务器没回应")
            }
        }
    }

    /// 等连接断开，返回原因
    pub async fn closed(&self) -> String {
        let mut rx = self.closed.clone();
        match rx.wait_for(|r| r.is_some()).await {
            Ok(r) => r.clone().unwrap_or_default(),
            Err(_) => "连接已断开".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_roundtrip() {
        let key = SecretKey::generate();
        let now = 1_800_000_000;
        let h = auth_header(&key, "Rdv.Example.com", now);
        assert_eq!(verify_auth(&h, "rdv.example.com", now + 10).unwrap(), key.public());
        // 换主机名、过期、改签名都不行
        assert!(verify_auth(&h, "other.example.com", now).is_err());
        assert!(verify_auth(&h, "rdv.example.com", now + AUTH_MAX_SKEW + 1).is_err());
        let mut bad = h.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == '0' { '1' } else { '0' });
        assert!(verify_auth(&bad, "rdv.example.com", now).is_err());
        // 拿别人的公钥配自己的签名也不行
        let other = SecretKey::generate().public();
        let forged = format!("{other}.{}", h.split_once('.').unwrap().1);
        assert!(verify_auth(&forged, "rdv.example.com", now).is_err());
    }

    #[test]
    fn frame_shape() {
        let f = WsFrame {
            id: Some(3),
            req: Some(RdvRequest::Lookup { code: "123456789".into() }),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&f).unwrap(),
            r#"{"id":3,"req":{"t":"lookup","code":"123456789"}}"#
        );
        let p: WsFrame = serde_json::from_str(r#"{"push":{"t":"punch","from":"b8b57687cd1390f89e145d6c71ee5b42d101114d1f7c50e1b8ca3a192775af5c","addrs":["[2001:db8::1]:7"]}}"#).unwrap();
        assert!(matches!(p.push, Some(RdvPush::Punch { .. })));
    }
}
