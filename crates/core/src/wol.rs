//! Wake-on-LAN: wake a sleeping computer when you move the pointer toward it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// MAC address of the network interface that owns `local_ip`
/// (the address this computer uses to talk to the server).
pub fn mac_for_ip(local_ip: IpAddr) -> Option<String> {
    let IpAddr::V4(v4) = local_ip else { return None };
    netdev::get_interfaces()
        .into_iter()
        .find(|i| i.ipv4.iter().any(|n| n.addr() == v4))
        .and_then(|i| i.mac_addr)
        .map(|m| m.to_string().to_lowercase())
        .filter(|m| m != "00:00:00:00:00:00")
}

pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<u8> = s.split([':', '-']).filter_map(|p| u8::from_str_radix(p, 16).ok()).collect();
    (parts.len() == 6).then(|| parts.try_into().unwrap())
}

pub fn magic_packet(mac: [u8; 6]) -> Vec<u8> {
    let mut p = vec![0xFFu8; 6];
    for _ in 0..16 {
        p.extend_from_slice(&mac);
    }
    p
}

/// Send the magic packet to the LAN broadcast address and, if known, the
/// computer's last address (some routers drop global broadcasts).
pub fn wake(mac: &str, last_ip: Option<IpAddr>) -> bool {
    let Some(mac) = parse_mac(mac) else { return false };
    let packet = magic_packet(mac);
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return false };
    let _ = sock.set_broadcast(true);
    let mut targets = vec![SocketAddr::from((Ipv4Addr::BROADCAST, 9))];
    if let Some(IpAddr::V4(ip)) = last_ip {
        let o = ip.octets();
        targets.push(SocketAddr::from((Ipv4Addr::new(o[0], o[1], o[2], 255), 9)));
        targets.push(SocketAddr::from((ip, 9)));
    }
    let mut ok = false;
    for t in targets {
        ok |= sock.send_to(&packet, t).is_ok();
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet() {
        let mac = parse_mac("AA:bb:cc:dd:ee:01").unwrap();
        let p = magic_packet(mac);
        assert_eq!(p.len(), 102);
        assert_eq!(&p[..6], &[0xFF; 6]);
        assert_eq!(&p[96..], &mac);
        assert!(parse_mac("nope").is_none());
    }
}
