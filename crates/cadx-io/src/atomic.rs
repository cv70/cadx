use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct AtomicWriteError {
    pub path: PathBuf,
    pub source: std::io::Error,
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = parent_directory(path);
    let mut temporary = tempfile::Builder::new()
        .prefix(".cadx-write-")
        .tempfile_in(parent)
        .map_err(|source| AtomicWriteError {
            path: parent.to_owned(),
            source,
        })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| AtomicWriteError {
            path: temporary.path().to_owned(),
            source,
        })?;
    temporary.persist(path).map_err(|error| AtomicWriteError {
        path: path.to_owned(),
        source: error.error,
    })?;
    sync_parent(parent).map_err(|source| AtomicWriteError {
        path: parent.to_owned(),
        source,
    })
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> std::io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn atomically_replaces_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("model.cadx");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"second");
    }

    #[test]
    fn failed_persist_removes_the_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("destination");
        fs::create_dir(&destination).unwrap();
        assert!(write_atomic(&destination, b"cannot replace a directory").is_err());
        let residual = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".cadx-write-")
            });
        assert!(!residual);
    }
}
