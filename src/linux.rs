use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub mod geo;

pub const HELPER: &str = "/usr/lib/tangledlink-desktop/tangledlink-tun-helper";
pub const CORE: &str = "/usr/lib/tangledlink-desktop/xray";
pub const INTERFACE: &str = "tangledlink0";
pub const APP_SLICE: &str = "tangledlinkapps.slice";

pub fn split_cgroup(uid: u32) -> String {
    format!("user.slice/user-{uid}.slice/user@{uid}.service/{APP_SLICE}")
}

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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub concurrency: u8,
    pub fragment: bool,
    pub proxy_only: bool,
    pub split_apps: bool,
    pub ipv6: bool,
    pub domains: Vec<String>,
    pub ips: Vec<String>,
    pub use_google_dns: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            concurrency: 8,
            fragment: false,
            proxy_only: false,
            split_apps: false,
            ipv6: false,
            domains: Vec::new(),
            ips: Vec::new(),
            use_google_dns: false,
        }
    }
}

impl Options {
    pub fn valid(&self) -> bool {
        !(self.proxy_only && self.split_apps)
            && (1..=128).contains(&self.concurrency)
            && self.domains.len() <= 64
            && self.ips.len() <= 64
            && self.domains.iter().all(|value| {
                let name = value.strip_prefix("geosite:").unwrap_or(value);
                !name.is_empty()
                    && name.len() <= 253
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
            })
            && self.ips.iter().all(|value| {
                if let Some(code) = value.strip_prefix("geoip:") {
                    return !code.is_empty()
                        && code.len() <= 24
                        && code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-');
                }
                if let Some((ip, prefix)) = value.split_once('/') {
                    return ip
                        .parse::<IpAddr>()
                        .ok()
                        .zip(prefix.parse::<u8>().ok())
                        .is_some_and(|(ip, prefix)| prefix <= if ip.is_ipv4() { 32 } else { 128 });
                }
                value.parse::<IpAddr>().is_ok()
            })
    }
}

pub fn config(
    mut outbound: Value,
    routes: &[String],
    mtu: u16,
    local_direct: bool,
    mux: bool,
    options: &Options,
) -> Value {
    let network = outbound["streamSettings"]["network"].as_str();
    let vision = outbound["settings"]["vnext"][0]["users"][0]["flow"]
        .as_str()
        .is_some_and(|flow| flow.starts_with("xtls-rprx-vision"));
    if mux && network == Some("raw") && !vision {
        outbound["mux"] = json!({"enabled": true, "concurrency": options.concurrency});
    }
    let mut rules = vec![json!({"inboundTag": ["tun"], "port": 53, "outboundTag": "dns-out"})];
    if local_direct {
        rules.push(json!({
            "inboundTag": ["tun"],
            "ip": ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"],
            "outboundTag": "local-direct"
        }));
    }
    let domains: Vec<_> = options
        .domains
        .iter()
        .map(|name| {
            if name.starts_with("geosite:") {
                name.clone()
            } else {
                format!("domain:{name}")
            }
        })
        .collect();
    if !domains.is_empty() {
        rules.push(json!({"inboundTag":["tun"], "domain": domains, "outboundTag":"local-direct"}));
    }
    if !options.ips.is_empty() {
        rules.push(json!({"inboundTag":["tun"], "ip": options.ips, "outboundTag":"local-direct"}));
    }
    let fragment = options.fragment && outbound["streamSettings"]["network"] == "raw";
    if fragment {
        outbound["streamSettings"]["finalmask"]["tcp"] = json!([{
            "type": "fragment",
            "settings": {"packets":"tlshello","length":"100-200","interval":"10-20"}
        }]);
    }
    let outbounds = vec![
        outbound,
        json!({"protocol":"dns","tag":"dns-out"}),
        json!({"protocol":"freedom","tag":"local-direct"}),
    ];
    let mut dns = ["https://1.1.1.1/dns-query", "https://8.8.8.8/dns-query"];
    if options.use_google_dns {
        dns.swap(0, 1);
    }
    let gateways = ["172.30.254.1/30", "fd79:746e:676c::1/64"];
    json!({
        "log": { "loglevel": "none" },
        "inbounds": [{
            "protocol": "tun",
            "sniffing": {"enabled": !domains.is_empty(), "destOverride": ["http", "tls", "quic"], "routeOnly": true},
            "tag": "tun",
            "settings": {
                "name": INTERFACE,
                "mtu": mtu,
                "gateway": &gateways[..if options.ipv6 { 2 } else { 1 }],
                "autoSystemRoutingTable": if options.split_apps { &[][..] } else { routes },
                "autoOutboundsInterface": "auto",
                "autoSystemDnsToGateway": !options.split_apps
            }
        }],
        "dns": { "servers": dns },
        "routing": { "rules": rules },
        "outbounds": outbounds
    })
}

