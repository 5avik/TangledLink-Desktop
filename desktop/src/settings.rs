#[cfg(windows)]
mod windows {
    use serde::{Serialize, de::DeserializeOwned};
    use std::{io::Write, path::Path, ptr};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };

    fn transform(bytes: &[u8], encrypt: bool) -> std::io::Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len()).map_err(std::io::Error::other)?,
            pbData: bytes.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        // DPAPI reads the input and allocates the output with LocalAlloc.
        unsafe {
            let ok = if encrypt {
                CryptProtectData(
                    &input,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            } else {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast());
            Ok(bytes)
        }
    }

    pub fn read<T: DeserializeOwned>(path: &Path) -> std::io::Result<Option<T>> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let decoded = transform(&bytes, false)?;
        serde_json::from_slice(&decoded)
            .map(Some)
            .map_err(std::io::Error::other)
    }

    pub fn write<T: Serialize>(path: &Path, state: &T) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(state).map_err(std::io::Error::other)?;
        let encrypted = transform(&bytes, true)?;
        let directory = path
            .parent()
            .ok_or_else(|| std::io::Error::other("Missing settings directory"))?;
        std::fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&encrypted)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(std::io::Error::other)?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Fixture {
            secret: String,
        }

        #[test]
        fn persists_and_replaces_encrypted_settings() {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("subscription.bin");
            let mut settings = Fixture {
                secret: "test-only-secret".into(),
            };
            assert!(read::<Fixture>(&path).unwrap().is_none());
            write(&path, &settings).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(settings.secret.len())
                    .any(|part| part == settings.secret.as_bytes())
            );
            settings.secret = "replacement-test-secret".into();
            write(&path, &settings).unwrap();
            let restored: Fixture = read(&path).unwrap().unwrap();
            assert_eq!(restored.secret, settings.secret);
        }
    }
}
#[cfg(windows)]
pub use windows::{read, write};

#[cfg(windows)]
pub fn remove(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(target_os = "linux")]
mod secret_service {
    use serde::{Serialize, de::DeserializeOwned};
    use std::{
        io::{self, Write},
        path::Path,
        process::{Command, Stdio},
    };

    fn command(action: &str, path: &Path) -> io::Result<Command> {
        let key = path
            .file_name()
            .ok_or_else(|| io::Error::other("Missing keyring item name"))?;
        let mut command = Command::new("/usr/bin/secret-tool");
        command.arg(action);
        if action == "store" {
            command.arg("--label=TangledLink Desktop");
        }
        command
            .args(["application", "pro.tngl.desktop", "item"])
            .arg(key);
        Ok(command)
    }

    pub fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
        let result = command("lookup", path)?.stdin(Stdio::null()).output()?;
        if result.status.success() {
            return serde_json::from_slice(&result.stdout)
                .map(Some)
                .map_err(io::Error::other);
        }
        if result.status.code() == Some(1) && result.stderr.is_empty() {
            return Ok(None);
        }
        Err(io::Error::other("Secret Service lookup failed"))
    }

    pub fn write<T: Serialize>(path: &Path, data: &T) -> io::Result<()> {
        let payload = serde_json::to_vec(data).map_err(io::Error::other)?;
        let mut child = command("store", path)?
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Missing keyring input"))
            .and_then(|mut input| input.write_all(&payload));
        let status = child.wait()?;
        written?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("Secret Service store failed"))
        }
    }

    pub fn remove(path: &Path) -> io::Result<()> {
        let result = command("clear", path)?.stdin(Stdio::null()).output()?;
        if result.status.success() || (result.status.code() == Some(1) && result.stderr.is_empty())
        {
            Ok(())
        } else {
            Err(io::Error::other("Secret Service clear failed"))
        }
    }
}

#[cfg(target_os = "linux")]
pub use secret_service::{read, remove, write};
