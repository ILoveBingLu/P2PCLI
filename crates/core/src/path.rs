//! 线路：按延迟和丢包打分的选路规则，以及当前连接走的是哪条路。

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use iroh::{Endpoint, TransportAddr, Watcher};
use iroh::endpoint::{Connection, LocalTransportAddr};
use iroh::endpoint::transports::{
    AddrKind, FourTuple, PathSelection, PathSelectionContext, PathSelectionData, PathSelector,
};
use serde::{Deserialize, Serialize};

/// 两次选路之间发出的包少于这么多，丢包率不算数，只看延迟
const MIN_SAMPLE: u64 = 20;
/// 丢包率封顶，免得分数除到无穷大
const MAX_LOSS: f64 = 0.8;
/// 当前路的分数比最佳路高出这么多才换，免得来回跳；中继换直连不受这个限制
const SWITCH_MIN: Duration = Duration::from_millis(10);
/// 最多记这么多条路的计数，超了丢最久没见的
const MAX_TRACKED: usize = 256;

/// 选路：每条路算分数 = 延迟 / (1 - 最近丢包率)，选分数最低的。
///
/// IPv6、IPv4、自定义传输都算直连，用同一个分数比；有直连时中继不参与比较，
/// 没有直连才在中继里按同一分数挑。当前路比最佳路高出 10 毫秒以上才换，
/// 从中继换到直连则立刻换。
///
/// 最近丢包率取两次 select 之间的差值。整个 endpoint 共用这一个选择器，
/// 所以按路记下上一次的发包数和丢包数。iroh 只在新连接、新路接通、旧路断开时调 select。
///
/// 名字是早先 IPv6 优先时留下的，现在已经不偏向 IPv6。
#[derive(Debug, Default)]
pub struct Ipv6First {
    last: Mutex<Vec<Seen>>,
}

/// node.rs 里按单元结构体写的 `Ipv6First`，靠这个同名常量照样能用；每用一次都是一份空记录
#[allow(non_upper_case_globals, clippy::declare_interior_mutable_const)]
pub const Ipv6First: Ipv6First = Ipv6First { last: Mutex::new(Vec::new()) };

/// 一条路上次选路时的累计计数
#[derive(Debug)]
struct Seen {
    path: FourTuple,
    counts: Counts,
    at: Instant,
}

#[derive(Debug, Clone, Copy)]
struct Counts {
    sent: u64,
    lost: u64,
}

/// 参与比较的一条路
#[derive(Debug, Clone, Copy)]
struct Candidate {
    direct: bool,
    score: Duration,
}

/// 最近丢包率：这一小段丢掉的包 / 这一小段发出的包，最高按 0.8。
/// 没有上一次记录、计数倒退（连接换过）或这一小段发出的包不到 20 个，都按 0。
fn loss_rate(prev: Option<Counts>, now: Counts) -> f64 {
    let Some(prev) = prev else { return 0.0 };
    let Some(sent) = now.sent.checked_sub(prev.sent) else { return 0.0 };
    if sent < MIN_SAMPLE {
        return 0.0;
    }
    let lost = now.lost.saturating_sub(prev.lost);
    (lost as f64 / sent as f64).min(MAX_LOSS)
}

/// 分数 = 延迟 / (1 - 丢包率)，越低越好
fn score(rtt: Duration, loss: f64) -> Duration {
    rtt.div_f64(1.0 - loss.clamp(0.0, MAX_LOSS))
}

/// 挑要换去的路，返回下标；`None` 表示保持现状。`current` 是当前路在 `cands` 里的位置。
fn choose(cands: &[Candidate], current: Option<usize>) -> Option<usize> {
    let has_direct = cands.iter().any(|c| c.direct);
    let (best, best_cand) = cands
        .iter()
        .enumerate()
        .filter(|(_, c)| c.direct == has_direct)
        .min_by_key(|(_, c)| c.score)?;
    let Some(cur) = current else { return Some(best) };
    if cur == best {
        return None;
    }
    let cur_cand = cands[cur];
    // 从中继换到直连：不等
    if !cur_cand.direct && best_cand.direct {
        return Some(best);
    }
    (cur_cand.score >= best_cand.score + SWITCH_MIN).then_some(best)
}

