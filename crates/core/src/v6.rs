//! IPv6 直连：两边都有公网 IPv6 时，经牵线服务器约好同时互相发包，第一个包就走直连，不经过中继。
//!
//! 公网 IPv6 没有 NAT，网卡上的地址就是对外地址，不用问外面“我是谁”；
//! 家用路由器对 IPv6 只有状态防火墙，双方各自先往对方发一个包，回包就放行了。
//! 所以这里不打洞，只是让被连方在连接方敲门的同时也往外发包。

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use iroh::Endpoint;

/// 一次最多报几个地址（隐私扩展会给一块网卡好几个临时地址）
pub const MAX_CANDIDATES: usize = 6;

/// 2000::/3，全球单播地址
pub fn is_global(ip: &Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xe000) == 0x2000
}

/// fc00::/7，局域网内用的 IPv6
fn is_ula(ip: &Ipv6Addr) -> bool {
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

/// 本机能用来直连的 IPv6 地址：网卡上的公网 IPv6 配上 iroh 的 IPv6 端口。
/// 隧道网卡（VPN）上的地址不要，那条路不是本机自己的网络。
pub fn local_candidates(endpoint: &Endpoint) -> Vec<SocketAddr> {
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
