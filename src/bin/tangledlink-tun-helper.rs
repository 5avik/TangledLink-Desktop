#[cfg(target_os = "linux")]
#[path = "tun_helper/split.rs"]
mod split;

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = helper::run() {
        println!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
mod helper {
    use std::{
        fs::OpenOptions,
        io::{BufRead, BufReader, Read, Write},
        net::{IpAddr, ToSocketAddrs},
        os::{
            fd::AsRawFd,
            unix::{fs::OpenOptionsExt, process::CommandExt},
        },
        path::Path,
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };
    use tangledlink_desktop::{
        linux::{self, CORE, geo},
        reality_outbound,
    };

    pub fn run() -> Result<(), &'static str> {
        if unsafe { libc::geteuid() } != 0 {
            return Err("failed");
        }
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let custom_geo = args.len() == 5 && args[4] == "geo";
        let args = if custom_geo { &args[..4] } else { &args[..] };
        let mtu = match args {
            [] => 1500,
            [value] | [value, _, _] | [value, _, _, _] => value
                .to_str()
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| (1280..=1500).contains(value))
                .ok_or("failed")?,
            _ => return Err("failed"),
        };
        let (local_direct, mux) = match args {
            [_, local, mux] | [_, local, mux, _]
                if [local, mux]
                    .iter()
                    .all(|value| *value == "0" || *value == "1") =>
            {
                (local == "1", mux == "1")
            }
            [_, _, _] | [_, _, _, _] => return Err("failed"),
            _ => (false, false),
        };
        let options = if let Some(raw) = args.get(3) {
            let raw = raw
                .to_str()
                .filter(|value| value.len() <= 32768)
                .ok_or("failed")?;
            serde_json::from_str::<linux::Options>(raw)
                .ok()
                .filter(linux::Options::valid)
                .ok_or("failed")?
        } else {
            linux::Options::default()
        };
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open("/run/tangledlink-desktop.lock")
            .map_err(|_| "failed")?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("busy");
        }
        if OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")
            .is_err()
        {
            return Err("tun");
        }
        if !options.split_apps
            && !Command::new("/usr/bin/resolvectl")
                .arg("status")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        {
            return Err("dns");
        }

        let mut input = BufReader::new(std::io::stdin());
        let mut line = String::new();
        (&mut input)
            .take(65537)
            .read_line(&mut line)
            .map_err(|_| "profile")?;
        if line.len() > 65536 || !line.ends_with('\n') {
            return Err("profile");
        }
        let assets = if custom_geo {
            // Receive only fixed-name data files. Never open a caller-supplied path as root.
            Some(geo::Assets::from_bundle(&mut input, Path::new("/run")).map_err(|_| "geo")?)
        } else {
            None
        };
        let asset_path = assets
            .as_ref()
            .map_or(Path::new(geo::PACKAGED), geo::Assets::path);
        if !linux::check_geo_rules(&options, asset_path) {
            return Err("geo-rules");
        }
        let mut outbound = reality_outbound(&line).map_err(|_| "profile")?;
        let node = &mut outbound["settings"]["vnext"][0];
        let address = node["address"].as_str().ok_or("profile")?;
        if address.parse::<IpAddr>().is_err() {
            let port = node["port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .ok_or("profile")?;
            let mut choices = (address, port).to_socket_addrs().map_err(|_| "resolve")?;
            // Prefer an IPv4 endpoint while preserving IPv6-only profiles.
            let first = choices.next().ok_or("resolve")?;
            let endpoint = if first.is_ipv4() {
                first
            } else {
                choices.find(|addr| addr.is_ipv4()).unwrap_or(first)
            };
            node["address"] = serde_json::json!(endpoint.ip().to_string());
        }
        let endpoint = node["address"]
            .as_str()
            .and_then(|address| address.parse().ok())
            .ok_or("profile")?;
        let mut routes = linux::tun_routes(endpoint, local_direct);
        if !options.ipv6 {
            routes.retain(|route| !route.contains(':'));
        }
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut input, &mut std::io::sink());
            let _ = send.send(());
        });
        // Keep the prepared interface alive until after Xray shuts down.
        let _prepared_tun = if options.ipv6 {
            PreparedTun::for_ipv6()?
        } else {
            None
        };
        let mut command = Command::new(CORE);
        command
            .args(["run", "-c", "stdin:", "-format", "json"])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("XRAY_LOCATION_ASSET", asset_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let parent = unsafe { libc::getpid() };
        // Ask Xray to unwind its network settings if the privileged helper exits unexpectedly.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let mut child = Core(command.spawn().map_err(|_| "core")?);
        let payload =
            linux::config(outbound, &routes, mtu, local_direct, mux, &options).to_string();
        child
            .0
            .stdin
            .take()
            .ok_or("core")?
            .write_all(payload.as_bytes())
            .map_err(|_| "core")?;
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if child.0.try_wait().map_err(|_| "core")?.is_some() {
                return Err("core");
            }
            if Instant::now() >= deadline {
                return Err("routes");
            }
            if unsafe { libc::if_nametoindex(c"tangledlink0".as_ptr()) } != 0
                && std::net::UdpSocket::bind(("172.30.254.1", 0)).is_ok()
                && (options.split_apps || routes_ready(routes.last().ok_or("failed")?))
            {
                break;
            }
            if receive.recv_timeout(Duration::from_millis(75)).is_ok() {
                return Ok(());
            }
        }
        let _policy = if options.split_apps {
            let uid = std::env::var("PKEXEC_UID")
                .ok()
                .and_then(|uid| uid.parse::<u32>().ok())
                .filter(|uid| *uid != 0)
                .ok_or("split")?;
            Some(
                crate::split::Policy::install(uid, options.ipv6, local_direct)
                    .map_err(|_| "split")?,
            )
        } else {
            None
        };
        writeln!(std::io::stdout(), "ready").map_err(|_| "failed")?;
        loop {
            if child.0.try_wait().map_err(|_| "failed")?.is_some() {
                return Err("failed");
            }
            if receive.recv_timeout(Duration::from_millis(100)).is_ok() {
                return Ok(());
            }
        }
    }

    struct PreparedTun;

    impl PreparedTun {
        fn for_ipv6() -> Result<Option<Self>, &'static str> {
            if !std::fs::read_to_string("/proc/sys/net/ipv6/conf/default/disable_ipv6")
                .is_ok_and(|setting| setting.trim() == "1")
            {
                return Ok(None);
            }
            if unsafe { libc::if_nametoindex(c"tangledlink0".as_ptr()) } != 0 {
                return Err("ipv6");
            }
            Self::tuntap("add")?;
            let tun = Self;
            std::fs::write(
                format!("/proc/sys/net/ipv6/conf/{}/disable_ipv6", linux::INTERFACE),
                b"0",
            )
            .map_err(|_| "ipv6")?;
            Ok(Some(tun))
        }

        fn tuntap(action: &str) -> Result<(), &'static str> {
            Command::new("/usr/bin/ip")
                .args(["tuntap", action, "dev", linux::INTERFACE, "mode", "tun"])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()
                .filter(|status| status.success())
                .map(|_| ())
                .ok_or("ipv6")
        }
    }

    impl Drop for PreparedTun {
        fn drop(&mut self) {
            let _ = Self::tuntap("del");
        }
    }

    fn routes_ready(last_route: &str) -> bool {
        Command::new("/usr/bin/ip")
            .args([
                if last_route.contains(':') { "-6" } else { "-4" },
                "route",
                "show",
                "exact",
                last_route,
                "dev",
                linux::INTERFACE,
            ])
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success() && !output.stdout.is_empty())
    }

    struct Core(Child);

    impl Drop for Core {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(None)) {
                return;
            }
            unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if !matches!(self.0.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
