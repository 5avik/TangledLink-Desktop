use std::{
    io::{self, Write},
    process::{Command, Stdio},
};
use tangledlink_desktop::linux::{INTERFACE, split_cgroup};

const TABLE: &str = "19180";
const PRIORITY: &str = "19180";
const MARK: &str = "0x00400000";
const MASKED_MARK: &str = "0x00400000/0x00400000";
const NFT_TABLE: &str = "tangledlink_split";

pub struct Policy {
    routes: [bool; 2],
    rules: [bool; 2],
    nft: bool,
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.env_clear().stdin(Stdio::null()).stderr(Stdio::null());
    cmd
}

fn ip(family: &str, args: &[&str]) -> io::Result<()> {
    let result = command("/usr/bin/ip")
        .arg(family)
        .args(args)
        .stdout(Stdio::null())
        .status()?;
    if result.success() {
        Ok(())
    } else {
        Err(io::Error::other("ip failed"))
    }
}

fn nft(script: &str) -> io::Result<()> {
    let program = if std::path::Path::new("/usr/bin/nft").is_file() {
        "/usr/bin/nft"
    } else {
        "/usr/sbin/nft"
    };
    let mut child = command(program)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    let result = child.stdin.take().unwrap().write_all(script.as_bytes());
    let status = child.wait()?;
    result?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("nft failed"))
    }
}

impl Policy {
    pub fn install(uid: u32, ipv6: bool) -> io::Result<Self> {
        let path = split_cgroup(uid);
        if !std::path::Path::new("/sys/fs/cgroup").join(&path).is_dir() {
            return Err(io::Error::other("missing application cgroup"));
        }
        // Loose reverse-path checking on this device permits TUN replies to marked flows.
        std::fs::write(
            format!("/proc/sys/net/ipv4/conf/{INTERFACE}/rp_filter"),
            b"2",
        )?;
        let mut policy = Self {
            routes: [false; 2],
            rules: [false; 2],
            nft: false,
        };
        let family_count = if ipv6 { 2 } else { 1 };
        for (i, family) in ["-4", "-6"].into_iter().take(family_count).enumerate() {
            let existing = command("/usr/bin/ip")
                .args([family, "rule", "show", "pref", PRIORITY])
                .output()?;
            if !existing.status.success() || !existing.stdout.is_empty() {
                return Err(io::Error::other("routing rule priority is in use"));
            }
            let existing = command("/usr/bin/ip")
                .args([family, "route", "show", "table", TABLE])
                .output()?;
            if !existing.stdout.is_empty() {
                return Err(io::Error::other("routing table is in use"));
            }
            ip(
                family,
                &["route", "add", "default", "dev", INTERFACE, "table", TABLE],
            )?;
            policy.routes[i] = true;
            ip(
                family,
                &[
                    "rule",
                    "add",
                    "pref",
                    PRIORITY,
                    "fwmark",
                    MASKED_MARK,
                    "lookup",
                    TABLE,
                ],
            )?;
            policy.rules[i] = true;
        }
        let mut script = String::new();
        for family in ["ip", "ip6"].into_iter().take(family_count) {
            script.push_str(&format!(
                "create table {family} {NFT_TABLE}\n\
                 add chain {family} {NFT_TABLE} output {{ type route hook output priority mangle; policy accept; }}\n\
                 add rule {family} {NFT_TABLE} output oifname != \"lo\" meta l4proto {{ tcp, udp }} socket cgroupv2 level 4 \"{path}\" meta mark set meta mark or {MARK}\n"
            ));
        }
        nft(&script)?;
        policy.nft = true;
        Ok(policy)
    }
}

impl Drop for Policy {
    fn drop(&mut self) {
        if self.nft {
            let mut script = format!("delete table ip {NFT_TABLE}\n");
            if self.rules[1] {
                script.push_str(&format!("delete table ip6 {NFT_TABLE}\n"));
            }
            let _ = nft(&script);
        }
        for (i, family) in ["-4", "-6"].into_iter().enumerate() {
            if self.rules[i] {
                let _ = ip(
                    family,
                    &[
                        "rule",
                        "del",
                        "pref",
                        PRIORITY,
                        "fwmark",
                        MASKED_MARK,
                        "lookup",
                        TABLE,
                    ],
                );
            }
            if self.routes[i] {
                let _ = ip(
                    family,
                    &["route", "del", "default", "dev", INTERFACE, "table", TABLE],
                );
            }
        }
    }
}