pub fn proxy_config(
    outbound: Value,
    mtu: u16,
    local_direct: bool,
    mux: bool,
    options: &Options,
) -> Value {
    let mut payload = config(outbound, &[], mtu, local_direct, mux, options);
    let inbound = &mut payload["inbounds"][0];
    inbound["protocol"] = json!("socks");
    inbound["listen"] = json!("127.0.0.1");
    inbound["port"] = json!(10808);
    inbound["settings"] = json!({"auth":"noauth","udp":true});
    payload
}

fn prepare_app_slice() -> Result<(), &'static str> {
    let uid = unsafe { libc::getuid() };
    let path = Path::new("/sys/fs/cgroup").join(split_cgroup(uid));
    if !path.is_dir() {
        // An active, exited service pins the cgroup ID without keeping a process alive.
        let _ = Command::new("/usr/bin/systemd-run")
            .args([
                "--user",
                "--quiet",
                "--unit=tangledlink-split-anchor",
                "--remain-after-exit",
            ])
            .arg(format!("--slice={APP_SLICE}"))
            .arg("/usr/bin/true")
            .output();
    }
    if !path.is_dir() {
        return Err("Для раздельного туннелирования нужны пользовательский systemd и cgroup v2.");
    }
    Ok(())
}

pub fn app_scope(program: &Path) -> Result<Child, &'static str> {
    use std::os::unix::fs::PermissionsExt;
    if !program.is_absolute()
        || !program
            .metadata()
            .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
    {
        return Err("Укажи абсолютный путь к исполняемому файлу.");
    }
    Command::new("/usr/bin/systemd-run")
        .args(["--user", "--scope", "--quiet"])
        .arg(format!("--slice={APP_SLICE}"))
        .arg("--")
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Не удалось запустить приложение через VPN.")
}

pub struct Tunnel {
    child: Child,
    proxy_only: bool,
    _assets: Option<geo::Assets>,
    split_apps: bool,
}

