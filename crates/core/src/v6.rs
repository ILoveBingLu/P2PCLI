//! IPv6 直连：两边都有公网 IPv6 时，经牵线服务器约好同时互相发包，第一个包就走直连，不经过中继。
//!
//! 公网 IPv6 没有 NAT，网卡上的地址就是对外地址，不用问外面“我是谁”；
//! 家用路由器对 IPv6 只有状态防火墙，双方各自先往对方发一个包，回包就放行了。
//! 所以这里不打洞，只是让被连方在连接方敲门的同时也往外发包。
//!
//! 有的路由器只给电脑发 fd 开头的内网 IPv6，出去时再转换成公网 IPv6（NAT66）。网卡上看不到公网地址，
//! 但 iroh 连中继时能看到本机从外面看是哪个 IPv6 地址、出口端口会不会随目标变。不随目标变时，
//! 这个出口地址和公网 IPv6 一样能用：双方同时往对方的出口发包，路由器的映射就打开了。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use iroh::{Endpoint, EndpointId, Watcher};

/// 一次最多报几个地址（隐私扩展会给一块网卡好几个临时地址）
pub const MAX_CANDIDATES: usize = 6;
/// iroh 1.4.1 的周期性 net_report（每 20~26 秒一次）有时因 QADv6 超时或被提前取消而丢掉 global_v6；
/// 在本地网卡地址和监听端口未变时保留最近一次有效出口，避免诊断与直连来回抖动
const NAT66_CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const NAT66_CACHE_LIMIT: usize = 64;

#[derive(Clone, PartialEq, Eq)]
struct Nat66Context {
    v6_port: Option<u16>,
    addrs: Vec<Ipv6Addr>,
}

struct CachedNat66 {
    ctx: Nat66Context,
    addr: SocketAddr,
    seen_at: Instant,
}

fn nat66_cache() -> &'static Mutex<HashMap<EndpointId, CachedNat66>> {
    static CACHE: OnceLock<Mutex<HashMap<EndpointId, CachedNat66>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 2000::/3，全球单播地址
pub fn is_global(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xe000) == 0x2000
}

/// fc00::/7，局域网内用的 IPv6
pub fn is_ula(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// 测试用：局域网的 fd 开头地址也当可用地址，手头没有公网 IPv6 时测整条流程
fn allow_ula() -> bool {
    std::env::var_os("P2PCLI_IPV6_ULA").is_some()
}

pub fn usable(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().is_none() && (is_global(&ip) || (allow_ula() && is_ula(&ip))),
        IpAddr::V4(_) => false,
    }
}

/// 从一组地址里挑出能用来 IPv6 直连的
pub fn pick<'a>(addrs: impl IntoIterator<Item = &'a SocketAddr>) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = addrs.into_iter().filter(|a| usable(a)).copied().collect();
    v.sort();
    v.dedup();
    v.truncate(MAX_CANDIDATES);
    v
}

/// 挑出网卡上可用作直连或诊断的 IPv6 地址（跳过未就绪、冲突、已过期的地址），
/// 返回 `(地址, 是否临时隐私地址)`
fn usable_interface_v6(iface: &netdev::Interface) -> Vec<(Ipv6Addr, bool)> {
    if !iface.is_up() || iface.is_loopback() || iface.is_tun() {
        return Vec::new();
    }
    iface
        .ipv6
        .iter()
        .enumerate()
        .filter_map(|(idx, net)| {
            let flags = iface.ipv6_addr_flags.get(idx).copied().unwrap_or_default();
            if flags.tentative || flags.duplicated || flags.deprecated {
                return None;
            }
            let ip = net.addr();
            (is_global(&ip) || is_ula(&ip)).then_some((ip, flags.temporary))
        })
        .collect()
}

/// 网卡上的 IPv6 地址（不算本地链接 fe80、不算隧道网卡、不算过期/未就绪地址）：(网卡名, 地址)，诊断时逐个列出来
pub fn interface_addrs() -> Vec<(String, Ipv6Addr)> {
    netdev::get_interfaces()
        .iter()
        .flat_map(|i| {
            let name = i.friendly_name.clone().unwrap_or_else(|| i.name.clone());
            usable_interface_v6(i)
                .into_iter()
                .map(move |(ip, _)| (name.clone(), ip))
        })
        .collect()
}

fn bound_v6_port(endpoint: &Endpoint) -> Option<u16> {
    endpoint
        .bound_sockets()
        .iter()
        .find(|s| s.is_ipv6())
        .map(|s| s.port())
}

