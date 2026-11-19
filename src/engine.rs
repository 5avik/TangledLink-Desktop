use crate::windows_tun_config;
use serde_json::Value;
use std::{
    io::Write,
    net::{IpAddr, ToSocketAddrs},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    System::Console::{
        AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleProcessList,
        GetConsoleWindow,
    },
    UI::WindowsAndMessaging::{SW_HIDE, ShowWindow},
};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub struct Tunnel {
    child: Child,
}

impl Tunnel {
    pub fn start(executable: &Path, mut outbound: Value) -> Result<Self, &'static str> {
        resolve_endpoint(&mut outbound)?;
        let mut console_process = 0;
        // An isolated console lets us signal the Xray group without killing the app.
        unsafe {
            if GetConsoleProcessList(&mut console_process, 1) == 0 {
                if AllocConsole() == 0 {
                    return Err("Cannot allocate a control console");
                }
                let window = GetConsoleWindow();
                if !window.is_null() {
                    ShowWindow(window, SW_HIDE);
                }
            }
        }
        let executable = executable
            .canonicalize()
            .map_err(|_| "Xray executable is missing")?;
        let mut child = Command::new(&executable)
            .current_dir(executable.parent().ok_or("Invalid Xray path")?)
            .args(["run", "-c", "stdin:", "-format", "json"])
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "Cannot launch Xray")?;
        let payload = windows_tun_config(outbound).to_string();
        let written = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(payload.as_bytes()).is_ok());
        let mut tunnel = Self { child };
        if !written {
            let _ = tunnel.stop();
            return Err("Cannot send config to Xray");
        }
        let mut adapter_ready = false;
        for _ in 0..20 {
            if !tunnel.running()? {
                return Err("Xray exited during startup. Check administrator rights and Wintun.");
            }
            if tunnel_adapter_ready() {
                adapter_ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        if !adapter_ready {
            let _ = tunnel.stop();
            return Err("The TangledLink network adapter did not start");
        }
        Ok(tunnel)
    }

    pub fn running(&mut self) -> Result<bool, &'static str> {
        self.child
            .try_wait()
            .map(|state| state.is_none())
            .map_err(|_| "Cannot inspect Xray process")
    }

    pub fn stop(&mut self) -> Result<(), &'static str> {
        if !self.running()? {
            return Ok(());
        }
        let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.child.id()) };
        if sent != 0 {
            let deadline = Instant::now() + Duration::from_secs(6);
            while Instant::now() < deadline {
                if !self.running()? {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        self.child.kill().map_err(|_| "Cannot stop Xray")?;
        self.child.wait().map_err(|_| "Cannot reap Xray")?;
        Err("Xray required a forced stop. Verify Windows network settings.")
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn tunnel_adapter_ready() -> bool {
    // Xray reports readiness through the OS adapter after assigning the configured address.
    std::net::UdpSocket::bind(("172.30.254.1", 0)).is_ok()
}

fn resolve_endpoint(outbound: &mut Value) -> Result<(), &'static str> {
    let server = &mut outbound["settings"]["vnext"][0];
    let host = server["address"]
        .as_str()
        .ok_or("Missing VPN server address")?;
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let port = server["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or("Missing VPN server port")?;
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| "Cannot resolve VPN server before opening the tunnel")?
        .collect();
    let endpoint = addresses
        .iter()
        .find(|addr| addr.is_ipv4())
        .or_else(|| addresses.first())
        .ok_or("VPN server has no network address")?;
    server["address"] = serde_json::json!(endpoint.ip().to_string());
    Ok(())
}
