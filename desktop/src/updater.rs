use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

const REPO: &str = "5avik/TangledLink-Desktop";
const APP: &str = "/usr/bin/tangledlink-desktop";
const INSTALLER: &str = "/usr/lib/tangledlink-desktop/tangledlink-update-helper";
const MAX_SIZE: u64 = 256 * 1024 * 1024;
const CHECK_FAILED: &str = "Не удалось проверить обновления.";
const DOWNLOAD_FAILED: &str = "Не удалось скачать обновление.";
const INVALID_PACKAGE: &str = "Проверка обновления не пройдена.";
const INSTALL_FAILED: &str = "Не удалось установить обновление.";
const NO_UPDATE: &str = "Подходящее обновление недоступно.";

#[derive(Clone, Copy)]
enum Format {
    Arch,
    Deb,
    Rpm,
}

impl Format {
    fn installed() -> Option<Self> {
        if !cfg!(target_arch = "x86_64")
            || std::env::current_exe().ok()?.as_path() != Path::new(APP)
        {
            return None;
        }
        [
            (
                Self::Arch,
                "/usr/bin/pacman",
                &["-Q", "tangledlink-desktop"][..],
            ),
            (
                Self::Deb,
                "/usr/bin/dpkg-query",
                &["-W", "-f=${db:Status-Status}", "tangledlink-desktop"][..],
            ),
            (
                Self::Rpm,
                "/usr/bin/rpm",
                &["-q", "tangledlink-desktop"][..],
            ),
        ]
        .into_iter()
        .find_map(|(format, binary, args)| {
            let result = Command::new(binary)
                .args(args)
                .stderr(Stdio::null())
                .output()
                .ok()?;
            (result.status.success()
                && (!matches!(format, Self::Deb) || result.stdout == b"installed")
                && (!matches!(format, Self::Rpm) || Path::new("/usr/bin/dnf").is_file()))
            .then_some(format)
        })
    }

    fn filename(self, v: &Version) -> String {
        match self {
            Self::Arch => format!("tangledlink-desktop-{v}-1-x86_64.pkg.tar.zst"),
            Self::Deb => format!("tangledlink-desktop_{v}-1_amd64.deb"),
            Self::Rpm => format!("tangledlink-desktop-{v}-1.x86_64.rpm"),
        }
    }

    fn install(self, path: &Path) -> Result<(), &'static str> {
        let (binary, args): (_, &[_]) = match self {
            Self::Arch => ("/usr/bin/pacman", &["-U", "--noconfirm", "--"]),
            Self::Deb => (
                "/usr/bin/apt-get",
                &["--yes", "--no-remove", "install", "--"],
            ),
            Self::Rpm => ("/usr/bin/dnf", &["--assumeyes", "install", "--"]),
        };
        let status = Command::new(binary)
            .args(args)
            .arg(path)
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .map_err(|_| INSTALL_FAILED)?;
        status.success().then_some(()).ok_or(INSTALL_FAILED)
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    size: u64,
}

#[derive(Serialize)]
pub struct Update {
    version: Version,
    notes: String,
    #[serde(skip)]
    tag: String,
    #[serde(skip)]
    size: u64,
    #[serde(skip)]
    digest: String,
    #[serde(skip)]
    filename: String,
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(seconds)))
            .build(),
    )
}

fn select(release: Release, format: Format, current: &Version) -> Option<Update> {
    if release.draft || release.prerelease {
        return None;
    }
    let v = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )
    .ok()?;
    if !v.pre.is_empty() || !v.build.is_empty() || &v <= current {
        return None;
    }
    let filename = format.filename(&v);
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == filename)?;
    let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
    if digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || asset.size == 0
        || asset.size > MAX_SIZE
    {
        return None;
    }
    Some(Update {
        version: v,
        notes: release.body.unwrap_or_default(),
        tag: release.tag_name,
        size: asset.size,
        digest: digest.to_ascii_lowercase(),
        filename,
    })
}

fn latest(format: Format) -> Result<Option<Update>, &'static str> {
    let mut response = match agent(15)
        .get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "TangledLink-Desktop")
        .call()
    {
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Ok(response) => response,
        Err(_) => return Err(CHECK_FAILED),
    };
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|_| CHECK_FAILED)?;
    let release = serde_json::from_slice(&body).map_err(|_| CHECK_FAILED)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| CHECK_FAILED)?;
    Ok(select(release, format, &current))
}

pub fn check() -> Result<Option<Update>, &'static str> {
    if !Path::new(INSTALLER).is_file() {
        return Ok(None);
    }
    match Format::installed() {
        Some(format) => latest(format),
        None => Ok(None),
    }
}

fn copy_verified(
    input: impl Read,
    output: &mut File,
    update: &Update,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), &'static str> {
    let mut input = input.take(update.size);
    let mut hash = Sha256::new();
    let mut count = 0;
    let mut buffer = [0u8; 65536];
    let mut last_percent = 0;
    loop {
        let n = input.read(&mut buffer).map_err(|_| DOWNLOAD_FAILED)?;
        if n == 0 {
            break;
        }
        output
            .write_all(&buffer[..n])
            .map_err(|_| DOWNLOAD_FAILED)?;
        hash.update(&buffer[..n]);
        count += n as u64;
        let percent = count * 100 / update.size;
        if percent != last_percent {
            last_percent = percent;
            progress(count, update.size);
        }
    }
    if count != update.size || format!("{:x}", hash.finalize()) != update.digest {
        return Err(INVALID_PACKAGE);
    }
    Ok(())
}

