use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const LAN_V4: [&str; 4] = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "169.254.0.0/16",
];
pub const LAN_V6: [&str; 2] = ["fc00::/7", "fe80::/10"];

pub(crate) fn tun_routes(endpoint: Option<IpAddr>, local_direct: bool) -> Vec<String> {
    // Keep the VPN endpoint and optional LAN ranges in the original kernel routing.
    fn cover(
        network: u128,
        prefix: u32,
        width: u32,
        exclusions: &[(u128, u32)],
        emit: &mut impl FnMut(u128, u32),
    ) {
        let mut split = false;
        for &(address, length) in exclusions {
            let common = prefix.min(length);
            if common != 0 && network >> (width - common) != address >> (width - common) {
                continue;
            }
            if length <= prefix {
                return;
            }
            split = true;
        }
        if split || prefix == 0 {
            let next = prefix + 1;
            cover(network, next, width, exclusions, emit);
            cover(
                network | (1 << (width - next)),
                next,
                width,
                exclusions,
                emit,
            );
        } else {
            emit(network, prefix);
        }
    }

    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    if local_direct {
        for cidr in LAN_V4 {
            let (ip, prefix) = cidr.split_once('/').unwrap();
            v4.push((
                u32::from(ip.parse::<Ipv4Addr>().unwrap()).into(),
                prefix.parse().unwrap(),
            ));
        }
        for cidr in LAN_V6 {
            let (ip, prefix) = cidr.split_once('/').unwrap();
            v6.push((
                u128::from(ip.parse::<Ipv6Addr>().unwrap()),
                prefix.parse().unwrap(),
            ));
        }
    }
    match endpoint {
        Some(IpAddr::V4(ip)) => v4.push((u32::from(ip).into(), 32)),
        Some(IpAddr::V6(ip)) => v6.push((u128::from(ip), 128)),
        None => {}
    }
    let mut result = Vec::new();
    cover(0, 0, 32, &v4, &mut |ip, prefix| {
        result.push(format!("{}/{prefix}", Ipv4Addr::from(ip as u32)))
    });
    cover(0, 0, 128, &v6, &mut |ip, prefix| {
        result.push(format!("{}/{prefix}", Ipv6Addr::from(ip)))
    });
    result
}
