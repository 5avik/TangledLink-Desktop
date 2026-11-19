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
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, Instant},
    };
    use tangledlink_desktop::{
        linux::{self, CORE},
        reality_outbound,
    };

    pub fn run() -> Result<(), &'static str> {
        if unsafe { libc::geteuid() } != 0 || std::env::args_os().len() != 1 {
            return Err("failed");
        }
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
        if !Command::new("/usr/bin/resolvectl")
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
            .map_err(|_| "failed")?;
        if line.len() > 65536 || !line.ends_with('\n') {
            return Err("failed");
        }
        let mut outbound = reality_outbound(&line).map_err(|_| "failed")?;
        let node = &mut outbound["settings"]["vnext"][0];
        let address = node["address"].as_str().ok_or("failed")?;
        if address.parse::<IpAddr>().is_err() {
            let port = node["port"]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .ok_or("failed")?;
            let mut choices = (address, port).to_socket_addrs().map_err(|_| "failed")?;
            // Prefer an IPv4 endpoint while preserving IPv6-only profiles.
            let first = choices.next().ok_or("failed")?;
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
            .ok_or("failed")?;
        let routes = linux::routes(endpoint);
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut input, &mut std::io::sink());
            let _ = send.send(());
        });
        let mut command = Command::new(CORE);
        command
            .args(["run", "-c", "stdin:", "-format", "json"])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
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
        let mut child = Core(command.spawn().map_err(|_| "failed")?);
        let payload = linux::config(outbound, &routes).to_string();
        child
            .0
            .stdin
            .take()
            .ok_or("failed")?
            .write_all(payload.as_bytes())
            .map_err(|_| "failed")?;
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            if child.0.try_wait().map_err(|_| "failed")?.is_some() || Instant::now() >= deadline {
                return Err("failed");
            }
            if unsafe { libc::if_nametoindex(c"tangledlink0".as_ptr()) } != 0
                && std::net::UdpSocket::bind(("172.30.254.1", 0)).is_ok()
                && routes_ready(routes.last().ok_or("failed")?)
            {
                break;
            }
            if receive.recv_timeout(Duration::from_millis(75)).is_ok() {
                return Ok(());
            }
        }
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

    fn routes_ready(last_route: &str) -> bool {
        Command::new("/usr/bin/ip")
            .args([
                "-6",
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
