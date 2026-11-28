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

/// 小米的 STUN 服务器，国内访问快；它支持 RFC 5780，会告诉我们另一个地址（只有 IPv4）
pub const STUN_SERVER: &str = "stun.miwifi.com:3478";
/// 支持 IPv6 的公共 STUN 服务器（并行探测，不依赖单个节点；两台回包端口一致为锥形 NAT66，不一致为对称）
pub const STUN_V6_SERVERS: &[&str] = &[
    "stun.cloudflare.com:3478",
    "stun.l.google.com:19302",
    "stun.nextcloud.com:3478",
    "stun.hot-chilli.net:3478",
];
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
    /// 网卡上的公网 IPv6
    pub ipv6_addrs: Vec<String>,
    /// 网卡上只有内网 IPv6，但路由器转换成公网出去（NAT66）时，从外面看到的出口地址
    pub ipv6_outbound: Option<String>,
    /// 网卡上的内网 IPv6（fd 开头）：只在同一个局域网里有用
    pub ipv6_lan: Vec<String>,
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

fn local_addr_for(bind_addr: &str, target: SocketAddr, sock_port: u16) -> Result<SocketAddr> {
    let probe = std::net::UdpSocket::bind(bind_addr)?;
    probe.connect(target)?;
    Ok(SocketAddr::new(probe.local_addr()?.ip(), sock_port))
}

fn classify_nat(local_ip: IpAddr, mapped: SocketAddr, mapped_second: Option<SocketAddr>) -> NatKind {
    if mapped.ip() == local_ip {
        NatKind::None
    } else {
        match mapped_second {
            Some(m) if m == mapped => NatKind::Cone,
            Some(_) => NatKind::Symmetric,
            None => NatKind::Unknown,
        }
    }
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
    let local = local_addr_for("0.0.0.0:0", first, sock.local_addr()?.port())?;

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
    let nat = classify_nat(local.ip(), r1.mapped, mapped_second);
    Ok(StunResult {
        local,
        mapped: r1.mapped,
        mapped_second,
        nat,
    })
}

/// 同一只 IPv6 UDP socket 并行向多个目标发 STUN Binding，拿到前两个不同服务器 IP 的映射结果
async fn probe_v6_targets(targets: &[SocketAddr]) -> Result<StunResult> {
    let first = *targets.first().ok_or_else(|| anyhow!("没有可用的 IPv6 STUN 服务器地址"))?;
    let sock = UdpSocket::bind("[::]:0").await?;
    let local = local_addr_for("[::]:0", first, sock.local_addr()?.port())?;

    let mut pending: Vec<(SocketAddr, [u8; 20], [u8; 12])> = targets
        .iter()
        .take(4)
        .map(|&to| {
            let (msg, tid) = build_request();
            (to, msg, tid)
        })
        .collect();
    let mut results: Vec<(SocketAddr, Response)> = Vec::new();
    let mut buf = [0u8; 1024];

    for round in 0..2 {
        for (to, msg, _) in &pending {
            let _ = sock.send_to(msg, *to).await;
        }
        let round_ms = if round == 0 { 900 } else { 1100 };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(round_ms);
        while !pending.is_empty()
            && let Ok(r) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await
        {
            let Ok((n, _)) = r else { continue };
            let mut matched = None;
            for (idx, (to, _, tid)) in pending.iter().enumerate() {
                if let Ok(resp) = parse_response(&buf[..n], tid) {
                    matched = Some((idx, *to, resp));
                    break;
                }
            }
            if let Some((idx, to, resp)) = matched {
                pending.remove(idx);
                let other = resp.other;
                if !results.iter().any(|(done, _)| *done == to) {
                    results.push((to, resp));
                }
                if results.len() >= 2 {
                    break;
                }
                if let Some(other) = other
                    && other != to
                    && !results.iter().any(|(done, _)| *done == other)
                    && !pending.iter().any(|(p, ..)| *p == other)
                {
                    let (msg, tid) = build_request();
                    let _ = sock.send_to(&msg, other).await;
                    pending.push((other, msg, tid));
                }
            }
        }
        if results.len() >= 2 || pending.is_empty() {
            break;
        }
    }

    let Some((_, r1)) = results.first() else {
        bail!("IPv6 STUN 服务器没有回应");
    };
    let mapped_second = results.get(1).map(|(_, r2)| r2.mapped);
    let nat = classify_nat(local.ip(), r1.mapped, mapped_second);
    Ok(StunResult {
        local,
        mapped: r1.mapped,
        mapped_second,
        nat,
    })
}

