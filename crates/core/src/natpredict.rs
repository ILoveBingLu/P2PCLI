//! 对称 NAT 的端口预测。
//!
//! 对称 NAT 给每个目标单独开一个外网端口：经中继测到的外网端口（iroh 告诉对方的候选地址）是发给中继服务器时
//! 用的那个，真正发包给对方时 NAT 会另开一个，对方往旧端口发的包进不来，打洞失败。
//!
//! 不少对称 NAT 是按顺序开端口的（每次 +1、+2……），这时能猜：从同一个 UDP 端口依次问几个 STUN 目标，
//! 看外网端口怎么变；按顺序的，就把接下来可能开的十几个端口也当成本机的候选地址告诉对方
//! （[`iroh::Endpoint::add_external_addr`]），对方往这些端口都发包，猜中一个就通。
//! 随机开端口的猜不了，不加（只会白发包）。
//!
//! 只管 IPv4：IPv6 大多不用转换，转换的见 [`crate::v6`]。

use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{Endpoint, Watcher};
use serde::Serialize;
use tokio::net::UdpSocket;
use tracing::{debug, info};

use crate::diag::{STUN_SERVER, binding, build_request, local_addr_for, parse_response};

/// 小米的服务器凑不齐目标时补的公共 STUN 服务器（都有 IPv4）
const MORE_SERVERS: &[&str] = &["stun.cloudflare.com:3478", "stun.nextcloud.com:3478", "stun.l.google.com:19302"];
/// 按顺序开端口时，相邻两次最多差多少还算“按顺序”
const MAX_STEP: i32 = 16;
/// 每个起点往后猜几个端口
const GUESSES: i32 = 8;
/// 多久重测一次（端口会被别的连接用掉往前走，猜的要跟上）
const REFRESH: Duration = Duration::from_secs(30);
/// 网络报告变了也至少隔这么久才重测
const MIN_GAP: Duration = Duration::from_secs(5);
const ATTR_CHANGE_REQUEST: u16 = 0x0003;
const CHANGE_IP: u32 = 0x04;
const CHANGE_PORT: u32 = 0x02;

/// NAT 怎么给新目标开外网端口
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortAllocation {
    /// 不管发给谁都是同一个外网端口（锥形）：不用猜
    Same,
    /// 按顺序开，相邻两次差 step
    Sequential { step: i32 },
    /// 没有规律：猜不了
    Random,
}

/// NAT 让哪些外面的包进来（对一个发过包的外网端口）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Filtering {
    /// 谁发来的都收
    Open,
    /// 只收发过包的那个 IP 发来的（端口不限）
    Address,
    /// 只收发过包的那个 IP 和端口发来的
    AddressPort,
}

/// 测出来的 NAT 行为
#[derive(Debug, Clone, Serialize)]
pub struct PortPattern {
    pub allocation: PortAllocation,
    /// 依次发给几个目标时外网看到的端口
    pub ports: Vec<u16>,
    /// 本机这只 UDP 的端口（外网端口和它一样说明 NAT 尽量保留端口）
    pub local_port: u16,
    pub public_ip: Ipv4Addr,
    /// 测不出时是空（服务器不支持换地址回包）
    pub filtering: Option<Filtering>,
}

impl PortPattern {
    /// 最后一次看到的外网端口：NAT 下一个开的端口在它附近
    pub fn last_port(&self) -> Option<u16> {
        self.ports.last().copied()
    }
}

/// 按依次看到的外网端口判断规律
pub fn classify(ports: &[u16]) -> PortAllocation {
    if ports.len() < 2 || ports.windows(2).all(|w| w[0] == w[1]) {
        return PortAllocation::Same;
    }
    // 只有一次变了（比如同一个 IP 不同端口时没变）也照样按差值算；全部差值同号、都不大才算按顺序
    let steps: Vec<i32> = ports.windows(2).map(|w| w[1] as i32 - w[0] as i32).filter(|d| *d != 0).collect();
    let same_sign = steps.iter().all(|d| *d > 0) || steps.iter().all(|d| *d < 0);
    if !same_sign || steps.iter().any(|d| d.abs() > MAX_STEP) {
        return PortAllocation::Random;
    }
    // 中间偶尔被别的连接插一个，取最小的那个当步长
    let step = *steps.iter().min_by_key(|d| d.abs()).unwrap();
    PortAllocation::Sequential { step }
}

/// 按顺序开端口时，猜对方该往哪些端口发：从每个起点往后猜几个（去掉已经在用的）
pub fn guesses(pattern: &PortPattern, bases: &[SocketAddrV4]) -> Vec<SocketAddr> {
    let PortAllocation::Sequential { step } = pattern.allocation else { return Vec::new() };
    let mut out = BTreeSet::new();
    for base in bases {
        for k in 1..=GUESSES {
            let port = base.port() as i32 + step * k;
            if (1024..=65535).contains(&port) {
                out.insert(SocketAddr::V4(SocketAddrV4::new(*base.ip(), port as u16)));
            }
        }
    }
    for base in bases {
        out.remove(&SocketAddr::V4(*base));
    }
    out.into_iter().collect()
}

