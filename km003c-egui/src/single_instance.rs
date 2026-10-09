//! One workbench per bundle identifier.
//!
//! The first instance holds an exclusive `flock` on a lock file in the
//! temporary directory. A second launch appends a byte to an activation file
//! and exits; the first instance sees the file grow on its next logic pass
//! and brings its window forward.

#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct SingleInstanceGuard {
    #[cfg(unix)]
    lock_file: File,
    #[cfg(unix)]
    lock_path: PathBuf,
    #[cfg(unix)]
    activation_path: PathBuf,
    #[cfg(unix)]
    activation_cursor: AtomicU64,
}

impl SingleInstanceGuard {
    #[cfg(unix)]
    pub(crate) fn acquire(app_id: &str) -> io::Result<Option<Self>> {
        let (lock_path, activation_path) = instance_lock_paths(app_id);
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;

        // SAFETY: `lock_file` owns a valid descriptor for the duration of the
        // call. `flock` neither retains the pointer nor accesses Rust memory.
        let lock_result = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if lock_result == 0 {
            std::fs::write(&activation_path, [])?;
            return Ok(Some(Self {
                lock_file,
                lock_path,
                activation_path,
                activation_cursor: AtomicU64::new(0),
            }));
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }

        // The secondary process only appends an activation byte, then exits.
        // The primary polls the file length from its normal egui frame loop.
        let mut activation_file = OpenOptions::new().create(true).append(true).open(&activation_path)?;
        activation_file.write_all(b"1")?;
        activation_file.sync_data()?;
        Ok(None)
    }

    #[cfg(unix)]
    pub(crate) fn activation_requested(&self) -> bool {
        let length = std::fs::metadata(&self.activation_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let previous = self.activation_cursor.swap(length, Ordering::AcqRel);
        length > previous
    }

    #[cfg(not(unix))]
    pub(crate) fn acquire(_app_id: &str) -> io::Result<Option<Self>> {
        Ok(Some(Self {}))
    }

    #[cfg(not(unix))]
    pub(crate) const fn activation_requested(&self) -> bool {
        false
    }
}

#[cfg(unix)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the descriptor remains valid until this struct is dropped.
        let _ = unsafe { libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN) };
        let _ = std::fs::remove_file(&self.activation_path);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn instance_lock_paths(app_id: &str) -> (PathBuf, PathBuf) {
    // Keep production, demo and test instances isolated without exposing the
    // full bundle identifier in temporary filenames.
    let hash = app_id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let root = std::env::temp_dir();
    (
        root.join(format!("km003c-{hash:016x}.lock")),
        root.join(format!("km003c-{hash:016x}.activate")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn second_instance_notifies_the_primary_without_taking_the_lock() {
        let unique_id = format!(
            "{}.test.{}.{}",
            crate::i18n::APP_ID,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let (lock_path, activation_path) = instance_lock_paths(&unique_id);
        let primary = SingleInstanceGuard::acquire(&unique_id).unwrap().unwrap();
        assert!(SingleInstanceGuard::acquire(&unique_id).unwrap().is_none());
        assert!(primary.activation_requested());
        drop(primary);
        assert!(!lock_path.exists());
        assert!(!activation_path.exists());
    }
}