/// 并行解析一批 IPv6 STUN 域名并测出外网 IPv6 出口和 NAT66 类型
pub async fn stun_probe_v6(servers: &[&str]) -> Result<StunResult> {
    let lookups = futures_util::future::join_all(
        servers
            .iter()
            .map(|&s| timeout(Duration::from_secs(2), tokio::net::lookup_host(s))),
    )
    .await;
    let mut targets: Vec<SocketAddr> = Vec::new();
    for res in lookups {
        let Ok(Ok(addrs)) = res else { continue };
        for addr in addrs {
            if let SocketAddr::V6(v6) = addr
                && v6.ip().to_ipv4_mapped().is_none()
                && !targets.iter().any(|t| t.ip() == addr.ip())
            {
                targets.push(addr);
                break;
            }
        }
    }
    if targets.is_empty() {
        bail!("解析不到 IPv6 STUN 服务器地址");
    }
    probe_v6_targets(&targets).await
}

/// 完整诊断，大约几秒
pub async fn diagnose(node: &Node) -> Diagnosis {
    let interfaces = v6::interface_addrs();
    let ipv6_addrs: Vec<String> = interfaces.iter().filter(|(_, ip)| v6::is_global(ip)).map(|(_, ip)| ip.to_string()).collect();
    let ipv6_lan: Vec<String> = interfaces.iter().filter(|(_, ip)| v6::is_ula(ip)).map(|(name, ip)| format!("{ip}（{name}）")).collect();

    let need_report = node.endpoint.net_report().get().is_none()
        && !node.is_offline()
        && !node.is_relay_only()
        && !node.endpoint.is_closed();
    let want_v6_stun = ipv6_addrs.is_empty()
        && !node.is_offline()
        && !node.is_relay_only()
        && (!ipv6_lan.is_empty()
            || node.endpoint.net_report().get().is_some_and(|r| r.global_v6.is_some()));

    let stun_fut = async {
        match timeout(Duration::from_secs(8), stun_probe(STUN_SERVER)).await {
            // 只给最外层的原因，界面上好读
            Ok(r) => r.map_err(|e| e.to_string()),
            Err(_) => Err("小米 STUN 服务器 8 秒没回应".to_string()),
        }
    };
    let report_fut = async {
        if need_report {
            let mut w = node.endpoint.net_report();
            let _ = timeout(Duration::from_secs(3), w.initialized()).await;
        }
    };
    let stun_v6_fut = async {
        if want_v6_stun {
            Some(
                timeout(Duration::from_secs(4), stun_probe_v6(STUN_V6_SERVERS))
                    .await
                    .map_err(|_| anyhow!("IPv6 STUN 检测超时"))
                    .and_then(|r| r),
            )
        } else {
            None
        }
    };

    let (stun, _, stun_v6) = tokio::join!(stun_fut, report_fut, stun_v6_fut);
    let report = node.endpoint.net_report().get();
    // 网卡上没有公网 IPv6 时才看出口：有公网地址时中继看到的就是它
    let iroh_outbound = if ipv6_addrs.is_empty() {
        v6::nat66_outbound(&node.endpoint).map(|a| a.ip().to_string())
    } else {
        None
    };
    // 有出口但映射随目标变（对称）：也告诉用户，只是打不通
    let iroh_symmetric = ipv6_addrs.is_empty()
        && iroh_outbound.is_none()
        && report.as_ref().is_some_and(|r| r.global_v6.is_some() && r.mapping_varies_by_dest_ipv6 == Some(true));

    let (ipv6_outbound, ipv6_symmetric) = if !ipv6_addrs.is_empty() {
        (None, false)
    } else if let Some(Ok(s)) = &stun_v6
        && matches!(s.mapped, SocketAddr::V6(v6) if v6::is_global(v6.ip()))
        && s.nat != NatKind::None
    {
        if s.nat == NatKind::Symmetric {
            v6::invalidate_nat66_cache(node.id());
            (None, true)
        } else if s.nat == NatKind::Unknown && iroh_symmetric {
            (None, true)
        } else {
            (Some(s.mapped.ip().to_string()), false)
        }
    } else {
        (iroh_outbound, iroh_symmetric)
    };

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
    if !ipv6_addrs.is_empty() {
        notes.push(format!(
            "有公网 IPv6（{} 个地址），和同样有 IPv6 的设备可以直接连，不用打洞",
            ipv6_addrs.len()
        ));
    } else if let Some(out) = &ipv6_outbound {
        notes.push(format!(
            "电脑上是路由器发的内网 IPv6（fd 开头），路由器把它转换成公网 IPv6 出去（出口 {out}）：和有 IPv6 的设备一般能打洞直连"
        ));
    } else if ipv6_symmetric {
        notes.push("路由器把内网 IPv6 转换成公网出去，但每个目标的出口端口都不一样（对称转换），IPv6 打洞多半不通，只能看 IPv4".to_string());
    } else if !ipv6_lan.is_empty() {
        notes.push(
            "只有路由器发的内网 IPv6（fd 开头），出不了外网：同一个局域网里的设备照样直连，跨网络要靠 IPv4 打洞或中继。想要公网 IPv6，在路由器里打开 IPv6（选桥接或中继模式），或者插到直接连光猫的网口"
                .to_string(),
        );
    } else {
        notes.push("没有 IPv6，只能走 IPv4 打洞或中继".to_string());
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
        // 错误本身已经说了连不上牵线服务器，不再重复
        Some(RdvStatus::Offline { error }) => notes.push(error.clone()),
        None => notes.push("没配置牵线服务器，别人没法用设备码连你".to_string()),
    }

    let (rating, summary) = if !ipv6_addrs.is_empty() {
        ("好", "有公网 IPv6，对方也有 IPv6 时直接连；没有时看 IPv4".to_string())
    } else if ipv6_outbound.is_some() {
        ("好", "有 IPv6 出口（路由器做了地址转换），对方也有 IPv6 时一般能打洞直连；没有时看 IPv4".to_string())
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
        ipv6_outbound,
        ipv6_lan,
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

    fn build_stun_v6_response(tid: &[u8; 12], mapped: SocketAddr, other: Option<SocketAddr>) -> Vec<u8> {
        let mut body = Vec::new();
        let SocketAddr::V6(m6) = mapped else { unreachable!() };
        let port = m6.port() ^ (MAGIC_COOKIE >> 16) as u16;
        let mut key = MAGIC_COOKIE.to_be_bytes().to_vec();
        key.extend_from_slice(tid);
        let xip: Vec<u8> = m6.ip().octets().iter().zip(&key).map(|(a, k)| a ^ k).collect();
        body.extend_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
        body.extend_from_slice(&20u16.to_be_bytes());
        body.extend_from_slice(&[0, 2]);
        body.extend_from_slice(&port.to_be_bytes());
        body.extend_from_slice(&xip);
        if let Some(SocketAddr::V6(o6)) = other {
            body.extend_from_slice(&ATTR_OTHER.to_be_bytes());
            body.extend_from_slice(&20u16.to_be_bytes());
            body.extend_from_slice(&[0, 2]);
            body.extend_from_slice(&o6.port().to_be_bytes());
            body.extend_from_slice(&o6.ip().octets());
        }
        let mut msg = Vec::new();
        msg.extend_from_slice(&0x0101u16.to_be_bytes());
        msg.extend_from_slice(&(body.len() as u16).to_be_bytes());
        msg.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        msg.extend_from_slice(tid);
        msg.extend_from_slice(&body);
        msg
    }

    #[test]
    fn parses_ipv6_xor_and_other_address() {
        let (_, tid) = build_request();
        let mapped: SocketAddr = "[240e:3b4:1::5]:45678".parse().unwrap();
        let other: SocketAddr = "[2001:db8::2]:3479".parse().unwrap();
        let msg = build_stun_v6_response(&tid, mapped, Some(other));
        let r = parse_response(&msg, &tid).unwrap();
        assert_eq!(r.mapped, mapped);
        assert_eq!(r.other, Some(other));
    }

    #[tokio::test]
    async fn probe_v6_targets_detects_cone_and_symmetric() {
        let Ok(s1) = UdpSocket::bind("[::1]:0").await else {
            eprintln!("本机不支持绑定 IPv6 回环，跳过");
            return;
        };
        let Ok(s2) = UdpSocket::bind("[::1]:0").await else {
            eprintln!("本机不支持绑定 IPv6 回环，跳过");
            return;
        };
        let a1 = s1.local_addr().unwrap();
        let a2 = s2.local_addr().unwrap();

        let spawn_mock = |sock: UdpSocket, mapped: SocketAddr, other: Option<SocketAddr>| {
            tokio::spawn(async move {
                let mut buf = [0u8; 512];
                for _ in 0..4 {
                    let Ok(Ok((n, peer))) = timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await else {
                        break;
                    };
                    if n >= 20 {
                        let tid: [u8; 12] = buf[8..20].try_into().unwrap();
                        let resp = build_stun_v6_response(&tid, mapped, other);
                        let _ = sock.send_to(&resp, peer).await;
                    }
                }
            })
        };

        // 1. 两个服务器返回相同的映射地址 -> 锥形 NAT66
        let cone_addr: SocketAddr = "[240e:1::9]:40000".parse().unwrap();
        let t1 = spawn_mock(s1, cone_addr, None);
        let t2 = spawn_mock(s2, cone_addr, None);
        let r_cone = probe_v6_targets(&[a1, a2]).await.unwrap();
        assert_eq!(r_cone.mapped, cone_addr);
        assert_eq!(r_cone.nat, NatKind::Cone);
        t1.abort();
        t2.abort();

        // 2. 两个服务器返回不同端口 -> 对称 NAT66
        let s1_b = UdpSocket::bind("[::1]:0").await.unwrap();
        let s2_b = UdpSocket::bind("[::1]:0").await.unwrap();
        let b1 = s1_b.local_addr().unwrap();
        let b2 = s2_b.local_addr().unwrap();
        let t1 = spawn_mock(s1_b, "[240e:1::9]:40001".parse().unwrap(), None);
        let t2 = spawn_mock(s2_b, "[240e:1::9]:40002".parse().unwrap(), None);
        let r_sym = probe_v6_targets(&[b1, b2]).await.unwrap();
        assert_eq!(r_sym.nat, NatKind::Symmetric);
        t1.abort();
        t2.abort();

        // 3. 只连一个服务器但回包带 OTHER-ADDRESS -> 继续探测第二个地址判定锥形 NAT66
        let s1_c = UdpSocket::bind("[::1]:0").await.unwrap();
        let s2_c = UdpSocket::bind("[::1]:0").await.unwrap();
        let c1 = s1_c.local_addr().unwrap();
        let c2 = s2_c.local_addr().unwrap();
        let t1 = spawn_mock(s1_c, cone_addr, Some(c2));
        let t2 = spawn_mock(s2_c, cone_addr, None);
        let r_other = probe_v6_targets(&[c1]).await.unwrap();
        assert_eq!(r_other.nat, NatKind::Cone);
        assert_eq!(r_other.mapped_second, Some(cone_addr));
        t1.abort();
        t2.abort();
    }
}
