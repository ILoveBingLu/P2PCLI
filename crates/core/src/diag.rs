//! 网络诊断：本机有没有公网 IPv6、IPv4 在什么样的 NAT 后面、中继通不通。
//!
//! NAT 类型用小米的 STUN 服务器测：同一个端口先后问两个不同的地址，
//! 外网看到的端口一样就是锥形 NAT（IPv4 打洞一般能成），不一样就是对称 NAT（基本打不通）。
//! 有的路由器或代理会拦 STUN，这时退回 iroh 自己经中继测到的结果。

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use iroh::Watcher;
use serde::Serialize;
use tokio::net::UdpSocket;
use tokio::time::timeout;

use crate::node::Node;
use crate::rendezvous::RdvStatus;
use crate::v6;

/// 小米的 STUN 服务器，国内访问快；它支持 RFC 5780，会告诉我们另一个地址
pub const STUN_SERVER: &str = "stun.miwifi.com:3478";
const MAGIC_COOKIE: u32 = 0x2112_A442;
const ATTR_MAPPED: u16 = 0x0001;
const ATTR_CHANGED: u16 = 0x0005;
const ATTR_XOR_MAPPED: u16 = 0x0020;
const ATTR_XOR_MAPPED_OLD: u16 = 0x8020;
const ATTR_OTHER: u16 = 0x802c;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NatKind {
    /// 本机就是公网 IPv4
    None,
    /// 外网端口不随目标变化，IPv4 打洞一般能成
    Cone,
    /// 每个目标外网端口都不同，IPv4 打洞基本不通
    Symmetric,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct StunResult {
    pub local: SocketAddr,
    pub mapped: SocketAddr,
    /// 第二个目标看到的外网地址；服务器没给第二个地址时为空
    pub mapped_second: Option<SocketAddr>,
    pub nat: NatKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnosis {
    /// 好 / 一般 / 差 / 未知
    pub rating: &'static str,
    pub summary: String,
    pub ipv6_addrs: Vec<String>,
    pub ipv4_public: Option<String>,
    pub nat: NatKind,
    /// NAT 类型的来源：stun（小米）/ relay（iroh 经中继测）/ none
    pub nat_source: &'static str,
    pub stun_error: Option<String>,
    pub relay: Option<String>,
    pub relay_connected: bool,
    pub relay_latency_ms: Option<u64>,
    pub server: Option<RdvStatus>,
    /// 逐条结论，界面直接显示
    pub notes: Vec<String>,
}

fn build_request() -> ([u8; 20], [u8; 12]) {
    let mut tid = [0u8; 12];
    for b in tid.iter_mut() {
        *b = rand::random();
    }
    let mut msg = [0u8; 20];
    msg[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    msg[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    msg[8..20].copy_from_slice(&tid);
    (msg, tid)
}

struct Response {
    mapped: SocketAddr,
    other: Option<SocketAddr>,
}

fn parse_addr(v: &[u8], xor: bool, tid: &[u8; 12]) -> Option<SocketAddr> {
    if v.len() < 8 {
        return None;
    }
    let mut port = u16::from_be_bytes([v[2], v[3]]);
    if xor {
        port ^= (MAGIC_COOKIE >> 16) as u16;
    }
    let mut key = MAGIC_COOKIE.to_be_bytes().to_vec();
    key.extend_from_slice(tid);
    let ip = match v[1] {
        1 => {
            let mut b: [u8; 4] = v[4..8].try_into().ok()?;
            if xor {
                b.iter_mut().zip(&key).for_each(|(x, k)| *x ^= k);
            }
            IpAddr::from(b)
        }
        2 if v.len() >= 20 => {
            let mut b: [u8; 16] = v[4..20].try_into().ok()?;
            if xor {
                b.iter_mut().zip(&key).for_each(|(x, k)| *x ^= k);
            }
            IpAddr::from(b)
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

fn parse_response(data: &[u8], tid: &[u8; 12]) -> Result<Response> {
    if data.len() < 20 || u16::from_be_bytes([data[0], data[1]]) != 0x0101 || data[8..20] != tid[..] {
        bail!("不是这次请求的回包");
    }
    let len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let body = data.get(20..20 + len).ok_or_else(|| anyhow!("回包长度不对"))?;
    let (mut xor_mapped, mut mapped, mut other) = (None, None, None);
    let mut i = 0;
    while i + 4 <= body.len() {
        let t = u16::from_be_bytes([body[i], body[i + 1]]);
        let l = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
        let Some(v) = body.get(i + 4..i + 4 + l) else { break };
        match t {
            ATTR_XOR_MAPPED | ATTR_XOR_MAPPED_OLD => xor_mapped = parse_addr(v, true, tid),
            ATTR_MAPPED => mapped = parse_addr(v, false, tid),
            ATTR_OTHER | ATTR_CHANGED => other = other.or(parse_addr(v, false, tid)),
            _ => {}
        }
        i += 4 + l.div_ceil(4) * 4;
    }
    Ok(Response {
        mapped: xor_mapped.or(mapped).ok_or_else(|| anyhow!("回包里没有外网地址"))?,
        other,
    })
}

/// 发一次 Binding 请求，丢包重试
async fn binding(sock: &UdpSocket, to: SocketAddr) -> Result<Response> {
    let (msg, tid) = build_request();
    let mut buf = [0u8; 1024];
    for _ in 0..3 {
        sock.send_to(&msg, to).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
        while let Ok(r) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
            let (n, _) = r?;
            if let Ok(resp) = parse_response(&buf[..n], &tid) {
                return Ok(resp);
            }
        }
    }
    bail!("{to} 没有回应")
}

/// 用 STUN 测本机 IPv4 的外网地址和 NAT 类型
pub async fn stun_probe(server: &str) -> Result<StunResult> {
    let targets: Vec<SocketAddr> = tokio::net::lookup_host(server)
        .await
        .with_context(|| format!("解析 {server} 失败"))?
        .filter(|a| a.is_ipv4())
        .collect();
    let first = *targets.first().ok_or_else(|| anyhow!("{server} 没有 IPv4 地址"))?;
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    // 找出本机出口 IP：连一下目标看系统选了哪块网卡，不发包
    let probe = std::net::UdpSocket::bind("0.0.0.0:0")?;
    probe.connect(first)?;
    let local = SocketAddr::new(probe.local_addr()?.ip(), sock.local_addr()?.port());

    let r1 = binding(&sock, first)
        .await
        .context("小米 STUN 服务器连不上，可能是路由器或代理拦了 UDP 3478")?;
    // 第二个目标：优先用服务器告诉的另一个地址，没有就用解析出来的另一个 IP
    let second = r1
        .other
        .filter(|o| o.ip() != first.ip())
        .or_else(|| targets.iter().copied().find(|t| t.ip() != first.ip()));
    let mapped_second = match second {
        Some(s) => binding(&sock, s).await.ok().map(|r| r.mapped),
        None => None,
    };
    let nat = if r1.mapped.ip() == local.ip() {
        NatKind::None
    } else {
        match mapped_second {
            Some(m) if m == r1.mapped => NatKind::Cone,
            Some(_) => NatKind::Symmetric,
            None => NatKind::Unknown,
        }
    };
    Ok(StunResult {
        local,
        mapped: r1.mapped,
        mapped_second,
        nat,
    })
}

/// 完整诊断，大约几秒
pub async fn diagnose(node: &Node) -> Diagnosis {
    let stun = match timeout(Duration::from_secs(8), stun_probe(STUN_SERVER)).await {
        // 只给最外层的原因，界面上好读
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err("小米 STUN 服务器 8 秒没回应".to_string()),
    };
    let report = node.endpoint.net_report().get();
    let ipv6_addrs: Vec<String> = v6::local_candidates(&node.endpoint)
        .iter()
        .map(|a| a.ip().to_string())
        .collect();

    let (nat, nat_source, ipv4_public) = match &stun {
        Ok(s) => (s.nat, "stun", Some(s.mapped.ip().to_string())),
        Err(_) => {
            let r = report.as_ref();
            let nat = match r.and_then(|r| r.mapping_varies_by_dest_ipv4) {
                Some(true) => NatKind::Symmetric,
                Some(false) => NatKind::Cone,
                None => NatKind::Unknown,
            };
            let public = r.and_then(|r| r.global_v4).map(|a| a.ip().to_string());
            (nat, if nat == NatKind::Unknown { "none" } else { "relay" }, public)
        }
    };

    let home = node.endpoint.home_relay_status().get();
    let relay_status = home.first();
    let relay = relay_status.map(|s| s.url().to_string());
    let relay_connected = relay_status.is_some_and(|s| s.is_connected());
    let relay_latency_ms = report.as_ref().and_then(|r| {
        let url = relay_status.map(|s| s.url().clone())?;
        r.relay_latency
            .iter()
            .filter(|(_, u, _)| **u == url)
            .map(|(_, _, d)| d.as_millis() as u64)
            .min()
    });

    let mut notes = Vec::new();
    if ipv6_addrs.is_empty() {
        notes.push("没有公网 IPv6，只能走 IPv4 打洞或中继".to_string());
    } else {
        notes.push(format!(
            "有公网 IPv6（{} 个地址），和同样有 IPv6 的设备可以直接连，不用打洞",
            ipv6_addrs.len()
        ));
    }
    let via = if nat_source == "relay" { "（经中继测得）" } else { "" };
    notes.push(match nat {
        NatKind::None => "本机直接是公网 IPv4，别人能直接连进来（系统防火墙放行的话）".to_string(),
        NatKind::Cone => format!("IPv4 是锥形 NAT{via}：外网端口固定，IPv4 打洞一般能成功"),
        NatKind::Symmetric => format!(
            "IPv4 是对称 NAT{via}：每个目标外网端口都不同；对方是锥形 NAT 时多半还能打通，对方也是对称 NAT 就只能走中继"
        ),
        NatKind::Unknown => "测不出 IPv4 的 NAT 类型".to_string(),
    });
    if let Err(e) = &stun {
        notes.push(format!("STUN 检测没成功：{e}"));
    }
    notes.push(match (&relay, relay_connected) {
        (Some(url), true) => match relay_latency_ms {
            Some(ms) => format!("中继已连上：{url}，延迟 {ms} 毫秒"),
            None => format!("中继已连上：{url}"),
        },
        (Some(url), false) => format!("中继没连上：{url}，打不通时会连不上"),
        (None, _) => "还没选定中继，打不通时会连不上".to_string(),
    });
    let server = node.rdv.as_ref().map(|r| r.status());
    match &server {
        Some(RdvStatus::Online) => notes.push("牵线服务器在线".to_string()),
        Some(RdvStatus::Connecting) => notes.push("正在连牵线服务器".to_string()),
        Some(RdvStatus::Offline { error }) => notes.push(format!("牵线服务器连不上：{error}")),
        None => notes.push("没配置牵线服务器，别人没法用设备码连你".to_string()),
    }

    let (rating, summary) = if !ipv6_addrs.is_empty() {
        ("好", "有公网 IPv6，对方也有 IPv6 时直接连；没有时看 IPv4".to_string())
    } else {
        match nat {
            NatKind::None | NatKind::Cone => ("一般", "没有 IPv6，但 IPv4 打洞一般能成功".to_string()),
            NatKind::Symmetric => (
                "差",
                "没有 IPv6，IPv4 是对称 NAT：对方网络好时还能直连，否则走中继，延迟会高一些".to_string(),
            ),
            NatKind::Unknown => ("未知", "测不出能不能直连，连接时会自动尝试，不通就走中继".to_string()),
        }
    };
    Diagnosis {
        rating,
        summary,
        ipv6_addrs,
        ipv4_public,
        nat,
        nat_source,
        stun_error: stun.err(),
        relay,
        relay_connected,
        relay_latency_ms,
        server,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用真实抓到的小米回包结构造一个，验证解析
    #[test]
    fn parses_xor_and_other_address() {
        let (_, tid) = build_request();
        let mut body = Vec::new();
        // XOR-MAPPED-ADDRESS 112.49.245.73:18762
        let port = 18762u16 ^ (MAGIC_COOKIE >> 16) as u16;
        let ip = [112u8, 49, 245, 73];
        let key = MAGIC_COOKIE.to_be_bytes();
        let xip: Vec<u8> = ip.iter().zip(key).map(|(a, k)| a ^ k).collect();
        body.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
        body.extend_from_slice(&8u16.to_be_bytes());
        body.extend_from_slice(&[0, 1]);
        body.extend_from_slice(&port.to_be_bytes());
        body.extend_from_slice(&xip);
        // OTHER-ADDRESS 111.206.174.3:3479
        body.extend_from_slice(&ATTR_OTHER.to_be_bytes());
        body.extend_from_slice(&8u16.to_be_bytes());
        body.extend_from_slice(&[0, 1]);
        body.extend_from_slice(&3479u16.to_be_bytes());
        body.extend_from_slice(&[111, 206, 174, 3]);
        let mut msg = Vec::new();
        msg.extend_from_slice(&0x0101u16.to_be_bytes());
        msg.extend_from_slice(&(body.len() as u16).to_be_bytes());
        msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(&tid);
        msg.extend_from_slice(&body);
        let r = parse_response(&msg, &tid).unwrap();
        assert_eq!(r.mapped, "112.49.245.73:18762".parse().unwrap());
        assert_eq!(r.other, Some("111.206.174.3:3479".parse().unwrap()));
        // 别的请求的回包不认
        let (_, other_tid) = build_request();
        assert!(parse_response(&msg, &other_tid).is_err());
    }
}
