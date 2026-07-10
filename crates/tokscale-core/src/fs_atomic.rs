use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;
#[cfg(unix)]
const PRIVATE_FILE_MODE: u32 = 0o600;
const TEMP_CREATE_ATTEMPTS: usize = 128;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Create a directory tree intended for private local state.
///
/// Unix directories are created with user-only access. Existing directory
/// permissions are repaired on a best-effort basis so a chmod failure does not
/// make otherwise-readable local state unusable.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(PRIVATE_DIR_MODE);
        builder.create(path)?;
    }

    #[cfg(not(unix))]
    fs::create_dir_all(path)?;

    repair_private_dir(path);
    Ok(())
}

/// Best-effort repair for an existing private directory.
pub fn repair_private_dir(path: &Path) {
    #[cfg(unix)]
    if fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        let _ = set_unix_mode(path, PRIVATE_DIR_MODE);
    }

    #[cfg(not(unix))]
    let _ = path;
}

/// Best-effort repair for an existing private regular file.
pub fn repair_private_file(path: &Path) {
    #[cfg(unix)]
    if fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        let _ = set_unix_mode(path, PRIVATE_FILE_MODE);
    }

    #[cfg(not(unix))]
    let _ = path;
}

/// Atomically replace a file with user-private contents.
///
/// The caller owns the parent directory policy. The temporary file is created
/// next to the destination with a collision-resistant name and `create_new`,
/// then flushed, synced, and replaced using the platform implementation below.
pub fn atomic_write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "private file path has no parent directory",
            )
        })?;
    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file path has no file name",
        ));
    }

    fs::create_dir_all(parent)?;
    let (temp_path, mut file) = create_unique_private_temp(parent)?;
    let mut cleanup = TempFileCleanup::new(temp_path);

    #[cfg(unix)]
    if let Err(error) = set_unix_mode(cleanup.path(), PRIVATE_FILE_MODE) {
        drop(file);
        return Err(error);
    }

    let write_result = (|| {
        file.write_all(data)?;
        file.flush()?;
        file.sync_all()
    })();
    drop(file);
    write_result?;

    replace_file(cleanup.path(), path)?;
    cleanup.disarm();
    sync_parent_directory(parent)
}

fn create_unique_private_temp(parent: &Path) -> io::Result<(PathBuf, File)> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);

    for _ in 0..TEMP_CREATE_ATTEMPTS {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".tokscale-atomic-{}-{timestamp:x}-{sequence:x}.tmp",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(PRIVATE_FILE_MODE);
        }

        match options.open(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique atomic-write temporary file",
    ))
}

#[cfg(unix)]
fn set_unix_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::metadata(path)?.permissions();
    if permissions.mode() & 0o7777 != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

struct TempFileCleanup {
    path: PathBuf,
    armed: bool,
}

impl TempFileCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn replace_file(tmp_path: &Path, final_path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        windows_replace_file(tmp_path, final_path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::fs::rename(tmp_path, final_path)
    }
}

#[cfg(target_os = "windows")]
fn windows_replace_file(tmp_path: &Path, final_path: &Path) -> io::Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    unsafe extern "system" {
        fn MoveFileExW(
            lp_existing_file_name: *const u16,
            lp_new_file_name: *const u16,
            dw_flags: u32,
        ) -> i32;
    }

    fn encode(path: &Path) -> Vec<u16> {
        OsStr::new(path.as_os_str())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let existing = encode(tmp_path);
    let new = encode(final_path);
    let result = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            new.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_private_replaces_existing_file_without_temp_leaks() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, b"old").unwrap();

        atomic_write_private(&path, b"new private state").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new private state");
        let entries = fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(entries, 1, "atomic write left a temporary file behind");
    }

    #[test]
    fn atomic_write_private_cleans_temp_when_replace_fails() {
        let temp = tempfile::TempDir::new().unwrap();
        let destination = temp.path().join("destination");
        fs::create_dir(&destination).unwrap();

        assert!(atomic_write_private(&destination, b"data").is_err());

        let entries = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            vec![destination.file_name().unwrap().to_os_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_helpers_repair_legacy_unix_modes() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join("private");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

        ensure_private_dir(&directory).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o7777,
            0o700
        );

        let file = directory.join("legacy.json");
        fs::write(&file, b"legacy").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        repair_private_file(&file);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_private_replaces_legacy_file_with_mode_0600() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("legacy.json");
        fs::write(&path, b"legacy").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let legacy_inode = fs::metadata(&path).unwrap().ino();

        atomic_write_private(&path, b"replacement").unwrap();

        let metadata = fs::metadata(&path).unwrap();
        assert_ne!(metadata.ino(), legacy_inode);
        assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    }
}