fn change_request(change: u32) -> ([u8; 28], [u8; 12]) {
    let (head, tid) = build_request();
    let mut msg = [0u8; 28];
    msg[..20].copy_from_slice(&head);
    msg[2..4].copy_from_slice(&8u16.to_be_bytes());
    msg[20..22].copy_from_slice(&ATTR_CHANGE_REQUEST.to_be_bytes());
    msg[22..24].copy_from_slice(&4u16.to_be_bytes());
    msg[24..28].copy_from_slice(&change.to_be_bytes());
    (msg, tid)
}

/// 请服务器换个地址回包，收得到就是 NAT 放它进来了
async fn reply_comes(sock: &UdpSocket, to: SocketAddr, change: u32) -> bool {
    let (msg, tid) = change_request(change);
    let mut buf = [0u8; 1024];
    for _ in 0..2 {
        if sock.send_to(&msg, to).await.is_err() {
            return false;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_millis(700);
        while let Ok(Ok((n, _))) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
            if parse_response(&buf[..n], &tid).is_ok() {
                return true;
            }
        }
    }
    false
}

/// 测 NAT 让哪些包进来：用一只只和服务器说过话的新端口，请服务器从别的 IP、别的端口回包
async fn probe_filtering(server: SocketAddr) -> Option<Filtering> {
    let sock = UdpSocket::bind("0.0.0.0:0").await.ok()?;
    // 先正常问一次，确认服务器在、知道它有没有另一个地址（没有就测不了）
    binding(&sock, server).await.ok()?.other?;
    if reply_comes(&sock, server, CHANGE_IP | CHANGE_PORT).await {
        return Some(Filtering::Open);
    }
    if reply_comes(&sock, server, CHANGE_PORT).await {
        return Some(Filtering::Address);
    }
    Some(Filtering::AddressPort)
}

async fn resolve_v4(server: &str) -> Vec<SocketAddr> {
    match tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host(server)).await {
        Ok(Ok(addrs)) => addrs.filter(|a| a.is_ipv4()).collect(),
        _ => Vec::new(),
    }
}

/// 测本机 IPv4 NAT 开端口的规律和放包规则，大约一两秒
pub async fn probe() -> Result<PortPattern> {
    let first = *resolve_v4(STUN_SERVER).await.first().ok_or_else(|| anyhow!("解析不到 {STUN_SERVER}"))?;
    let filtering = tokio::spawn(probe_filtering(first));
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let local = local_addr_for("0.0.0.0:0", first, sock.local_addr()?.port())?;
    let r1 = binding(&sock, first).await.context("STUN 服务器没回应，可能是路由器或代理拦了 UDP")?;
    let public_ip = match r1.mapped {
        SocketAddr::V4(a) => *a.ip(),
        SocketAddr::V6(_) => bail!("STUN 回的不是 IPv4"),
    };
    // 目标：服务器的另一个端口、另一个 IP、另一个 IP 的另一个端口；凑不齐用别的服务器补
    let mut targets: Vec<SocketAddr> = Vec::new();
    if let Some(other) = r1.other.filter(|o| o.ip() != first.ip() && o.port() != first.port()) {
        targets.push(SocketAddr::new(first.ip(), other.port()));
        targets.push(SocketAddr::new(other.ip(), first.port()));
        targets.push(other);
    }
    for server in MORE_SERVERS {
        if targets.len() >= 3 {
            break;
        }
        if let Some(t) = resolve_v4(server).await.into_iter().find(|t| !targets.contains(t) && *t != first) {
            targets.push(t);
        }
    }
    let mut ports = vec![r1.mapped.port()];
    for t in targets {
        match binding(&sock, t).await {
            Ok(r) => ports.push(r.mapped.port()),
            Err(e) => debug!(%t, "问 STUN 没回应：{e:#}"),
        }
    }
    if ports.len() < 2 {
        bail!("只问通了一个 STUN 目标，看不出规律");
    }
    let filtering = filtering.await.ok().flatten();
    Ok(PortPattern { allocation: classify(&ports), ports, local_port: local.port(), public_ip, filtering })
}

