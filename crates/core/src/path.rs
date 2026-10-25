//! 线路：IPv6 优先的选路规则，以及当前连接走的是哪条路。

use std::time::Duration;

use iroh::TransportAddr;
use iroh::endpoint::Connection;
use iroh::endpoint::transports::{
    AddrKind, PathSelection, PathSelectionContext, PathSelectionData, PathSelector,
};
use serde::{Deserialize, Serialize};

/// IPv6 比 IPv4 慢多少以内仍然用 IPv6
const IPV6_SLACK: Duration = Duration::from_millis(50);
/// 已经在用 IPv6 时，再多给这么多余量才退回 IPv4，免得来回跳
const IPV6_STICKY: Duration = Duration::from_millis(10);
/// 同一类线路里，新路要快这么多才换
const SWITCH_MIN: Duration = Duration::from_millis(5);

/// 选路：IPv6 直连 > IPv4 直连 > 中继。
///
/// iroh 自带的规则只给 IPv6 让 3 毫秒，又要求快 5 毫秒以上才换路，
/// 结果是哪条先通就一直用哪条，做不到 IPv6 优先。
#[derive(Debug, Default)]
pub struct Ipv6First;

fn tier(kind: AddrKind) -> usize {
    match kind {
        AddrKind::IpV6 => 0,
        AddrKind::IpV4 | AddrKind::Custom(_) => 1,
        AddrKind::Relay => 2,
    }
}

impl PathSelector for Ipv6First {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let current = ctx.current().cloned();
        let mut best: [Option<(PathSelectionData<'_>, Duration)>; 3] = [None, None, None];
        let mut current_rtt: Option<Duration> = None;
        for p in ctx.paths() {
            let Some(stats) = p.stats() else { continue };
            let rtt = stats.rtt;
            if current.as_ref() == Some(p.network_path()) {
                current_rtt = Some(current_rtt.map_or(rtt, |c| c.min(rtt)));
            }
            let t = tier(p.network_path().addr_kind());
            if best[t].as_ref().is_none_or(|(_, r)| rtt < *r) {
                best[t] = Some((p, rtt));
            }
        }
        let current_tier = current.as_ref().map(|c| tier(c.addr_kind()));
        let want = match (&best[0], &best[1]) {
            (Some((_, v6)), Some((_, v4))) => {
                let slack = if current_tier == Some(0) {
                    IPV6_SLACK + IPV6_STICKY
                } else {
                    IPV6_SLACK
                };
                if *v6 <= *v4 + slack { 0 } else { 1 }
            }
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (None, None) => 2,
        };
        let mut selection = PathSelection::none();
        let Some((cand, cand_rtt)) = best[want].take() else {
            return selection;
        };
        match (current_tier, current_rtt) {
            // 同一类里只在明显更快时换
            (Some(t), Some(rtt)) if t == want => {
                if current.as_ref() != Some(cand.network_path()) && cand_rtt + SWITCH_MIN <= rtt {
                    selection.set(&cand);
                }
            }
            _ => selection.set(&cand),
        }
        selection
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    Ipv6,
    Ipv4,
    Relay,
    Unknown,
}

impl PathKind {
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Ipv6 => "IPv6 直连",
            PathKind::Ipv4 => "IPv4 直连",
            PathKind::Relay => "中继",
            PathKind::Unknown => "连接中",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PathInfo {
    pub kind: PathKind,
    pub rtt_ms: Option<u64>,
    /// 对面的地址，给排查用
    pub remote: Option<String>,
}

pub fn current(conn: &Connection) -> PathInfo {
    let paths = conn.paths();
    let Some(p) = paths.iter().find(|p| p.is_selected()) else {
        return PathInfo {
            kind: PathKind::Unknown,
            rtt_ms: None,
            remote: None,
        };
    };
    let (kind, remote) = match p.remote_addr() {
        TransportAddr::Ip(addr) => {
            // IPv4 映射成 IPv6 的地址也算 IPv4
            let v6 = match addr.ip() {
                std::net::IpAddr::V6(ip) => ip.to_ipv4_mapped().is_none(),
                std::net::IpAddr::V4(_) => false,
            };
            (
                if v6 { PathKind::Ipv6 } else { PathKind::Ipv4 },
                Some(addr.to_string()),
            )
        }
        TransportAddr::Relay(url) => (PathKind::Relay, Some(url.to_string())),
        _ => (PathKind::Unknown, None),
    };
    PathInfo {
        kind,
        rtt_ms: Some(p.rtt().as_millis() as u64),
        remote,
    }
}
