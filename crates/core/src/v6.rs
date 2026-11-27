//! IPv6 直连：两边都有公网 IPv6 时，经牵线服务器约好同时互相发包，第一个包就走直连，不经过中继。
//!
//! 公网 IPv6 没有 NAT，网卡上的地址就是对外地址，不用问外面“我是谁”；
//! 家用路由器对 IPv6 只有状态防火墙，双方各自先往对方发一个包，回包就放行了。
//! 所以这里不打洞，只是让被连方在连接方敲门的同时也往外发包。
//!
//! 有的路由器只给电脑发 fd 开头的内网 IPv6，出去时再转换成公网 IPv6（NAT66）。网卡上看不到公网地址，
//! 但 iroh 连中继时能看到本机从外面看是哪个 IPv6 地址、出口端口会不会随目标变。不随目标变时，
//! 这个出口地址和公网 IPv6 一样能用：双方同时往对方的出口发包，路由器的映射就打开了。

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use iroh::{Endpoint, Watcher};

/// 一次最多报几个地址（隐私扩展会给一块网卡好几个临时地址）
pub const MAX_CANDIDATES: usize = 6;

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

/// 网卡上的 IPv6 地址（不算本地链接 fe80、不算隧道网卡）：(网卡名, 地址)，诊断时逐个列出来
pub fn interface_addrs() -> Vec<(String, Ipv6Addr)> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback() && !i.is_tun())
        .flat_map(|i| {
            let name = i.friendly_name.clone().unwrap_or_else(|| i.name.clone());
            i.ipv6.into_iter().map(move |n| (name.clone(), n.addr()))
        })
        .filter(|(_, ip)| is_global(ip) || is_ula(ip))
        .collect()
}

/// iroh 经中继看到的本机 IPv6 出口：网卡上只有内网 IPv6、路由器做了地址转换（NAT66）时，
/// 这就是对外的地址。出口端口随目标变（对称映射）时打不通，不算
pub fn nat66_outbound(endpoint: &Endpoint) -> Option<SocketAddr> {
    let report = endpoint.net_report().get()?;
    if report.mapping_varies_by_dest_ipv6 == Some(true) {
        return None;
    }
    let addr = SocketAddr::V6(report.global_v6?);
    usable(&addr).then_some(addr)
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

fn interface_candidates(endpoint: &Endpoint) -> Vec<SocketAddr> {
    let Some(port) = endpoint
        .bound_sockets()
        .iter()
        .find(|s| s.is_ipv6())
        .map(|s| s.port())
    else {
        return Vec::new();
    };
    let addrs: Vec<SocketAddr> = netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback() && !i.is_tun())
        .flat_map(|i| i.ipv6.into_iter().map(|n| n.addr()))
        .map(|ip| SocketAddr::new(IpAddr::V6(ip), port))
        .collect();
    pick(&addrs)
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
}
