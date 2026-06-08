use std::net::IpAddr;

use ipnet::IpNet;

#[derive(Debug, Clone)]
pub struct LocalIpInfo {
    pub ip: String,
    pub is_preferred: bool,
}

pub fn get_local_ips() -> Vec<LocalIpInfo> {
    let mut results = get_effective_local_ips();

    if results.is_empty() {
        results.push(LocalIpInfo {
            ip: "127.0.0.1".to_string(),
            is_preferred: true,
        });
    }

    results
}

pub fn get_effective_local_ips() -> Vec<LocalIpInfo> {
    let preferred_ip = detect_preferred_ip();
    let mut seen = std::collections::HashSet::<String>::new();
    let mut results = Vec::new();

    #[cfg(unix)]
    {
        if let Ok(ifaddrs) = nix::ifaddrs::getifaddrs() {
            for ifaddr in ifaddrs {
                if !is_usable_interface(&ifaddr) {
                    continue;
                }
                let Some(addr) = ifaddr.address else {
                    continue;
                };
                let ip = if let Some(sockaddr) = addr.as_sockaddr_in() {
                    IpAddr::V4(sockaddr.ip())
                } else {
                    continue;
                };
                if !is_effective_client_ip(&ip) {
                    continue;
                }
                let ip_str = ip.to_string();
                if seen.insert(ip_str.clone()) {
                    let is_preferred = preferred_ip.as_deref() == Some(ip_str.as_str());
                    results.push(LocalIpInfo {
                        ip: ip_str,
                        is_preferred,
                    });
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(netifas) = local_ip_address::list_afinet_netifas() {
            for (name, ip) in netifas {
                if !ip.is_ipv4() {
                    continue;
                }
                if is_virtual_interface_name(&name) {
                    continue;
                }
                if !is_effective_client_ip(&ip) {
                    continue;
                }
                let ip_str = ip.to_string();
                if seen.insert(ip_str.clone()) {
                    let is_preferred = preferred_ip.as_deref() == Some(ip_str.as_str());
                    results.push(LocalIpInfo {
                        ip: ip_str,
                        is_preferred,
                    });
                }
            }
        }
    }

    if let Some(ref pref) = preferred_ip {
        if !results.iter().any(|r| r.ip == *pref) {
            results.insert(
                0,
                LocalIpInfo {
                    ip: pref.clone(),
                    is_preferred: true,
                },
            );
        }
    }

    results.sort_by_key(|b| std::cmp::Reverse(b.is_preferred));

    results
}

fn detect_preferred_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        return None;
    }
    if !is_effective_client_ip(&ip) {
        return None;
    }
    Some(ip.to_string())
}

pub fn is_effective_client_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            if ipv4.is_loopback()
                || ipv4.is_link_local()
                || ipv4.is_unspecified()
                || ipv4.is_multicast()
                || ipv4.is_broadcast()
                || ipv4.is_documentation()
            {
                return false;
            }
            let octets = ipv4.octets();
            if octets[0] == 0 || octets[0] >= 224 {
                return false;
            }
            if octets[0] == 169 && octets[1] == 254 {
                return false;
            }
            if octets[0] == 198 && (18..=19).contains(&octets[1]) {
                return false;
            }
            true
        }
        IpAddr::V6(_) => false,
    }
}

#[cfg(unix)]
fn is_usable_interface(ifaddr: &nix::ifaddrs::InterfaceAddress) -> bool {
    use nix::net::if_::InterfaceFlags;

    let flags = ifaddr.flags;
    if !flags.contains(InterfaceFlags::IFF_UP) || !flags.contains(InterfaceFlags::IFF_RUNNING) {
        return false;
    }
    if flags.contains(InterfaceFlags::IFF_LOOPBACK) {
        return false;
    }

    let name = &ifaddr.interface_name;
    !is_virtual_interface_name(name)
}

fn is_virtual_interface_name(name: &str) -> bool {
    const VIRTUAL_PREFIXES: &[&str] = &[
        "docker",
        "br-",
        "veth",
        "vnet",
        "virbr",
        "cni",
        "flannel",
        "calico",
        "weave",
        "cilium",
        "lxc",
        "lxd",
        "podman",
        "crc",
        "tun",
        "tap",
        "wg",
        "tailscale",
        "utun",
        "ipsec",
        "ppp",
        "vmnet",
        "vmware",
        "vboxnet",
        "bridge",
        "dummy",
    ];
    let lower = name.to_lowercase();
    VIRTUAL_PREFIXES.iter().any(|p| lower.starts_with(p))
}

