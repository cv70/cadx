use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ConfigError;
use crate::paths::{
    PREFERENCES_FILE_NAME, ensure_default_working_directory, private_create_new,
    validate_private_config_file,
};

pub const CURRENT_PREFERENCES_VERSION: u32 = 1;
pub const MAX_PREFERENCES_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiLanguage {
    #[default]
    English,
    SimplifiedChinese,
}

impl UiLanguage {
    pub const ALL: [Self; 2] = [Self::English, Self::SimplifiedChinese];

    pub fn detect_system() -> Self {
        sys_locale::get_locale()
            .as_deref()
            .map(Self::from_locale)
            .unwrap_or_default()
    }

    pub fn from_locale(locale: &str) -> Self {
        let locale = locale.to_ascii_lowercase().replace('_', "-");
        if locale == "zh" || locale.starts_with("zh-") {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }

    pub const fn text(
        self,
        english: &'static str,
        simplified_chinese: &'static str,
    ) -> &'static str {
        match self {
            Self::English => english,
            Self::SimplifiedChinese => simplified_chinese,
        }
    }

    pub const fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::SimplifiedChinese => "简体中文",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CadxPreferences {
    pub version: u32,
    pub language: UiLanguage,
}

impl Default for CadxPreferences {
    fn default() -> Self {
        Self {
            version: CURRENT_PREFERENCES_VERSION,
            language: UiLanguage::detect_system(),
        }
    }
}

impl CadxPreferences {
    pub const fn for_language(language: UiLanguage) -> Self {
        Self {
            version: CURRENT_PREFERENCES_VERSION,
            language,
        }
    }

    pub fn load_default() -> Result<Self, ConfigError> {
        let directory = ensure_default_working_directory()?;
        let path = directory.join(PREFERENCES_FILE_NAME);
        match fs::symlink_metadata(&path) {
            Ok(_) => Self::load(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::io(path, error)),
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        validate_private_config_file(path)?;
        let metadata = fs::metadata(path).map_err(|error| ConfigError::io(path, error))?;
        if metadata.len() > MAX_PREFERENCES_BYTES {
            return Err(ConfigError::ConfigTooLarge {
                path: path.into(),
                limit: MAX_PREFERENCES_BYTES,
            });
        }
        let mut contents = Vec::with_capacity(metadata.len() as usize);
        File::open(path)
            .map_err(|error| ConfigError::io(path, error))?
            .take(MAX_PREFERENCES_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| ConfigError::io(path, error))?;
        if contents.len() as u64 > MAX_PREFERENCES_BYTES {
            return Err(ConfigError::ConfigTooLarge {
                path: path.into(),
                limit: MAX_PREFERENCES_BYTES,
            });
        }
        let preferences = serde_yaml::from_slice::<Self>(&contents)
            .map_err(|_| ConfigError::InvalidYaml(path.into()))?;
        if preferences.version != CURRENT_PREFERENCES_VERSION {
            return Err(ConfigError::UnsupportedVersion(preferences.version));
        }
        Ok(preferences)
    }

    pub fn save_default(&self) -> Result<PathBuf, ConfigError> {
        let directory = ensure_default_working_directory()?;
        let path = directory.join(PREFERENCES_FILE_NAME);
        self.save(&path)?;
        Ok(path)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        if self.version != CURRENT_PREFERENCES_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        let path = path.as_ref();
        match fs::symlink_metadata(path) {
            Ok(_) => validate_private_config_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ConfigError::io(path, error)),
        }
        let bytes = serde_yaml::to_string(self)
            .map_err(|_| ConfigError::InvalidYaml(path.into()))?
            .into_bytes();
        if bytes.len() as u64 > MAX_PREFERENCES_BYTES {
            return Err(ConfigError::ConfigTooLarge {
                path: path.into(),
                limit: MAX_PREFERENCES_BYTES,
            });
        }
        let (temporary, mut file) = create_temporary_file(path)?;
        let result = file.write_all(&bytes).and_then(|()| file.sync_all());
        if let Err(error) = result {
            drop(file);
            let _ = fs::remove_file(&temporary);
            return Err(ConfigError::io(path, error));
        }
        let temporary_path = match tempfile::TempPath::try_from_path(&temporary) {
            Ok(path) => path,
            Err(error) => {
                drop(file);
                let _ = fs::remove_file(&temporary);
                return Err(ConfigError::io(&temporary, error));
            }
        };
        let temporary_file = tempfile::NamedTempFile::from_parts(file, temporary_path);
        match temporary_file.persist(path) {
            Ok(file) => drop(file),
            Err(error) => return Err(ConfigError::io(path, error.error)),
        }
        sync_parent(path)?;
        Ok(())
    }
}

fn create_temporary_file(path: &Path) -> Result<(PathBuf, File), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| ConfigError::PathIsNotFile(path.into()))?
        .to_string_lossy();
    for attempt in 0..64 {
        let candidate = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match private_create_new(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(ConfigError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(ConfigError::io(
        path,
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a preferences temporary file",
        ),
    ))
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), ConfigError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| ConfigError::io(parent, error))
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}