fn current_nat66_context(endpoint: &Endpoint) -> Nat66Context {
    let mut addrs: Vec<Ipv6Addr> = interface_addrs().into_iter().map(|(_, ip)| ip).collect();
    addrs.sort();
    addrs.dedup();
    Nat66Context {
        v6_port: bound_v6_port(endpoint),
        addrs,
    }
}

fn evaluate_nat66_state(
    id: EndpointId,
    ctx: Nat66Context,
    global_v6: Option<SocketAddr>,
    varies_by_dest: Option<bool>,
) -> Option<SocketAddr> {
    let mut cache = nat66_cache().lock().unwrap_or_else(|e| e.into_inner());
    if varies_by_dest == Some(true) {
        cache.remove(&id);
        return None;
    }
    if let Some(addr) = global_v6.filter(usable) {
        if ctx.v6_port.is_some() && !ctx.addrs.is_empty() {
            if cache.len() >= NAT66_CACHE_LIMIT && !cache.contains_key(&id) {
                let now = Instant::now();
                cache.retain(|_, v| now.duration_since(v.seen_at) < NAT66_CACHE_TTL);
                if cache.len() >= NAT66_CACHE_LIMIT
                    && let Some(oldest) = cache.iter().min_by_key(|(_, v)| v.seen_at).map(|(k, _)| *k)
                {
                    cache.remove(&oldest);
                }
            }
            cache.insert(
                id,
                CachedNat66 {
                    ctx,
                    addr,
                    seen_at: Instant::now(),
                },
            );
        }
        return Some(addr);
    }
    if let Some(entry) = cache.get(&id) {
        if entry.ctx == ctx
            && ctx.v6_port.is_some()
            && !ctx.addrs.is_empty()
            && entry.seen_at.elapsed() < NAT66_CACHE_TTL
        {
            return Some(entry.addr);
        }
        cache.remove(&id);
    }
    None
}

/// 清除该节点缓存的 NAT66 出口（节点关闭或诊断测出对称 NAT66 时调用）
pub fn invalidate_nat66_cache(id: EndpointId) {
    nat66_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
}

/// iroh 经中继看到的本机 IPv6 出口：网卡上只有内网 IPv6、路由器做了地址转换（NAT66）时，
/// 这就是对外的地址。出口端口随目标变（对称映射）时打不通，不算；
/// 若本机网卡与监听端口未变，临时丢失 `global_v6` 时沿用最近一次有效结果。
pub fn nat66_outbound(endpoint: &Endpoint) -> Option<SocketAddr> {
    let report = endpoint.net_report().get();
    let global_v6 = report.as_ref().and_then(|r| r.global_v6).map(SocketAddr::V6);
    let varies = report.as_ref().and_then(|r| r.mapping_varies_by_dest_ipv6);
    let ctx = current_nat66_context(endpoint);
    evaluate_nat66_state(endpoint.id(), ctx, global_v6, varies)
}

/// 在后台捕获到新的 net_report 时顺手刷新缓存，避免下一次周期性刷新把它刷成 None
pub fn record_net_report(endpoint: &Endpoint) {
    let _ = nat66_outbound(endpoint);
}

/// 本机能用来直连的 IPv6 地址：网卡上的公网 IPv6 配上 iroh 的 IPv6 端口；
/// 网卡上没有公网 IPv6 时，用路由器转换后的出口（见 [`nat66_outbound`]）。
/// 隧道网卡（VPN）上的地址不要，那条路不是本机自己的网络。
pub fn local_candidates(endpoint: &Endpoint) -> Vec<SocketAddr> {
    let found = interface_candidates(endpoint);
    if !found.is_empty() {
        return found;
    }
    nat66_outbound(endpoint).into_iter().collect()
}

fn pick_interface_candidates(ifaces: &[netdev::Interface], port: u16) -> Vec<SocketAddr> {
    let mut stable = Vec::new();
    let mut temp = Vec::new();
    for iface in ifaces {
        for (ip, is_temp) in usable_interface_v6(iface) {
            let sock = SocketAddr::new(IpAddr::V6(ip), port);
            if usable(&sock) {
                if is_temp {
                    temp.push(sock);
                } else {
                    stable.push(sock);
                }
            }
        }
    }
    stable.sort();
    stable.dedup();
    temp.sort();
    temp.dedup();
    for addr in temp {
        if !stable.contains(&addr) {
            stable.push(addr);
        }
    }
    stable.truncate(MAX_CANDIDATES);
    stable
}

