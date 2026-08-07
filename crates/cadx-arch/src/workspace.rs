//! Workspace discovery: locates the repository root and enumerates members.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use crate::manifest::CrateManifest;

/// A parsed view of the CADX cargo workspace read from disk.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    members: Vec<String>,
    manifests: Vec<CrateManifest>,
}

impl Workspace {
    /// Locates the workspace root by walking up from this crate's directory.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root manifest cannot be found or
    /// read, or when a declared member manifest is missing.
    pub fn discover() -> Result<Self, WorkspaceError> {
        let root = workspace_root()?;
        Self::open(&root)
    }

    /// Reads the workspace rooted at `root`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root manifest or a declared member
    /// manifest cannot be read.
    pub fn open(root: &Path) -> Result<Self, WorkspaceError> {
        let root_manifest = root.join("Cargo.toml");
        let text = fs::read_to_string(&root_manifest)
            .map_err(|error| WorkspaceError::Read(root_manifest.clone(), error.to_string()))?;
        let member_paths = parse_members(&text);
        if member_paths.is_empty() {
            return Err(WorkspaceError::NoMembers(root_manifest));
        }

        let mut members = Vec::with_capacity(member_paths.len());
        let mut manifests = Vec::with_capacity(member_paths.len());
        for relative in member_paths {
            let directory = root.join(&relative);
            let manifest = CrateManifest::open(&directory)
                .map_err(|error| WorkspaceError::Member(relative.clone(), error))?;
            members.push(manifest.name.clone());
            manifests.push(manifest);
        }
        Ok(Self {
            root: root.to_path_buf(),
            members,
            manifests,
        })
    }

    /// The absolute workspace root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every workspace member crate name, in manifest declaration order.
    #[must_use]
    pub fn members(&self) -> &[String] {
        &self.members
    }

    /// Every parsed member manifest, in manifest declaration order.
    #[must_use]
    pub fn manifests(&self) -> &[CrateManifest] {
        &self.manifests
    }
}

fn workspace_root() -> Result<PathBuf, WorkspaceError> {
    let mut directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = directory.join("Cargo.toml");
        if candidate.is_file() {
            let text = fs::read_to_string(&candidate)
                .map_err(|error| WorkspaceError::Read(candidate.clone(), error.to_string()))?;
            if text.contains("[workspace]") {
                return Ok(directory);
            }
        }
        if !directory.pop() {
            return Err(WorkspaceError::RootNotFound);
        }
    }
}

/// Extracts the `members = [...]` paths from a workspace root manifest.
fn parse_members(text: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            inside = true;
            continue;
        }
        if inside {
            if trimmed.starts_with(']') {
                break;
            }
            if let Some(value) = quoted(trimmed) {
                members.push(value);
            }
        }
    }
    members
}

fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    let value = &rest[..end];
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

/// A failure while reading the workspace layout from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// No ancestor directory contains a `[workspace]` manifest.
    RootNotFound,
    /// A manifest exists but could not be read.
    Read(PathBuf, String),
    /// The root manifest declares no members.
    NoMembers(PathBuf),
    /// A declared member could not be read.
    Member(String, String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotFound => formatter.write_str("no ancestor [workspace] manifest was found"),
            Self::Read(path, error) => write!(formatter, "cannot read {}: {error}", path.display()),
            Self::NoMembers(path) => {
                write!(formatter, "{} declares no members", path.display())
            }
            Self::Member(member, error) => write!(formatter, "member {member}: {error}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_member_paths_until_the_closing_bracket() {
        let text = "[workspace]\nresolver = \"3\"\nmembers = [\n    \"crates/a\",\n    \"crates/b\",\n]\ndefault-members = [\"crates/a\"]\n";
        assert_eq!(parse_members(text), vec!["crates/a", "crates/b"]);
    }

    #[test]
    fn discovers_the_cadx_workspace_with_every_member_present() {
        let workspace = Workspace::discover().unwrap();
        assert!(workspace.root().join("Cargo.toml").is_file());
        assert!(workspace.members().iter().any(|name| name == "cadx-core"));
        assert_eq!(workspace.members().len(), workspace.manifests().len());
    }
}