impl PathSelector for Ipv6First {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        // 同一条路可能出现在几个连接上：延迟取最小，计数加起来
        let mut paths: Vec<(PathSelectionData<'_>, Duration, Counts)> = Vec::new();
        for p in ctx.paths() {
            let Some(stats) = p.stats() else { continue };
            match paths.iter_mut().find(|(q, ..)| q.network_path() == p.network_path()) {
                Some((_, rtt, counts)) => {
                    *rtt = (*rtt).min(stats.rtt);
                    counts.sent += stats.sent_packets;
                    counts.lost += stats.lost_packets;
                }
                None => {
                    let counts = Counts { sent: stats.sent_packets, lost: stats.lost_packets };
                    paths.push((p, stats.rtt, counts));
                }
            }
        }

        let now = Instant::now();
        let cands: Vec<Candidate> = {
            let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
            let cands = paths
                .iter()
                .map(|(p, rtt, counts)| {
                    let path = p.network_path();
                    let prev = match last.iter_mut().find(|s| &s.path == path) {
                        Some(seen) => {
                            let prev = seen.counts;
                            seen.counts = *counts;
                            seen.at = now;
                            Some(prev)
                        }
                        None => {
                            last.push(Seen { path: path.clone(), counts: *counts, at: now });
                            None
                        }
                    };
                    Candidate {
                        direct: !matches!(path.addr_kind(), AddrKind::Relay),
                        score: score(*rtt, loss_rate(prev, *counts)),
                    }
                })
                .collect();
            while last.len() > MAX_TRACKED {
                let oldest = last.iter().enumerate().min_by_key(|(_, s)| s.at).map(|(i, _)| i);
                let Some(i) = oldest else { break };
                last.swap_remove(i);
            }
            cands
        };

        let current = ctx.current().and_then(|c| paths.iter().position(|(p, ..)| p.network_path() == c));
        let mut selection = PathSelection::none();
        if let Some(i) = choose(&cands, current) {
            selection.set(&paths[i].0);
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

/// 连接信息卡片用的统计。字节数、包数都是从连上开始累计的，界面隔一秒取一次，自己算速率和丢包率
#[derive(Debug, Clone, Serialize)]
pub struct ConnStats {
    pub path: PathInfo,
    /// 本机这头的地址：IPv6 直连一般就是公网地址，IPv4 多半是内网地址；走中继时没有
    pub local: Option<String>,
    /// 本机对外的 IPv4 公网地址，打洞时探测到的；IPv4 走 NAT 时看这个
    pub public_v4: Option<String>,
    pub sent_bytes: u64,
    pub recv_bytes: u64,
    pub sent_packets: u64,
    pub lost_packets: u64,
}

pub fn stats(conn: &Connection, endpoint: &Endpoint) -> ConnStats {
    let local = conn.paths().iter().find(|p| p.is_selected()).and_then(|p| match p.local_addr() {
        LocalTransportAddr::Ip(ip) => ip.map(|ip| ip.to_string()),
        _ => None,
    });
    let public_v4 = endpoint.net_report().get().and_then(|r| r.global_v4).map(|a| a.ip().to_string());
    let s = conn.stats();
    ConnStats {
        path: current(conn),
        local,
        public_v4,
        sent_bytes: s.udp_tx.bytes,
        recv_bytes: s.udp_rx.bytes,
        sent_packets: s.sent_packets,
        lost_packets: s.lost_packets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// 分数四舍五入到毫秒，避开浮点误差
    fn score_ms(rtt: u64, loss: f64) -> u64 {
        (score(ms(rtt), loss).as_secs_f64() * 1000.0).round() as u64
    }

    fn direct(rtt: u64, loss: f64) -> Candidate {
        Candidate { direct: true, score: score(ms(rtt), loss) }
    }

    fn relay(rtt: u64, loss: f64) -> Candidate {
        Candidate { direct: false, score: score(ms(rtt), loss) }
    }

    fn counts(sent: u64, lost: u64) -> Counts {
        Counts { sent, lost }
    }

    #[test]
    fn score_examples() {
        assert_eq!(score_ms(20, 0.0), 20);
        assert_eq!(score_ms(20, 0.5), 40);
        assert_eq!(score_ms(20, 0.8), 100);
        assert_eq!(score_ms(60, 0.0), 60);
        // 前三种里选不丢包的 20 毫秒
        assert_eq!(choose(&[direct(20, 0.5), direct(20, 0.8), direct(20, 0.0)], None), Some(2));
        // 60 毫秒不丢包胜过 20 毫秒丢 0.8，正在用丢包那条也要换
        let cands = [direct(20, 0.8), direct(60, 0.0)];
        assert_eq!(choose(&cands, None), Some(1));
        assert_eq!(choose(&cands, Some(0)), Some(1));
    }

    #[test]
    fn loss_rate_is_capped() {
        assert_eq!(loss_rate(Some(counts(100, 10)), counts(200, 60)), 0.5);
        assert_eq!(loss_rate(Some(counts(0, 0)), counts(100, 95)), MAX_LOSS);
        // 这一段丢的比发的还多（前面发的包这时才判丢）也按封顶
        assert_eq!(loss_rate(Some(counts(0, 0)), counts(20, 50)), MAX_LOSS);
        assert_eq!(score_ms(20, 1.0), 100);
    }

    #[test]
    fn loss_rate_needs_enough_samples() {
        // 没有上一次记录
        assert_eq!(loss_rate(None, counts(1000, 500)), 0.0);
        // 这一段只发了 19 个
        assert_eq!(loss_rate(Some(counts(100, 0)), counts(119, 19)), 0.0);
        // 刚好 20 个就算
        assert_eq!(loss_rate(Some(counts(100, 0)), counts(120, 10)), 0.5);
        // 计数倒退说明连接换过，当没有记录
        assert_eq!(loss_rate(Some(counts(500, 100)), counts(30, 30)), 0.0);
    }

    #[test]
    fn direct_ignores_relay() {
        // 中继再快，有直连就不看中继
        assert_eq!(choose(&[relay(5, 0.0), direct(100, 0.0)], None), Some(1));
        // 正在走中继，直连一出现立刻换，不受 10 毫秒限制
        assert_eq!(choose(&[relay(5, 0.0), direct(100, 0.0)], Some(0)), Some(1));
        // 没有直连才在中继里挑
        assert_eq!(choose(&[relay(50, 0.0), relay(30, 0.0)], None), Some(1));
        assert_eq!(choose(&[], None), None);
    }

    #[test]
    fn switch_needs_10ms_gap() {
        let cands = [direct(29, 0.0), direct(20, 0.0)];
        assert_eq!(choose(&cands, Some(1)), None);
        assert_eq!(choose(&cands, Some(0)), None);
        assert_eq!(choose(&[direct(30, 0.0), direct(20, 0.0)], Some(0)), Some(1));
        // 中继之间也一样
        assert_eq!(choose(&[relay(29, 0.0), relay(20, 0.0)], Some(0)), None);
        assert_eq!(choose(&[relay(30, 0.0), relay(20, 0.0)], Some(0)), Some(1));
    }
}
