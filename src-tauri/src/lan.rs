//! 局域网共享: this computer's LAN addresses, for other devices to use the
//! proxy port.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

use serde::Serialize;

#[derive(Serialize, Debug, PartialEq)]
pub struct LanAddr {
    pub ip: String,
    /// Network adapter name, e.g. `WLAN` / `en0`.
    pub iface: String,
}

fn is_lan(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    // TUN adapters (ours: 172.19.0.1/30) aren't reachable from other devices.
    ip.is_private() && !(o[0] == 172 && o[1] == 19) && !(o[0] == 198 && (o[1] & 0xfe) == 18)
}

/// Adapters that are virtual / not on the physical LAN go last.
fn virtual_iface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["vethernet", "virtualbox", "vmware", "vmnet", "docker", "wsl", "hyper-v", "prism", "wintun", "utun", "tailscale", "zerotier", "bridge", "loopback"]
        .iter()
        .any(|v| n.contains(v))
}

/// The address used for the default route (no packet is sent).
fn primary() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("223.5.5.5:53").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(_) => None,
    }
}

/// LAN IPv4 addresses, the most likely one first.
pub fn addresses() -> Vec<LanAddr> {
    let primary = primary();
    let mut out: Vec<(u8, LanAddr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !i.is_loopback() && is_lan(v4) => {
                let rank = if Some(v4) == primary {
                    0
                } else if virtual_iface(&i.name) {
                    3
                } else if v4.octets()[0] == 192 {
                    1
                } else {
                    2
                };
                Some((rank, LanAddr { ip: v4.to_string(), iface: i.name }))
            }
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.ip.cmp(&b.1.ip)));
    out.dedup_by(|a, b| a.1.ip == b.1.ip);
    out.into_iter().map(|(_, a)| a).collect()
}

/// Index of the physical adapter with the most likely LAN address (for
/// pinning sockets past the TUN routes).
pub fn physical_index() -> Option<u32> {
    let best = addresses().into_iter().next()?;
    if_addrs::get_if_addrs().ok()?.into_iter().find(|i| i.name == best.iface).and_then(|i| i.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert!(is_lan("192.168.1.5".parse().unwrap()));
        assert!(is_lan("10.0.0.2".parse().unwrap()));
        assert!(!is_lan("172.19.0.1".parse().unwrap()), "TUN");
        assert!(!is_lan("8.8.8.8".parse().unwrap()));
        assert!(virtual_iface("vEthernet (WSL)") && !virtual_iface("WLAN"));
        // Whatever this machine has: no loopback, no duplicates.
        let a = addresses();
        assert!(a.iter().all(|x| !x.ip.starts_with("127.")));
        eprintln!("{a:?}");
    }
}