pub fn get_local_subnets() -> Vec<IpNet> {
    let mut subnets = Vec::new();

    #[cfg(unix)]
    {
        if let Ok(ifaddrs) = nix::ifaddrs::getifaddrs() {
            for ifaddr in ifaddrs {
                if !is_usable_interface(&ifaddr) {
                    continue;
                }
                let Some(addr) = ifaddr.address else {
                    continue;
                };
                let ip = if let Some(sockaddr) = addr.as_sockaddr_in() {
                    IpAddr::V4(sockaddr.ip())
                } else {
                    continue;
                };
                if ip.is_loopback() || ip.is_unspecified() {
                    continue;
                }
                let prefix_len = ifaddr
                    .netmask
                    .and_then(|m| m.as_sockaddr_in().map(|s| s.ip()))
                    .map(|mask| u32::from(mask).count_ones() as u8)
                    .unwrap_or(24);
                if let Ok(subnet) = IpNet::new(ip, prefix_len) {
                    let network = subnet.trunc();
                    if !subnets.contains(&network) {
                        subnets.push(network);
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(netifas) = local_ip_address::list_afinet_netifas() {
            for (name, ip) in netifas {
                if !ip.is_ipv4() || ip.is_loopback() || ip.is_unspecified() {
                    continue;
                }
                if is_virtual_interface_name(&name) {
                    continue;
                }
                if let Ok(subnet) = IpNet::new(ip, 24) {
                    let network = subnet.trunc();
                    if !subnets.contains(&network) {
                        subnets.push(network);
                    }
                }
            }
        }
    }

    subnets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_virtual_interface_name_filters_docker() {
        assert!(is_virtual_interface_name("docker0"));
        assert!(is_virtual_interface_name("docker_gwbridge"));
    }

    #[test]
    fn test_is_virtual_interface_name_filters_veth() {
        assert!(is_virtual_interface_name("vethaa11bb22"));
        assert!(is_virtual_interface_name("veth0"));
    }

    #[test]
    fn test_is_virtual_interface_name_filters_bridge() {
        assert!(is_virtual_interface_name("br-abcdef123456"));
        assert!(is_virtual_interface_name("bridge0"));
        assert!(is_virtual_interface_name("virbr0"));
    }

    #[test]
    fn test_is_virtual_interface_name_filters_vpn() {
        assert!(is_virtual_interface_name("tun0"));
        assert!(is_virtual_interface_name("tap0"));
        assert!(is_virtual_interface_name("wg0"));
        assert!(is_virtual_interface_name("tailscale0"));
        assert!(is_virtual_interface_name("utun3"));
        assert!(is_virtual_interface_name("ppp0"));
    }

    #[test]
    fn test_is_virtual_interface_name_filters_vm() {
        assert!(is_virtual_interface_name("vmnet1"));
        assert!(is_virtual_interface_name("vmnet8"));
        assert!(is_virtual_interface_name("vboxnet0"));
    }

    #[test]
    fn test_is_virtual_interface_name_filters_container_orchestration() {
        assert!(is_virtual_interface_name("cni0"));
        assert!(is_virtual_interface_name("flannel.1"));
        assert!(is_virtual_interface_name("calico-xyz"));
        assert!(is_virtual_interface_name("weave"));
        assert!(is_virtual_interface_name("cilium_host"));
    }

    #[test]
    fn test_is_virtual_interface_name_allows_physical() {
        assert!(!is_virtual_interface_name("eth0"));
        assert!(!is_virtual_interface_name("eth1"));
        assert!(!is_virtual_interface_name("en0"));
        assert!(!is_virtual_interface_name("en1"));
        assert!(!is_virtual_interface_name("ens33"));
        assert!(!is_virtual_interface_name("enp0s3"));
        assert!(!is_virtual_interface_name("wlan0"));
        assert!(!is_virtual_interface_name("wlp2s0"));
    }

    #[test]
    fn test_is_virtual_interface_name_case_insensitive() {
        assert!(is_virtual_interface_name("Docker0"));
        assert!(is_virtual_interface_name("VETH123"));
    }

    #[test]
    fn test_is_effective_client_ip_accepts_private() {
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "172.16.0.1".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_rejects_loopback() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_rejects_link_local() {
        let ip: IpAddr = "169.254.1.1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_accepts_cgn() {
        let ip: IpAddr = "100.64.0.1".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "100.86.178.33".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "100.127.255.255".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_accepts_public() {
        let ip: IpAddr = "100.63.255.255".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "100.128.0.1".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_rejects_special_public_ranges() {
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
        let ip: IpAddr = "198.18.0.1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
        let ip: IpAddr = "224.0.0.1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
    }

    #[test]
    fn test_is_effective_client_ip_rejects_ipv6() {
        let ip: IpAddr = "::1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
        let ip: IpAddr = "fe80::1".parse().unwrap();
        assert!(!is_effective_client_ip(&ip));
    }

    #[test]
    fn test_get_local_ips_returns_non_empty() {
        let ips = get_local_ips();
        assert!(!ips.is_empty());
    }

    #[test]
    fn test_get_local_ips_preferred_is_first() {
        let ips = get_local_ips();
        if ips.len() > 1 {
            let has_preferred = ips.iter().any(|i| i.is_preferred);
            if has_preferred {
                assert!(ips[0].is_preferred);
            }
        }
    }

    #[test]
    fn test_get_local_ips_no_duplicates() {
        let ips = get_local_ips();
        let mut seen = std::collections::HashSet::new();
        for ip in &ips {
            assert!(seen.insert(&ip.ip), "duplicate IP: {}", ip.ip);
        }
    }

    #[test]
    fn test_get_local_ips_all_entries_are_valid_addresses() {
        let ips = get_local_ips();
        for info in &ips {
            let parsed: IpAddr = info
                .ip
                .parse()
                .unwrap_or_else(|_| panic!("invalid IP returned: {}", info.ip));
            assert!(
                !parsed.is_unspecified(),
                "returned unspecified address: {}",
                info.ip
            );
        }
    }
}