impl Tunnel {
    pub fn start(
        profile: &str,
        mtu: u16,
        local_direct: bool,
        mux: bool,
        options: &Options,
        bundle: Option<&Path>,
    ) -> Result<Self, &'static str> {
        if options.proxy_only {
            return Self::start_proxy(profile, mtu, local_direct, mux, options, bundle);
        }
        if !Path::new(HELPER).is_file() {
            return Err("Установи системный помощник TangledLink из пакета Linux.");
        }
        if options.split_apps {
            prepare_app_slice()?;
        }
        let mut command = Command::new("/usr/bin/pkexec");
        command.args(["--disable-internal-agent", HELPER]);
        command.args([
            mtu.to_string(),
            u8::from(local_direct).to_string(),
            u8::from(mux).to_string(),
            serde_json::to_string(options).map_err(|_| "Не удалось подготовить настройки")?,
        ]);
        let mut custom = bundle
            .map(std::fs::File::open)
            .transpose()
            .map_err(|_| "Не удалось открыть обновлённые геофайлы")?;
        if custom.is_some() {
            command.arg("geo");
        }
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Не удалось запустить polkit.")?;
        let mut tunnel = Self {
            child,
            proxy_only: false,
            _assets: None,
            split_apps: options.split_apps,
        };
        let mut input = tunnel
            .child
            .stdin
            .take()
            .ok_or("Не удалось передать профиль помощнику.")?;
        let output = tunnel
            .child
            .stdout
            .take()
            .ok_or("Нет ответа от помощника.")?;
        let (send_input, receive_input) = mpsc::sync_channel(1);
        let profile = profile.to_owned();
        std::thread::spawn(move || {
            let written = writeln!(input, "{profile}").and_then(|_| {
                custom
                    .as_mut()
                    .map_or(Ok(0), |file| std::io::copy(file, &mut input))
            });
            if written.is_ok() {
                let _ = send_input.send(input);
            }
        });
        let (send, receive) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(output).read_line(&mut line);
            let _ = send.send(line);
        });
        let issue = match receive.recv_timeout(Duration::from_secs(90)).as_deref() {
            Ok("ready\n") => {
                tunnel.child.stdin = Some(
                    receive_input
                        .recv()
                        .map_err(|_| "Не удалось передать профиль помощнику.")?,
                );
                return Ok(tunnel);
            }
            Ok("busy\n") => "Другой туннель TangledLink уже запущен.",
            Ok("dns\n") => "Для системного VPN нужны активный systemd-resolved и resolvectl.",
            Ok("tun\n") => "Системное устройство /dev/net/tun недоступно.",
            Ok("ipv6\n") => "Не удалось включить IPv6 на интерфейсе TangledLink.",
            Ok("split\n") => {
                "Не удалось настроить раздельное туннелирование. Проверь nftables и cgroup v2."
            }
            Ok("geo\n") => "Не удалось открыть обновлённые геофайлы",
            Ok("failed\n") => "Xray не смог настроить туннель Linux.",
            Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                "Запуск отменён, не подтверждён в polkit или превышено время ожидания."
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(receive_input);
                if tunnel.child.kill().is_ok() {
                    let _ = tunnel.child.wait();
                } else {
                    std::thread::spawn(move || {
                        let _ = tunnel.child.wait();
                    });
                }
                return Err(
                    "Запуск отменён, не подтверждён в polkit или превышено время ожидания.",
                );
            }
        };
        drop(receive_input);
        let _ = tunnel.child.wait();
        Err(issue)
    }

    fn start_proxy(
        profile: &str,
        mtu: u16,
        local_direct: bool,
        mux: bool,
        options: &Options,
        bundle: Option<&Path>,
    ) -> Result<Self, &'static str> {
        let outbound = crate::reality_outbound(profile)?;
        let payload = proxy_config(outbound, mtu, local_direct, mux, options);
        if std::net::TcpListener::bind(("127.0.0.1", 10808)).is_err() {
            return Err("Порт SOCKS5 10808 уже занят");
        }
        let assets = bundle
            .map(|path| {
                std::fs::File::open(path)
                    .and_then(|file| geo::Assets::from_bundle(file, &std::env::temp_dir()))
            })
            .transpose()
            .map_err(|_| "Не удалось открыть обновлённые геофайлы")?;
        let mut command = Command::new(CORE);
        command
            .args(["run", "-c", "stdin:", "-format", "json"])
            .env(
                "XRAY_LOCATION_ASSET",
                assets
                    .as_ref()
                    .map_or(Path::new(geo::PACKAGED), geo::Assets::path),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command
            .spawn()
            .map_err(|_| "Установи пакет TangledLink с ядром Xray")?;
        let mut tunnel = Self {
            child,
            proxy_only: true,
            _assets: assets,
            split_apps: false,
        };
        tunnel
            .child
            .stdin
            .take()
            .ok_or("Нет входа Xray")?
            .write_all(payload.to_string().as_bytes())
            .map_err(|_| "Не удалось передать настройки Xray")?;
        for _ in 0..40 {
            if !tunnel.running()? {
                return Err("Xray не запустил SOCKS5");
            }
            if std::net::TcpStream::connect(("127.0.0.1", 10808)).is_ok() {
                return Ok(tunnel);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("SOCKS5 не ответил")
    }

    pub fn routes_apps(&mut self) -> Result<bool, &'static str> {
        Ok(self.split_apps && self.running()?)
    }

    pub fn running(&mut self) -> Result<bool, &'static str> {
        self.child
            .try_wait()
            .map(|result| result.is_none())
            .map_err(|_| "Не удалось проверить состояние VPN.")
    }

    pub fn stop(&mut self) -> Result<(), &'static str> {
        if self.proxy_only {
            if self.running()? {
                self.child
                    .kill()
                    .map_err(|_| "Не удалось остановить SOCKS5")?;
                self.child
                    .wait()
                    .map_err(|_| "Не удалось остановить SOCKS5")?;
            }
            return Ok(());
        }
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
        if self.proxy_only || self.child.stdin.is_some() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn split_mode_leaves_system_routes_and_dns_alone() {
        use super::{Options, config};
        use serde_json::json;
        let routes = vec!["0.0.0.0/1".into()];
        let options: Options = serde_json::from_value(json!({"split_apps": true})).unwrap();
        assert!(options.valid());
        let payload = config(json!({}), &routes, 1500, false, false, &options);
        let settings = &payload["inbounds"][0]["settings"];
        assert_eq!(settings["autoSystemRoutingTable"], json!([]));
        assert_eq!(settings["autoSystemDnsToGateway"], false);

        let settings = &config(json!({}), &routes, 1500, false, false, &Options::default())["inbounds"]
            [0]["settings"];
        assert_eq!(settings["autoSystemRoutingTable"], json!(routes));
        assert_eq!(settings["autoSystemDnsToGateway"], true);
        assert!(
            !Options {
                proxy_only: true,
                ..options
            }
            .valid()
        );
    }

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
