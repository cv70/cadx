use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use cadx_core::{CURRENT_SCHEMA_VERSION, TaskWorkspace};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::archive::{
    checksum, encode_archive, read_entries, sync_parent_directory, write_atomically,
};
use crate::error::ProjectError;

pub const CURRENT_PROJECT_FORMAT_VERSION: u32 = 11;
pub const PROJECT_EXTENSION: &str = "cadx";
pub const RECOVERY_SUFFIX: &str = ".autosave.cadx";

pub(crate) const MANIFEST_ENTRY: &str = "manifest.json";
pub(crate) const WORKSPACE_ENTRY: &str = "workspace.json";
pub(crate) const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub(crate) const MAX_WORKSPACE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_ARCHIVE_ENTRIES: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectManifest {
    #[serde(default)]
    pub format_version: u32,
    #[serde(default)]
    pub document_schema_version: u32,
    #[serde(default = "default_workspace_entry")]
    pub workspace_entry: String,
    #[serde(default)]
    pub workspace_bytes: u64,
    #[serde(default)]
    pub workspace_crc32: u32,
}

fn default_workspace_entry() -> String {
    WORKSPACE_ENTRY.into()
}

impl ProjectManifest {
    pub(crate) fn current(workspace: &[u8]) -> Self {
        Self {
            format_version: CURRENT_PROJECT_FORMAT_VERSION,
            document_schema_version: CURRENT_SCHEMA_VERSION,
            workspace_entry: WORKSPACE_ENTRY.into(),
            workspace_bytes: workspace.len() as u64,
            workspace_crc32: checksum(workspace),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSaveReport {
    pub path: PathBuf,
    pub workspace_bytes: u64,
    pub format_version: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectLoad {
    pub workspace: TaskWorkspace,
    pub manifest: ProjectManifest,
    pub migrated: bool,
}

/// Saves a validated workspace as a lossless, versioned `.cadx` archive.
///
/// The payload is fully encoded before the destination changes. It is then
/// written to a same-directory temporary file, synced, and renamed into place.
pub fn save_workspace(
    workspace: &TaskWorkspace,
    path: impl AsRef<Path>,
) -> Result<ProjectSaveReport, ProjectError> {
    workspace.validate_integrity()?;
    let workspace_bytes = serde_json::to_vec(workspace)?;
    if workspace_bytes.len() as u64 > MAX_WORKSPACE_BYTES {
        return Err(ProjectError::EntryTooLarge {
            entry: WORKSPACE_ENTRY.into(),
            limit: MAX_WORKSPACE_BYTES,
        });
    }
    let manifest = ProjectManifest::current(&workspace_bytes);
    let archive = encode_archive(&manifest, &workspace_bytes)?;
    let path = path.as_ref();
    write_atomically(path, &archive)?;
    Ok(ProjectSaveReport {
        path: path.to_path_buf(),
        workspace_bytes: manifest.workspace_bytes,
        format_version: manifest.format_version,
    })
}

/// Loads a `.cadx` archive, verifies its integrity, migrates supported legacy
/// versions, and proves that the active document agrees with replayed history.
pub fn load_workspace(path: impl AsRef<Path>) -> Result<ProjectLoad, ProjectError> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    let entries = read_entries(&mut archive)?;
    let manifest_bytes = entries
        .get(MANIFEST_ENTRY)
        .ok_or(ProjectError::MissingEntry(MANIFEST_ENTRY))?;
    let workspace_bytes = entries
        .get(WORKSPACE_ENTRY)
        .ok_or(ProjectError::MissingEntry(WORKSPACE_ENTRY))?;
    let mut manifest = serde_json::from_slice::<ProjectManifest>(manifest_bytes)?;
    let source_format_version = manifest.format_version;
    let requires_legacy_evidence_migration = source_format_version < 3;
    let requires_legacy_execution_migration = source_format_version < 5;
    let requires_legacy_object_precondition_migration = source_format_version < 6;
    let requires_legacy_task_hierarchy_migration = source_format_version < 7;
    let requires_legacy_compensation_migration = source_format_version < 8;
    let requires_legacy_execution_strategy_migration = source_format_version < 9;
    let requires_legacy_remote_policy_migration = source_format_version < 10;
    let requires_legacy_planning_budget_migration = source_format_version < 11;
    let mut migrated = migrate_manifest(&mut manifest, workspace_bytes)?;
    let mut workspace = serde_json::from_slice::<TaskWorkspace>(workspace_bytes)?;
    if manifest.document_schema_version != workspace.document().schema_version {
        return Err(ProjectError::InvalidManifest(format!(
            "manifest document schema {} does not match workspace schema {}",
            manifest.document_schema_version,
            workspace.document().schema_version
        )));
    }
    let requires_schema_migration = workspace.document().schema_version != CURRENT_SCHEMA_VERSION
        || workspace
            .history()
            .snapshots
            .values()
            .any(|snapshot| snapshot.document.schema_version != CURRENT_SCHEMA_VERSION);
    let recovered_interrupted_task = workspace
        .tasks()
        .values()
        .any(|task| task.status == cadx_core::TaskStatus::Running);
    if requires_legacy_execution_strategy_migration {
        workspace.kernel().migrate_legacy_execution_strategies();
    }
    if requires_legacy_remote_policy_migration {
        workspace.kernel().migrate_legacy_remote_policy();
    }
    if requires_legacy_planning_budget_migration {
        workspace.kernel().migrate_legacy_planning_budgets();
    }
    if requires_legacy_evidence_migration {
        workspace.kernel().migrate_legacy_to_current()?;
    } else if requires_legacy_execution_migration {
        workspace.kernel().migrate_legacy_executions_to_current()?;
    } else if requires_legacy_object_precondition_migration {
        workspace
            .kernel()
            .migrate_legacy_object_preconditions_to_current()?;
    } else if requires_legacy_task_hierarchy_migration {
        workspace
            .kernel()
            .migrate_legacy_task_hierarchy_to_current()?;
    } else if requires_legacy_compensation_migration || requires_legacy_execution_strategy_migration
    {
        // Format v8 fields are additive and default to an uncompensated
        // ChangeSet plus an empty deleted-parameter diff for v7 payloads.
        // Format v9 explicitly marks older persisted plans as batch execution.
        workspace.kernel().migrate_to_current()?;
    } else {
        workspace.kernel().migrate_to_current()?;
    }
    migrated |= requires_schema_migration
        || recovered_interrupted_task
        || requires_legacy_remote_policy_migration
        || requires_legacy_planning_budget_migration;
    manifest.document_schema_version = workspace.document().schema_version;
    Ok(ProjectLoad {
        workspace,
        manifest,
        migrated,
    })
}

/// Returns the hidden, same-directory recovery file for a project path.
pub fn recovery_path(path: impl AsRef<Path>) -> Result<PathBuf, ProjectError> {
    let path = path.as_ref();
    let file_name = path
        .file_name()
        .ok_or_else(|| ProjectError::InvalidPath(path.to_path_buf()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut recovery_name = OsString::from(".");
    recovery_name.push(file_name);
    recovery_name.push(RECOVERY_SUFFIX);
    Ok(parent.join(recovery_name))
}

/// Checks for a trustworthy regular recovery file without following symlinks.
pub fn recovery_exists(path: impl AsRef<Path>) -> Result<bool, ProjectError> {
    let recovery = recovery_path(path)?;
    match fs::symlink_metadata(&recovery) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ProjectError::InvalidPath(recovery))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ProjectError::Io(error)),
    }
}

/// Saves a validated workspace to its atomic crash-recovery sidecar.
pub fn save_recovery(
    workspace: &TaskWorkspace,
    project_path: impl AsRef<Path>,
) -> Result<ProjectSaveReport, ProjectError> {
    let project_path = project_path.as_ref();
    let recovery = recovery_path(project_path)?;
    let _ = recovery_exists(project_path)?;
    save_workspace(workspace, recovery)
}

/// Loads and fully validates a project's crash-recovery sidecar.
pub fn load_recovery(project_path: impl AsRef<Path>) -> Result<ProjectLoad, ProjectError> {
    let project_path = project_path.as_ref();
    if !recovery_exists(project_path)? {
        return Err(ProjectError::InvalidPath(recovery_path(project_path)?));
    }
    load_workspace(recovery_path(project_path)?)
}

/// Removes a recovery sidecar after a successful primary save or explicit discard.
pub fn discard_recovery(project_path: impl AsRef<Path>) -> Result<bool, ProjectError> {
    let recovery = recovery_path(project_path)?;
    match fs::remove_file(&recovery) {
        Ok(()) => {
            let parent = recovery
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            sync_parent_directory(parent)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ProjectError::Io(error)),
    }
}

fn migrate_manifest(
    manifest: &mut ProjectManifest,
    workspace: &[u8],
) -> Result<bool, ProjectError> {
    if manifest.format_version > CURRENT_PROJECT_FORMAT_VERSION {
        return Err(ProjectError::UnsupportedFormatVersion(
            manifest.format_version,
        ));
    }
    if manifest.workspace_entry != WORKSPACE_ENTRY {
        return Err(ProjectError::InvalidManifest(format!(
            "workspace entry must be {WORKSPACE_ENTRY}"
        )));
    }
    if manifest.format_version >= 1 {
        if manifest.workspace_bytes != workspace.len() as u64 {
            return Err(ProjectError::IntegrityMismatch {
                expected: manifest.workspace_crc32,
                actual: checksum(workspace),
            });
        }
        let actual = checksum(workspace);
        if manifest.workspace_crc32 != actual {
            return Err(ProjectError::IntegrityMismatch {
                expected: manifest.workspace_crc32,
                actual,
            });
        }
    } else {
        // Format zero predated a manifest checksum. It uses the same workspace
        // representation, so migration adds the integrity metadata in memory.
        manifest.workspace_bytes = workspace.len() as u64;
        manifest.workspace_crc32 = checksum(workspace);
    }

    let migrated = manifest.format_version != CURRENT_PROJECT_FORMAT_VERSION;
    manifest.format_version = CURRENT_PROJECT_FORMAT_VERSION;
    Ok(migrated)
}
