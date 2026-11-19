use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub const HELPER: &str = "/usr/lib/tangledlink-desktop/tangledlink-tun-helper";
pub const CORE: &str = "/usr/lib/tangledlink-desktop/xray";
pub const INTERFACE: &str = "tangledlink0";

// Exclude the selected endpoint so replies arrive on the same interface even
// when the kernel checks the reverse route for packets on the physical link.
pub fn routes(endpoint: IpAddr) -> Vec<String> {
    fn exclude(address: u128, width: u32, format: impl Fn(u128) -> String) -> Vec<String> {
        (1..=width)
            .map(|prefix| {
                let host_bits = width - prefix;
                let sibling = ((address >> host_bits) ^ 1) << host_bits;
                format!("{}/{prefix}", format(sibling))
            })
            .collect()
    }
    match endpoint {
        IpAddr::V4(address) => {
            let mut result = exclude(u32::from(address).into(), 32, |ip| {
                Ipv4Addr::from(ip as u32).to_string()
            });
            result.extend(["::/1".to_owned(), "8000::/1".to_owned()]);
            result
        }
        IpAddr::V6(address) => {
            let mut result = vec!["0.0.0.0/1".to_owned(), "128.0.0.0/1".to_owned()];
            result.extend(exclude(address.into(), 128, |ip| {
                Ipv6Addr::from(ip).to_string()
            }));
            result
        }
    }
}

pub fn config(outbound: Value, routes: &[String]) -> Value {
    json!({
        "log": { "loglevel": "none" },
        "inbounds": [{
            "protocol": "tun",
            "tag": "tun",
            "settings": {
                "name": INTERFACE,
                "mtu": 1500,
                "gateway": ["172.30.254.1/30", "fd79:746e:676c::1/64"],
                "autoSystemRoutingTable": routes,
                "autoOutboundsInterface": "auto",
                "autoSystemDnsToGateway": true
            }
        }],
        "dns": { "servers": ["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"] },
        "routing": { "rules": [
            { "inboundTag": ["tun"], "port": 53, "outboundTag": "dns-out" }
        ] },
        "outbounds": [outbound, { "protocol": "dns", "tag": "dns-out" }]
    })
}

pub struct Tunnel {
    child: Child,
}

impl Tunnel {
    pub fn start(profile: &str) -> Result<Self, &'static str> {
        if !Path::new(HELPER).is_file() {
            return Err("Установи системный помощник TangledLink из пакета Linux.");
        }
        let child = Command::new("/usr/bin/pkexec")
            .args(["--disable-internal-agent", HELPER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Не удалось запустить polkit.")?;
        let mut tunnel = Self { child };
        if !tunnel
            .child
            .stdin
            .as_mut()
            .is_some_and(|input| writeln!(input, "{profile}").is_ok())
        {
            return Err("Не удалось передать профиль помощнику.");
        }
        let output = tunnel
            .child
            .stdout
            .take()
            .ok_or("Нет ответа от помощника.")?;
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = send.send(line);
        });
        match receive.recv_timeout(Duration::from_secs(90)).as_deref() {
            Ok("ready\n") => Ok(tunnel),
            Ok("busy\n") => Err("Другой туннель TangledLink уже запущен."),
            Ok("dns\n") => Err("Для системного VPN нужны активный systemd-resolved и resolvectl."),
            Ok("tun\n") => Err("Системное устройство /dev/net/tun недоступно."),
            Ok("failed\n") => Err("Xray не смог настроить туннель Linux."),
            _ => Err("Запуск отменён, не подтверждён в polkit или превышено время ожидания."),
        }
    }

    pub fn running(&mut self) -> Result<bool, &'static str> {
        self.child
            .try_wait()
            .map(|result| result.is_none())
            .map_err(|_| "Не удалось проверить состояние VPN.")
    }

    pub fn stop(&mut self) -> Result<(), &'static str> {
        // EOF tells the privileged helper to close Xray and revert the network.
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(9);
        while self.running()? {
            if Instant::now() >= deadline {
                // This can still be our unprivileged pkexec process waiting for approval.
                let _ = self.child.kill();
                return Err("Помощник не подтвердил остановку VPN.");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if self.child.stdin.is_some() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn protects_all_ipv4_destinations_except_the_vpn_endpoint() {
        use std::net::Ipv4Addr;
        let endpoint = Ipv4Addr::new(44, 2, 3, 4);
        let routes = super::routes(endpoint.into());
        let contains = |ip: Ipv4Addr| {
            routes
                .iter()
                .filter_map(|route| {
                    let (network, prefix) = route.split_once('/')?;
                    let shift = 32 - prefix.parse::<u32>().ok()?;
                    let network = network.parse::<Ipv4Addr>().ok()?;
                    Some((u32::from(network) >> shift) == (u32::from(ip) >> shift))
                })
                .any(|matches| matches)
        };
        assert!(!contains(endpoint));
        assert!(contains(Ipv4Addr::new(44, 2, 3, 3)));
        assert!(contains(Ipv4Addr::new(44, 2, 3, 5)));
        assert!(contains(Ipv4Addr::new(255, 255, 255, 255)));
    }
}
