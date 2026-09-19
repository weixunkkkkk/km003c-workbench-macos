//! Save an imported file unchanged; never re-integrate or rebase its samples.
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use crate::recording_session::{RecordingSessionMetadataV1, sidecar_path};

pub(crate) fn start(
    source: PathBuf,
    destination: PathBuf,
    metadata: Option<RecordingSessionMetadataV1>,
) -> Receiver<Result<PathBuf, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = copy_recording(&source, &destination, metadata.as_ref())
            .map(|()| destination)
            .map_err(|error| error.to_string());
        let _ = tx.send(result);
    });
    rx
}

fn copy_recording(
    source: &Path,
    destination: &Path,
    metadata: Option<&RecordingSessionMetadataV1>,
) -> std::io::Result<()> {
    // create_new protects both the source and any existing recording/sidecar.
    let sidecar = sidecar_path(destination);
    if sidecar.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Metadata already exists; choose a new filename",
        ));
    }
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(destination)?;
    let mut created_sidecar = false;
    let result = (|| {
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        if let Some(metadata) = metadata {
            let file = OpenOptions::new().write(true).create_new(true).open(&sidecar)?;
            created_sidecar = true;
            serde_json::to_writer_pretty(&file, metadata)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        drop(output);
        let _ = std::fs::remove_file(destination);
        if created_sidecar {
            let _ = std::fs::remove_file(sidecar);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_copy_preserves_bytes_metadata_and_existing_files() {
        let directory = std::env::temp_dir().join(format!(
            "km003c-copy-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("source.csv");
        let destination = directory.join("copy.csv");
        let bytes = b"elapsed_us,sample_index\n0,1\n";
        std::fs::write(&source, bytes).unwrap();
        let metadata = RecordingSessionMetadataV1::default();
        copy_recording(&source, &destination, Some(&metadata)).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), bytes);
        assert_eq!(
            crate::recording_session::read_sidecar(&destination).unwrap(),
            Some(metadata)
        );
        assert!(copy_recording(&source, &destination, None).is_err());
        assert!(copy_recording(&source, &source, None).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
