use tangledlink_desktop::windows_tun_config as config;

pub fn run() -> Result<(), &'static str> {
    use std::path::Path;

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [mode, executable, subscription, index, seconds] if mode == "trial" => {
            #[cfg(windows)]
            return trial(executable, subscription, index, seconds);
            #[cfg(not(windows))]
            {
                let _ = (executable, subscription, index, seconds);
                Err("Trial mode requires Windows")
            }
        }
        [mode, executable, subscription] if mode == "check-subscription" => {
            let body =
                std::fs::read_to_string(subscription).map_err(|_| "Cannot read subscription")?;
            let profiles = tangledlink_desktop::subscription_profiles(&body)?;
            let count = profiles.len();
            for profile in profiles {
                check_profile(Path::new(executable), config(profile.outbound))?;
            }
            println!("Xray accepted {count} profiles.");
            Ok(())
        }
        [mode, executable, profile] if mode == "check" || mode == "connect" => {
            let profile =
                std::fs::read_to_string(profile).map_err(|_| "Cannot read profile file")?;
            let outbound =
                tangledlink_desktop::reality_outbound(profile.trim_start_matches('\u{feff}'))?;
            if mode == "check" {
                check_profile(Path::new(executable), config(outbound))?;
                println!("Xray accepted the configuration.");
                return Ok(());
            }
            #[cfg(windows)]
            return connect(Path::new(executable), outbound);
            #[cfg(not(windows))]
            Err("Connect mode requires Windows")
        }
        _ => Err(
            "Usage: tangledlink-desktop <check|connect|check-subscription> <xray> <file>, or trial <xray> <subscription> <index> <seconds>",
        ),
    }
}

#[cfg(windows)]
fn connect(executable: &std::path::Path, outbound: serde_json::Value) -> Result<(), &'static str> {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed))
        .map_err(|_| "Cannot install Ctrl+C handler")?;
    let mut tunnel = tangledlink_desktop::engine::Tunnel::start(executable, outbound)?;
    eprintln!("TUN ready. Press Ctrl+C to disconnect.");
    while !stopping.load(Ordering::Relaxed) {
        if !tunnel.running()? {
            return Err("Xray stopped unexpectedly");
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    tunnel.stop()
}

fn check_profile(
    executable: &std::path::Path,
    config: serde_json::Value,
) -> Result<(), &'static str> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let executable = executable
        .canonicalize()
        .map_err(|_| "Cannot locate Xray")?;
    let mut child = Command::new(&executable)
        .current_dir(executable.parent().ok_or("Invalid Xray path")?)
        .args(["run", "-test", "-c", "stdin:", "-format", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Cannot launch Xray")?;
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(config.to_string().as_bytes()).is_ok());
    let status = child.wait().map_err(|_| "Cannot wait for Xray")?;
    if written && status.success() {
        Ok(())
    } else {
        Err("Xray rejected a profile configuration")
    }
}

#[cfg(windows)]
fn trial(
    executable: &std::ffi::OsStr,
    subscription: &std::ffi::OsStr,
    index: &std::ffi::OsStr,
    seconds: &std::ffi::OsStr,
) -> Result<(), &'static str> {
    let index: usize = index
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or("Invalid profile index")?;
    let seconds: u64 = seconds
        .to_str()
        .and_then(|s| s.parse().ok())
        .filter(|s| (1..=60).contains(s))
        .ok_or("Trial must be 1–60 seconds")?;
    let body = std::fs::read_to_string(subscription).map_err(|_| "Cannot read subscription")?;
    let profile = tangledlink_desktop::subscription_profiles(&body)?
        .into_iter()
        .nth(index)
        .ok_or("Unknown profile")?;
    let mut tunnel = tangledlink_desktop::engine::Tunnel::start(
        std::path::Path::new(executable),
        profile.outbound,
    )?;
    println!("TUN ready. Testing external HTTP and Windows DNS.");
    let during = public_ip();
    let secondary = std::process::Command::new("curl.exe")
        .args([
            "--noproxy",
            "*",
            "-sS",
            "-f",
            "--max-time",
            "8",
            "-o",
            "NUL",
            "-w",
            "%{http_code}\\n",
            "https://www.microsoft.com/",
        ])
        .status()
        .map_err(|_| "Cannot run curl.exe")?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    tunnel.stop()?;
    println!("TUN stopped gracefully.");
    let after = public_ip()?;
    let during = during?;
    println!("Public exit IP changed: {}", during != after);
    if !secondary.success() || during == after {
        return Err("A network verification probe did not pass");
    }
    Ok(())
}

#[cfg(windows)]
fn public_ip() -> Result<std::net::IpAddr, &'static str> {
    let output = std::process::Command::new("curl.exe")
        .args([
            "--noproxy",
            "*",
            "-fsS",
            "--max-time",
            "8",
            "https://api.ipify.org",
        ])
        .output()
        .map_err(|_| "Cannot run public IP probe")?;
    if !output.status.success() {
        return Err("Public IP probe failed");
    }
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .ok_or("Invalid response from public IP probe")
}

#[cfg(test)]
mod tests {
    use super::config;
    use tangledlink_desktop::reality_outbound;

    #[test]
    fn reality_config_routes_both_address_families() {
        let profile = "vless://12345678-1234-4123-8123-123456789abc@[2001:db8::1]:443?type=tcp&security=reality&sni=example.com&pbk=test&sid=12&flow=xtls-rprx-vision&spx=%2Fa";
        let result = config(reality_outbound(profile).unwrap());
        let outbound = &result["outbounds"][0];
        assert_eq!(outbound["settings"]["vnext"][0]["address"], "2001:db8::1");
        assert_eq!(
            outbound["streamSettings"]["realitySettings"]["spiderX"],
            "/a"
        );
        assert_eq!(
            result["inbounds"][0]["settings"]["autoSystemRoutingTable"],
            serde_json::json!(["0.0.0.0/0", "::/0"])
        );
    }

    #[test]
    fn rejects_transport_options_it_cannot_preserve() {
        let profile =
            "vless://user@example.com:443?security=reality&pbk=test&sni=example.com&type=ws";
        assert!(reality_outbound(profile).is_err());
    }
}
