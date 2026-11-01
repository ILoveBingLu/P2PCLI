use std::net::Ipv4Addr;

pub(super) fn ipv4() -> Vec<Ipv4Addr> {
    select(netdev::get_interfaces())
}

fn select(interfaces: impl IntoIterator<Item = netdev::Interface>) -> Vec<Ipv4Addr> {
    // Cover every usable network instead of trusting the default multicast route.
    // Keep virtual Ethernet/bridges: physical-only detection misses valid LANs.
    let mut addresses: Vec<_> = interfaces.into_iter()
        .filter(|interface| {
            interface.is_up() && interface.is_multicast()
                && !interface.is_loopback() && !interface.is_tun()
        })
        .flat_map(|interface| interface.ipv4.into_iter().map(|network| network.addr()))
        .filter(|address| {
            !address.is_unspecified() && !address.is_loopback()
                && !address.is_multicast() && !address.is_broadcast()
        })
        .collect();
    addresses.sort_unstable();
    addresses.dedup();
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdev::interface::flags::{IFF_BROADCAST, IFF_LOOPBACK, IFF_MULTICAST, IFF_POINTOPOINT, IFF_UP};

    fn interface(name: &str, addresses: &[&str], flags: u32) -> netdev::Interface {
        let mut interface = netdev::Interface::dummy();
        interface.name = name.into();
        interface.flags = flags;
        interface.ipv4 = addresses.iter().map(|address| address.parse().unwrap()).collect();
        interface
    }

    fn lan_flags() -> u32 {
        (IFF_UP | IFF_MULTICAST | IFF_BROADCAST) as u32
    }

    #[test]
    fn includes_wifi_even_when_vmware_is_first() {
        let actual = select([
            interface("VMware VMnet8", &["192.168.109.1/24"], lan_flags()),
            interface("WLAN", &["192.168.3.57/24"], lan_flags()),
            interface("Ethernet", &["10.0.0.2/24", "192.168.3.57/24"], lan_flags()),
        ]);
        assert_eq!(actual, vec![
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(192, 168, 3, 57),
            Ipv4Addr::new(192, 168, 109, 1),
        ]);
    }

    #[test]
    fn excludes_down_loopback_tunnels_and_non_multicast_interfaces() {
        assert!(select([
            interface("down", &["192.168.1.1/24"], IFF_MULTICAST as u32),
            interface("loopback", &["127.0.0.1/8"], lan_flags() | IFF_LOOPBACK as u32),
            interface("vpn", &["10.7.0.5/24"], (IFF_UP | IFF_MULTICAST | IFF_POINTOPOINT) as u32),
            interface("no-multicast", &["192.168.2.1/24"], IFF_UP as u32),
        ]).is_empty());
    }

    #[test]
    fn keeps_link_local_lans_without_a_gateway_but_rejects_special_addresses() {
        assert_eq!(select([
            interface("offline-lan", &["169.254.2.3/16"], lan_flags()),
            interface("invalid", &["0.0.0.0/32", "127.0.0.1/8", "224.0.0.251/32", "255.255.255.255/32"], lan_flags()),
        ]), vec![Ipv4Addr::new(169, 254, 2, 3)]);
    }

    #[test]
    fn refresh_uses_current_addresses_and_allows_ipv6_only_networks() {
        let before = select([interface("WLAN", &["192.168.3.57/24"], lan_flags())]);
        let after = select([interface("WLAN", &["192.168.8.57/24"], lan_flags())]);
        assert_ne!(before, after);
        assert_eq!(after, vec![Ipv4Addr::new(192, 168, 8, 57)]);
        assert!(select([interface("ipv6-only", &[], lan_flags())]).is_empty());
    }
}