/// 后台：iroh 发现本机 IPv4 是对称 NAT 时，定时测规律、把猜的端口加进候选地址；不是了就撤掉
pub fn spawn(endpoint: Endpoint) {
    if std::env::var_os("P2PCLI_NO_PORT_PREDICT").is_some() {
        return;
    }
    tokio::spawn(async move {
        use futures_util::StreamExt as _;
        let mut reports = endpoint.net_report().stream();
        let mut added: BTreeSet<SocketAddr> = BTreeSet::new();
        let mut last_probe: Option<Instant> = None;
        let mut announced = false;
        loop {
            // 网络报告变了马上看，没变就按周期重测（端口会被别的连接用掉往前走）
            let changed = tokio::select! {
                r = reports.next() => {
                    if r.is_none() {
                        break;
                    }
                    true
                }
                _ = tokio::time::sleep(REFRESH) => false,
            };
            if endpoint.is_closed() {
                break;
            }
            let report = endpoint.net_report().get();
            let iroh_addr = report.as_ref().and_then(|r| r.global_v4);
            let symmetric = report.as_ref().and_then(|r| r.mapping_varies_by_dest_ipv4) == Some(true);
            let mut want = BTreeSet::new();
            if let (true, Some(iroh_addr)) = (symmetric, iroh_addr) {
                let gap = if changed { MIN_GAP } else { REFRESH };
                if last_probe.is_some_and(|t| t.elapsed() < gap) {
                    continue;
                }
                last_probe = Some(Instant::now());
                match tokio::time::timeout(Duration::from_secs(8), probe()).await {
                    Ok(Ok(p)) if p.public_ip == *iroh_addr.ip() => {
                        let mut bases = vec![iroh_addr];
                        if let Some(port) = p.last_port() {
                            bases.push(SocketAddrV4::new(p.public_ip, port));
                        }
                        want = guesses(&p, &bases).into_iter().collect();
                        if !announced {
                            info!(allocation = ?p.allocation, ports = ?p.ports, filtering = ?p.filtering, guesses = want.len(), "本机 IPv4 是对称 NAT，测了开端口的规律");
                            announced = true;
                        }
                    }
                    // 出口不是同一个 IP（多出口、代理）：猜了也白猜
                    Ok(Ok(p)) => debug!(stun = %p.public_ip, iroh = %iroh_addr.ip(), "STUN 和 iroh 看到的出口 IP 不一样，不猜端口"),
                    Ok(Err(e)) => debug!("测开端口规律没成功：{e:#}"),
                    Err(_) => debug!("测开端口规律超时"),
                }
            }
            for addr in added.difference(&want) {
                endpoint.remove_external_addr(addr).await;
            }
            for addr in want.difference(&added) {
                endpoint.add_external_addr(*addr).await;
            }
            if added != want {
                debug!(count = want.len(), "更新了猜的外网端口");
            }
            added = want;
        }
        for addr in &added {
            endpoint.remove_external_addr(addr).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_port_patterns() {
        assert_eq!(classify(&[40000, 40000, 40000, 40000]), PortAllocation::Same);
        assert_eq!(classify(&[40000, 40001, 40002, 40003]), PortAllocation::Sequential { step: 1 });
        assert_eq!(classify(&[40000, 40002, 40004, 40006]), PortAllocation::Sequential { step: 2 });
        // 同一个 IP 不同端口没变（按 IP 开端口），换 IP 才变
        assert_eq!(classify(&[40000, 40000, 40001, 40001]), PortAllocation::Sequential { step: 1 });
        // 中间被别的连接插了一个
        assert_eq!(classify(&[40000, 40001, 40003, 40004]), PortAllocation::Sequential { step: 1 });
        assert_eq!(classify(&[50000, 49999, 49998]), PortAllocation::Sequential { step: -1 });
        assert_eq!(classify(&[40000, 12345, 61000, 3000]), PortAllocation::Random);
        assert_eq!(classify(&[40000, 40001, 39000]), PortAllocation::Random);
        assert_eq!(classify(&[40000]), PortAllocation::Same);
    }

    #[test]
    fn guesses_follow_the_step_from_each_base() {
        let ip = Ipv4Addr::new(1, 2, 3, 4);
        let p = PortPattern { allocation: PortAllocation::Sequential { step: 2 }, ports: vec![], local_port: 0, public_ip: ip, filtering: None };
        let g = guesses(&p, &[SocketAddrV4::new(ip, 40000), SocketAddrV4::new(ip, 40010)]);
        // 两个起点各往后 8 个，重叠的只算一次，起点本身不算
        assert!(g.contains(&"1.2.3.4:40002".parse().unwrap()));
        assert!(g.contains(&"1.2.3.4:40026".parse().unwrap()));
        assert!(!g.contains(&"1.2.3.4:40000".parse().unwrap()));
        assert!(!g.contains(&"1.2.3.4:40010".parse().unwrap()));
        assert_eq!(g.len(), 12);
        // 不越过 65535
        let high = guesses(&p, &[SocketAddrV4::new(ip, 65530)]);
        assert!(high.iter().all(|a| a.port() > 65530));
        assert_eq!(high.len(), 2);
        // 随机、锥形不猜
        let r = PortPattern { allocation: PortAllocation::Random, ..p.clone() };
        assert!(guesses(&r, &[SocketAddrV4::new(ip, 40000)]).is_empty());
    }

    #[test]
    fn change_request_is_well_formed() {
        let (msg, tid) = change_request(CHANGE_IP | CHANGE_PORT);
        assert_eq!(u16::from_be_bytes([msg[2], msg[3]]), 8);
        assert_eq!(&msg[8..20], &tid);
        assert_eq!(u32::from_be_bytes([msg[24], msg[25], msg[26], msg[27]]), 6);
    }
}