pub fn download(version: &str, progress: impl FnMut(u64, u64)) -> Result<File, &'static str> {
    let update = check()?
        .filter(|candidate| candidate.version.to_string() == version)
        .ok_or(NO_UPDATE)?;
    let url = format!(
        "https://github.com/{REPO}/releases/download/{}/{}",
        update.tag, update.filename
    );
    let mut response = agent(180).get(url).call().map_err(|_| DOWNLOAD_FAILED)?;
    let mut file = tempfile::tempfile().map_err(|_| DOWNLOAD_FAILED)?;
    let mut body = response.body_mut().as_reader();
    copy_verified(&mut body, &mut file, &update, progress)?;
    if body.read(&mut [0]).map_err(|_| DOWNLOAD_FAILED)? != 0 {
        return Err(INVALID_PACKAGE);
    }
    file.rewind().map_err(|_| DOWNLOAD_FAILED)?;
    Ok(file)
}

pub fn install(
    mut file: File,
    version: &str,
    before_install: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut helper = Command::new("/usr/bin/pkexec")
        .args(["--disable-internal-agent", INSTALLER, version])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|_| INSTALL_FAILED)?;
    let transfer: Result<(), String> = (|| {
        let mut input = helper.stdin.take().ok_or(INSTALL_FAILED)?;
        let output = helper.stdout.take().ok_or(INSTALL_FAILED)?;
        io::copy(&mut file, &mut input).map_err(|_| INSTALL_FAILED)?;
        let mut reply = String::new();
        BufReader::new(output)
            .read_line(&mut reply)
            .map_err(|_| INSTALL_FAILED)?;
        if reply != "verified\n" {
            return Err(INSTALL_FAILED.into());
        }
        before_install()?;
        input
            .write_all(b"install\n")
            .map_err(|_| INSTALL_FAILED.into())
    })();
    let status = helper.wait().map_err(|_| INSTALL_FAILED)?;
    match status.code() {
        Some(126 | 127) => Err("Установка отменена или не подтверждена.".into()),
        _ => {
            transfer?;
            status
                .success()
                .then_some(())
                .ok_or_else(|| INSTALL_FAILED.into())
        }
    }
}

pub fn install_privileged(version: &str) -> Result<(), &'static str> {
    let format = Format::installed().ok_or(NO_UPDATE)?;
    let update = latest(format)?
        .filter(|candidate| candidate.version.to_string() == version)
        .ok_or(NO_UPDATE)?;
    let directory = tempfile::tempdir_in("/var/tmp").map_err(|_| INSTALL_FAILED)?;
    let path = directory.path().join(&update.filename);
    let mut file = File::create(&path).map_err(|_| INSTALL_FAILED)?;
    let mut input = io::stdin().lock();
    copy_verified(&mut input, &mut file, &update, |_, _| {})?;
    drop(file);
    {
        let mut output = io::stdout().lock();
        writeln!(output, "verified").map_err(|_| INSTALL_FAILED)?;
        output.flush().map_err(|_| INSTALL_FAILED)?;
    }
    let mut confirmation = [0; 8];
    input
        .read_exact(&mut confirmation)
        .map_err(|_| INSTALL_FAILED)?;
    if &confirmation != b"install\n" {
        return Err(INSTALL_FAILED);
    }
    format.install(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, format: Format, payload: &[u8]) -> Release {
        Release {
            tag_name: tag.into(),
            body: Some("Release notes".into()),
            draft: false,
            prerelease: false,
            assets: vec![Asset {
                name: format.filename(&Version::new(0, 2, 0)),
                digest: Some(format!("sha256:{:x}", Sha256::digest(payload))),
                size: payload.len() as u64,
            }],
        }
    }

    #[test]
    fn selects_only_a_newer_stable_release_with_a_verified_package_for_the_installed_format() {
        let current = Version::new(0, 1, 5);
        for format in [Format::Arch, Format::Deb, Format::Rpm] {
            let result = select(release("v0.2.0", format, b"test"), format, &current).unwrap();
            assert_eq!(result.version, Version::new(0, 2, 0));
            assert_eq!(result.notes, "Release notes");
        }
        let mut pre = release("v0.2.0", Format::Arch, b"test");
        pre.prerelease = true;
        assert!(select(pre, Format::Arch, &current).is_none());
        let mut unverified = release("v0.2.0", Format::Arch, b"test");
        unverified.assets[0].digest = None;
        assert!(select(unverified, Format::Arch, &current).is_none());
        assert!(
            select(
                release("v0.1.5", Format::Arch, b"test"),
                Format::Arch,
                &current
            )
            .is_none()
        );
        assert!(
            select(
                release("v0.2.0", Format::Rpm, b"test"),
                Format::Arch,
                &current
            )
            .is_none()
        );
    }

    #[test]
    fn package_bytes_must_match_the_released_size_and_hash() {
        let payload = b"original";
        let update = select(
            release("v0.2.0", Format::Deb, payload),
            Format::Deb,
            &Version::new(0, 1, 5),
        )
        .unwrap();
        for invalid in [&b"truncate"[..7], &b"tampered"[..]] {
            let result = copy_verified(
                invalid,
                &mut tempfile::tempfile().unwrap(),
                &update,
                |_, _| {},
            );
            assert_eq!(result, Err(INVALID_PACKAGE));
        }
        let mut output = tempfile::tempfile().unwrap();
        let mut progress = Vec::new();
        copy_verified(&payload[..], &mut output, &update, |received, size| {
            progress.push((received, size))
        })
        .unwrap();
        assert_eq!(progress, [(8, 8)]);
        assert_eq!(output.metadata().unwrap().len(), 8);
    }
}
