use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const NAMES: [&str; 2] = ["geoip.dat", "geosite.dat"];
pub const PACKAGED: &str = "/usr/lib/tangledlink-desktop";
pub const MAX_FILE_SIZE: usize = 128 * 1024 * 1024;

pub fn sizes(mut input: impl Read) -> io::Result<[usize; 2]> {
    let mut sizes = [0; 2];
    for size in &mut sizes {
        let mut bytes = [0; 4];
        input.read_exact(&mut bytes)?;
        *size = u32::from_le_bytes(bytes) as usize;
        if *size == 0 || *size > MAX_FILE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid geofile size",
            ));
        }
    }
    Ok(sizes)
}

pub struct Assets(PathBuf);

impl Assets {
    pub fn from_bundle(mut input: impl Read, parent: &Path) -> io::Result<Self> {
        use std::{ffi::CStr, os::unix::ffi::OsStrExt};
        let sizes = sizes(&mut input)?;
        let mut template = parent
            .join("tangledlink-geo-XXXXXX")
            .into_os_string()
            .into_encoded_bytes();
        template.push(0);
        let path = unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) };
        if path.is_null() {
            return Err(io::Error::last_os_error());
        }
        let directory = Self(PathBuf::from(std::ffi::OsStr::from_bytes(
            unsafe { CStr::from_ptr(path) }.to_bytes(),
        )));
        for (name, size) in NAMES.into_iter().zip(sizes) {
            let mut file = File::create(directory.0.join(name))?;
            if io::copy(&mut (&mut input).take(size as u64), &mut file)? != size as u64 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            file.flush()?;
        }
        Ok(directory)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Assets {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_exactly_two_files_and_leaves_the_remaining_stream_open() {
        let mut input = &b"\x03\0\0\0\x02\0\0\0abcdeextra"[..];
        let assets = Assets::from_bundle(&mut input, &std::env::temp_dir()).unwrap();
        assert_eq!(fs::read(assets.path().join(NAMES[0])).unwrap(), b"abc");
        assert_eq!(fs::read(assets.path().join(NAMES[1])).unwrap(), b"de");
        assert_eq!(input, b"extra");
        let path = assets.path().to_owned();
        drop(assets);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_empty_oversized_and_truncated_files() {
        for input in [
            &b"\0\0\0\0\x02\0\0\0"[..],
            &b"\xff\xff\xff\xff\x02\0\0\0"[..],
            &b"\x03\0\0\0\x02\0\0\0abcd"[..],
        ] {
            assert!(Assets::from_bundle(input, &std::env::temp_dir()).is_err());
        }
    }
}
