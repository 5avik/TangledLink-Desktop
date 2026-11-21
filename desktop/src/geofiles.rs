use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, UNIX_EPOCH},
};
use tangledlink_desktop::linux::{CORE, geo};
use tauri::{AppHandle, Manager};

const SOURCES: [&str; 3] = [
    "Loyalsoldier/v2ray-rules-dat",
    "runetfreedom/russia-v2ray-rules-dat",
    "Chocolate4U/Iran-v2ray-rules",
];
const READ_ERROR: &str = "Не удалось открыть обновлённые геофайлы";
const WRITE_ERROR: &str = "Не удалось сохранить геофайлы";
const DOWNLOAD_ERROR: &str = "Не удалось загрузить геофайлы";
const INVALID_FILES: &str = "Xray не смог прочитать скачанные геофайлы";

pub fn bundle_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|_| "Папка настроек недоступна")?
        .join("geofiles.bundle"))
}

fn metadata(path: &Path) -> Option<(u64, Option<u64>)> {
    let file = fs::metadata(path).ok()?;
    Some((
        file.len(),
        file.modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs()),
    ))
}

pub fn status(path: &Path) -> Result<Value, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(json!({"custom": false, "files": geo::NAMES.map(|name| {
                let info = metadata(&Path::new(geo::PACKAGED).join(name));
                json!({"size": info.map(|(size, _)| size), "modified": info.and_then(|(_, time)| time)})
            })}));
        }
        Err(_) => return Err(READ_ERROR.into()),
    };
    let sizes = geo::sizes(&mut file).map_err(|_| READ_ERROR)?;
    let metadata = file.metadata().map_err(|_| READ_ERROR)?;
    if metadata.len() != (8 + sizes.iter().sum::<usize>()) as u64 {
        return Err(READ_ERROR.into());
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    Ok(
        json!({"custom": true, "files": sizes.map(|size| json!({"size": size, "modified": modified}))}),
    )
}

pub fn update(path: &Path, source: usize) -> Result<Value, String> {
    let source = SOURCES
        .get(source)
        .ok_or("Неизвестный источник геофайлов")?;
    if !Path::new(CORE).is_file() {
        return Err("Установи пакет TangledLink с ядром Xray".into());
    }
    let directory = path.parent().ok_or(WRITE_ERROR)?;
    fs::create_dir_all(directory).map_err(|_| WRITE_ERROR)?;
    let mut candidate = tempfile::NamedTempFile::new_in(directory).map_err(|_| WRITE_ERROR)?;
    candidate.write_all(&[0; 8]).map_err(|_| WRITE_ERROR)?;

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(60)))
            .build(),
    );
    let mut sizes = [0u32; 2];
    for (name, size) in geo::NAMES.into_iter().zip(&mut sizes) {
        let url = format!("https://github.com/{source}/releases/latest/download/{name}");
        let mut response = agent.get(url).call().map_err(|_| DOWNLOAD_ERROR)?;
        let mut body = response
            .body_mut()
            .as_reader()
            .take((geo::MAX_FILE_SIZE + 1) as u64);
        let bytes = io::copy(&mut body, &mut candidate).map_err(|_| DOWNLOAD_ERROR)?;
        if bytes == 0 || bytes > geo::MAX_FILE_SIZE as u64 {
            return Err(INVALID_FILES.into());
        }
        *size = bytes as u32;
    }

    candidate.rewind().map_err(|_| WRITE_ERROR)?;
    for size in sizes {
        candidate
            .write_all(&size.to_le_bytes())
            .map_err(|_| WRITE_ERROR)?;
    }
    candidate.rewind().map_err(|_| WRITE_ERROR)?;
    let assets = geo::Assets::from_bundle(&mut candidate, directory).map_err(|_| WRITE_ERROR)?;
    validate(CORE.as_ref(), assets.path())?;
    candidate.as_file().sync_all().map_err(|_| WRITE_ERROR)?;
    candidate.persist(path).map_err(|_| WRITE_ERROR)?;
    status(path)
}

fn validate(core: &Path, assets: &Path) -> Result<(), String> {
    // Loading both groups makes Xray decode both databases before they replace the current pair.
    let config = json!({
        "outbounds": [{"tag": "direct", "protocol": "freedom"}],
        "routing": {"rules": [
            {"ip": ["geoip:private"], "outboundTag": "direct"},
            {"domain": ["geosite:cn"], "outboundTag": "direct"}
        ]}
    });
    let mut child = Command::new(core)
        .args(["run", "-test", "-c", "stdin:", "-format", "json"])
        .env("XRAY_LOCATION_ASSET", assets)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| INVALID_FILES)?;
    let written = child
        .stdin
        .take()
        .ok_or(INVALID_FILES)?
        .write_all(config.to_string().as_bytes());
    let result = child.wait().map_err(|_| INVALID_FILES)?;
    if written.is_err() || !result.success() {
        return Err(INVALID_FILES.into());
    }
    Ok(())
}

pub fn reset(path: &Path) -> Result<Value, String> {
    match fs::remove_file(path) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(_) => return Err(WRITE_ERROR.into()),
    }
    status(path)
}
