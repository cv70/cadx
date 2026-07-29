use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::Path;

use crc32fast::Hasher;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::ProjectError;
use crate::project::{
    MANIFEST_ENTRY, MAX_ARCHIVE_ENTRIES, MAX_MANIFEST_BYTES, MAX_WORKSPACE_BYTES, ProjectManifest,
    WORKSPACE_ENTRY,
};

pub(crate) fn encode_archive(
    manifest: &ProjectManifest,
    workspace: &[u8],
) -> Result<Vec<u8>, ProjectError> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ProjectError::EntryTooLarge {
            entry: MANIFEST_ENTRY.into(),
            limit: MAX_MANIFEST_BYTES,
        });
    }
    let cursor = Cursor::new(Vec::new());
    let mut archive = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file(MANIFEST_ENTRY, options)?;
    archive.write_all(&manifest_bytes)?;
    archive.start_file(WORKSPACE_ENTRY, options)?;
    archive.write_all(workspace)?;
    Ok(archive.finish()?.into_inner())
}

pub(crate) fn read_entries<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<BTreeMap<String, Vec<u8>>, ProjectError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ProjectError::InvalidArchive(format!(
            "archive has more than {MAX_ARCHIVE_ENTRIES} entries"
        )));
    }
    let mut entries = BTreeMap::new();
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_owned();
        if entry.is_dir() || !matches!(name.as_str(), MANIFEST_ENTRY | WORKSPACE_ENTRY) {
            return Err(ProjectError::UnexpectedEntry(name));
        }
        if !names.insert(name.clone()) {
            return Err(ProjectError::DuplicateEntry(name));
        }
        let limit = match name.as_str() {
            MANIFEST_ENTRY => MAX_MANIFEST_BYTES,
            WORKSPACE_ENTRY => MAX_WORKSPACE_BYTES,
            _ => unreachable!("unexpected entries were rejected"),
        };
        if entry.size() > limit {
            return Err(ProjectError::EntryTooLarge { entry: name, limit });
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(ProjectError::EntryTooLarge { entry: name, limit });
        }
        entries.insert(name, bytes);
    }
    if !entries.contains_key(MANIFEST_ENTRY) {
        return Err(ProjectError::MissingEntry(MANIFEST_ENTRY));
    }
    if !entries.contains_key(WORKSPACE_ENTRY) {
        return Err(ProjectError::MissingEntry(WORKSPACE_ENTRY));
    }
    Ok(entries)
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| ProjectError::InvalidPath(path.to_path_buf()))?
        .to_string_lossy();
    let mut temporary = None;
    for attempt in 0..64 {
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ProjectError::Io(error)),
        }
    }
    let (temporary_path, mut temporary_file) = temporary.ok_or_else(|| {
        ProjectError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a project temporary file",
        ))
    })?;
    let write_result = temporary_file
        .write_all(bytes)
        .and_then(|()| temporary_file.sync_all());
    drop(temporary_file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(ProjectError::Io(error));
    }
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(ProjectError::Io(error));
    }
    sync_parent_directory(parent)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn sync_parent_directory(parent: &Path) -> Result<(), ProjectError> {
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn sync_parent_directory(_parent: &Path) -> Result<(), ProjectError> {
    Ok(())
}

pub(crate) fn checksum(bytes: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    hasher.finalize()
}