fn interface_candidates(endpoint: &Endpoint) -> Vec<SocketAddr> {
    let Some(port) = bound_v6_port(endpoint) else {
        return Vec::new();
    };
    let ifaces = netdev::get_interfaces();
    pick_interface_candidates(&ifaces, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify() {
        let g: SocketAddr = "[240e:3b4:1::5]:7788".parse().unwrap();
        let ula: SocketAddr = "[fd00::1]:7788".parse().unwrap();
        let ll: SocketAddr = "[fe80::1]:7788".parse().unwrap();
        let mapped: SocketAddr = "[::ffff:1.2.3.4]:7788".parse().unwrap();
        let v4: SocketAddr = "1.2.3.4:7788".parse().unwrap();
        assert!(usable(&g));
        assert!(!usable(&ll));
        assert!(!usable(&mapped));
        assert!(!usable(&v4));
        if !allow_ula() {
            assert!(!usable(&ula));
        }
        assert_eq!(pick([&v4, &g, &g, &ll]), vec![g]);
    }

    #[test]
    fn filters_invalid_interface_flags_and_prefers_stable_addrs() {
        use netdev::interface::flags::IFF_UP;
        use netdev::interface::ipv6_addr_flags::Ipv6AddrFlags;
        use netdev::ipnet::Ipv6Net;

        let mut iface = netdev::Interface::dummy();
        iface.flags = IFF_UP as u32;
        let push = |iface: &mut netdev::Interface, addr: &str, flags: Ipv6AddrFlags| {
            iface.ipv6.push(Ipv6Net::new(addr.parse().unwrap(), 64).unwrap());
            iface.ipv6_addr_flags.push(flags);
        };
        push(&mut iface, "240e:1::1", Ipv6AddrFlags { tentative: true, ..Default::default() });
        push(&mut iface, "240e:1::2", Ipv6AddrFlags { duplicated: true, ..Default::default() });
        push(&mut iface, "240e:1::3", Ipv6AddrFlags { deprecated: true, ..Default::default() });
        for i in 1..=6 {
            push(&mut iface, &format!("240e:1::1{i}"), Ipv6AddrFlags { temporary: true, ..Default::default() });
        }
        // 字典序排在临时地址后面的稳定地址，不能被临时地址挤出前 6 个
        push(&mut iface, "240e:ffff::1", Ipv6AddrFlags::default());

        let picked = pick_interface_candidates(&[iface], 7788);
        assert_eq!(picked.len(), MAX_CANDIDATES);
        assert_eq!(picked[0], "[240e:ffff::1]:7788".parse().unwrap());
        assert!(!picked.contains(&"[240e:1::1]:7788".parse().unwrap()));
        assert!(!picked.contains(&"[240e:1::2]:7788".parse().unwrap()));
        assert!(!picked.contains(&"[240e:1::3]:7788".parse().unwrap()));
    }

    #[test]
    fn nat66_cache_survives_transient_none_and_invalidates_on_change() {
        let id = iroh::SecretKey::generate().public();
        let ula: Ipv6Addr = "fd00::10".parse().unwrap();
        let ctx = Nat66Context { v6_port: Some(12345), addrs: vec![ula] };
        let out: SocketAddr = "[240e:3b4:1::5]:12345".parse().unwrap();

        assert_eq!(evaluate_nat66_state(id, ctx.clone(), Some(out), Some(false)), Some(out));
        // 周期刷新丢掉 global_v6 时，同网络上下文应继续命中缓存
        assert_eq!(evaluate_nat66_state(id, ctx.clone(), None, None), Some(out));

        // 本地监听端口或网卡地址变化时立即失效
        let changed_port = Nat66Context { v6_port: Some(12346), addrs: vec![ula] };
        assert_eq!(evaluate_nat66_state(id, changed_port, None, None), None);

        // 重新写入后若检测到对称映射（varies_by_dest == Some(true)），必须清除缓存
        assert_eq!(evaluate_nat66_state(id, ctx.clone(), Some(out), None), Some(out));
        assert_eq!(evaluate_nat66_state(id, ctx.clone(), Some(out), Some(true)), None);
        assert_eq!(evaluate_nat66_state(id, ctx, None, None), None);

        invalidate_nat66_cache(id);
    }
}
