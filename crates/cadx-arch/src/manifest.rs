//! Minimal manifest reader: crate name, dependency names, and source files.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// One workspace member's manifest reduced to the facts the layering rules use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateManifest {
    /// The crate name from `[package] name`.
    pub name: String,
    /// The crate's directory, absolute.
    pub directory: PathBuf,
    /// Names from `[dependencies]`, excluding dev and build dependencies.
    pub dependencies: Vec<String>,
    /// Names from `[dev-dependencies]`.
    pub dev_dependencies: Vec<String>,
}

impl CrateManifest {
    /// Reads `directory/Cargo.toml`.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the manifest cannot be read or
    /// declares no package name.
    pub fn open(directory: &Path) -> Result<Self, String> {
        let path = directory.join("Cargo.toml");
        let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let name = parse_name(&text).ok_or_else(|| "manifest has no [package] name".to_owned())?;
        Ok(Self {
            name,
            directory: directory.to_path_buf(),
            dependencies: parse_dependencies(&text, "dependencies"),
            dev_dependencies: parse_dependencies(&text, "dev-dependencies"),
        })
    }

    /// Every `.rs` file below this crate's `src/` directory, sorted.
    #[must_use]
    pub fn source_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        collect_rust_files(&self.directory.join("src"), &mut files);
        collect_rust_files(&self.directory.join("tests"), &mut files);
        files.sort();
        files
    }
}

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn parse_name(text: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == "[package]";
            continue;
        }
        if inside && let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            return Some(rest.trim_matches('"').to_owned());
        }
    }
    None
}

/// Extracts dependency names from the named dependency table of a manifest.
///
/// Both `foo.workspace = true` and `foo = { ... }` forms are recognized.
#[must_use]
pub fn parse_dependencies(text: &str, table: &str) -> Vec<String> {
    let heading = format!("[{table}]");
    let mut names = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == heading;
            continue;
        }
        if !inside || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((left, _)) = trimmed.split_once('=') else {
            continue;
        };
        let name = left.trim().split('.').next().unwrap_or_default().trim();
        if !name.is_empty() && !names.iter().any(|existing| existing == name) {
            names.push(name.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[package]\nname = \"cadx-example\"\nedition.workspace = true\n\n[dependencies]\ncadx-core.workspace = true\nserde = { version = \"1\", features = [\"derive\"] }\n# comment\n\n[dev-dependencies]\ntempfile.workspace = true\n\n[lints]\nworkspace = true\n";

    #[test]
    fn reads_the_package_name() {
        assert_eq!(parse_name(SAMPLE).as_deref(), Some("cadx-example"));
    }

    #[test]
    fn separates_runtime_from_dev_dependencies() {
        assert_eq!(
            parse_dependencies(SAMPLE, "dependencies"),
            vec!["cadx-core", "serde"]
        );
        assert_eq!(
            parse_dependencies(SAMPLE, "dev-dependencies"),
            vec!["tempfile"]
        );
    }

    #[test]
    fn ignores_unrelated_tables() {
        assert!(parse_dependencies(SAMPLE, "build-dependencies").is_empty());
    }
}
