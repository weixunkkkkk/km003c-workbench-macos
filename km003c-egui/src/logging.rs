//! Tracing output goes to `logs/session.log` in the workbench's storage
//! directory (or under `KM003C_STORAGE_ROOT` when that is set).
//!
//! A GUI app has no visible stderr, so panics are logged there as well, with
//! the thread, location and backtrace: a panicking USB or writer thread used
//! to leave no trace. The log rotates at startup once it passes
//! [`MAX_LOG_BYTES`], keeping one previous file, so it cannot grow without
//! bound over months of sessions.

use std::path::{Path, PathBuf};

/// Size above which `session.log` moves to `session.log.1` at startup.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

pub(crate) fn init(runtime_app_id: &str) {
    install_panic_hook();
    let log_dir = std::env::var_os("KM003C_STORAGE_ROOT")
        .map(PathBuf::from)
        .or_else(|| eframe::storage_dir(runtime_app_id))
        .map(|path| path.join("logs"));
    if let Some(log_dir) = log_dir
        && std::fs::create_dir_all(&log_dir).is_ok()
    {
        let log_path = log_dir.join("session.log");
        let _ = rotate_if_larger_than(&log_path, MAX_LOG_BYTES);
        if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            tracing_subscriber::fmt().with_writer(file).with_ansi(false).init();
            return;
        }
    }
    tracing_subscriber::fmt().with_ansi(false).init();
}

/// Moves `path` to `<path>.1`, replacing an older copy, once it exceeds
/// `max_bytes`. Returns whether it rotated.
fn rotate_if_larger_than(path: &Path, max_bytes: u64) -> std::io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > max_bytes => {
            let mut rotated = path.as_os_str().to_owned();
            rotated.push(".1");
            std::fs::rename(path, rotated)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Logs a panic with its thread and backtrace, then runs the default hook.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(thread = thread.name().unwrap_or("unnamed"), "{info}\n{backtrace}");
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("km003c-logging-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_large_log_is_rotated_once_and_a_small_one_is_kept() {
        let dir = scratch_dir("rotate");
        let log = dir.join("session.log");
        let rotated = dir.join("session.log.1");

        assert!(
            !rotate_if_larger_than(&log, 8).unwrap(),
            "a missing log is not an error"
        );

        std::fs::write(&log, b"0123456789").unwrap();
        assert!(rotate_if_larger_than(&log, 8).unwrap());
        assert!(!log.exists());
        assert_eq!(std::fs::read(&rotated).unwrap(), b"0123456789");

        std::fs::write(&log, b"0123").unwrap();
        assert!(!rotate_if_larger_than(&log, 8).unwrap());
        assert_eq!(std::fs::read(&log).unwrap(), b"0123");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
