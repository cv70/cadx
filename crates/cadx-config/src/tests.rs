use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::paths::PROJECTS_DIRECTORY_NAME;
use super::settings::{CURRENT_CONFIG_VERSION, MAX_CONFIG_BYTES};
use super::*;

static TEMP_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

const VALID_CONFIG: &str = r#"version: 1
provider:
  endpoint: "https://provider.example/v1"
  model: "test-model"
  api_key: "test-key"
  timeout_seconds: 30
"#;

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let counter = TEMP_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cadx-config-{label}-{}-{counter}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_private_file(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn loads_a_valid_provider_configuration_without_exposing_the_key_in_debug() {
    let directory = TemporaryDirectory::new("valid");
    let path = directory.path().join("config.yaml");
    write_private_file(&path, VALID_CONFIG.as_bytes());

    let config = CadxConfig::load(&path).unwrap();

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    assert_eq!(config.provider.endpoint, "https://provider.example/v1");
    assert_eq!(config.provider.model, "test-model");
    assert_eq!(
        config.provider.timeout(),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(config.provider.api_key(), "test-key");
    assert!(!format!("{config:?}").contains("test-key"));
}

#[test]
fn rejects_blank_api_keys_after_parsing_the_template_schema() {
    let directory = TemporaryDirectory::new("blank-key");
    let path = directory.path().join("config.yaml");
    write_private_file(&path, VALID_CONFIG.replace("test-key", "   ").as_bytes());

    let error = CadxConfig::load(&path).unwrap_err();

    assert!(matches!(
        error,
        ConfigError::InvalidProvider("provider API key is required")
    ));
}

#[test]
fn rejects_unknown_yaml_fields() {
    let directory = TemporaryDirectory::new("unknown-field");
    let path = directory.path().join("config.yaml");
    write_private_file(
        &path,
        format!("{VALID_CONFIG}unexpected: true\n").as_bytes(),
    );

    let error = CadxConfig::load(&path).unwrap_err();

    assert!(matches!(error, ConfigError::InvalidYaml(_)));
}

#[test]
fn rejects_a_configuration_larger_than_the_limit() {
    let directory = TemporaryDirectory::new("large");
    let path = directory.path().join("config.yaml");
    let mut contents = VALID_CONFIG.as_bytes().to_vec();
    contents.resize((MAX_CONFIG_BYTES + 1) as usize, b' ');
    write_private_file(&path, &contents);

    let error = CadxConfig::load(&path).unwrap_err();

    assert!(matches!(error, ConfigError::ConfigTooLarge { .. }));
}

#[cfg(unix)]
#[test]
fn rejects_a_group_or_world_readable_configuration_file() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TemporaryDirectory::new("permissions");
    let path = directory.path().join("config.yaml");
    fs::write(&path, VALID_CONFIG).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let error = CadxConfig::load(&path).unwrap_err();

    assert!(matches!(error, ConfigError::InsecurePermissions(_)));
}

#[cfg(unix)]
#[test]
fn rejects_a_symbolic_linked_configuration_file() {
    use std::os::unix::fs::symlink;

    let directory = TemporaryDirectory::new("symlink");
    let target = directory.path().join("provider.yaml");
    let path = directory.path().join("config.yaml");
    write_private_file(&target, VALID_CONFIG.as_bytes());
    symlink(&target, &path).unwrap();

    let error = CadxConfig::load(&path).unwrap_err();

    assert!(matches!(error, ConfigError::PathIsSymlink(_)));
}

#[test]
fn creates_private_working_and_project_directories() {
    let directory = TemporaryDirectory::new("directories");
    let home = directory.path().join(".cadx");

    super::paths::ensure_private_directory(&home).unwrap();
    super::paths::ensure_private_directory(&home.join(PROJECTS_DIRECTORY_NAME)).unwrap();

    assert!(home.is_dir());
    assert!(home.join(PROJECTS_DIRECTORY_NAME).is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        assert_eq!(fs::metadata(&home).unwrap().mode() & 0o777, 0o700);
    }
}

#[test]
fn initialization_creates_a_private_empty_key_template_idempotently() {
    let directory = TemporaryDirectory::new("template");
    let home = directory.path().join(".cadx");

    let path = super::paths::initialize_config_at(&home).unwrap();
    let repeated_path = super::paths::initialize_config_at(&home).unwrap();

    assert_eq!(path, repeated_path);
    assert!(home.join(PROJECTS_DIRECTORY_NAME).is_dir());
    assert!(fs::read_to_string(&path).unwrap().contains("api_key: \"\""));
    assert!(matches!(
        CadxConfig::load(&path),
        Err(ConfigError::InvalidProvider("provider API key is required"))
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }
}

#[test]
fn interface_preferences_round_trip_both_supported_languages() {
    let directory = TemporaryDirectory::new("preferences-round-trip");
    let path = directory.path().join("preferences.yaml");

    for language in [UiLanguage::English, UiLanguage::SimplifiedChinese] {
        let preferences = CadxPreferences::for_language(language);
        preferences.save(&path).unwrap();

        assert_eq!(CadxPreferences::load(&path).unwrap(), preferences);
    }
}

#[test]
fn interface_preferences_reject_unknown_fields_and_versions() {
    let directory = TemporaryDirectory::new("invalid-preferences");
    let path = directory.path().join("preferences.yaml");
    write_private_file(&path, b"version: 1\nlanguage: english\nunexpected: true\n");
    assert!(matches!(
        CadxPreferences::load(&path),
        Err(ConfigError::InvalidYaml(_))
    ));

    write_private_file(&path, b"version: 999\nlanguage: english\n");
    assert!(matches!(
        CadxPreferences::load(&path),
        Err(ConfigError::UnsupportedVersion(999))
    ));
}

#[test]
fn locale_detection_maps_chinese_variants_and_falls_back_to_english() {
    assert_eq!(
        UiLanguage::from_locale("zh-CN"),
        UiLanguage::SimplifiedChinese
    );
    assert_eq!(
        UiLanguage::from_locale("zh_Hans_CN.UTF-8"),
        UiLanguage::SimplifiedChinese
    );
    assert_eq!(UiLanguage::from_locale("en-US"), UiLanguage::English);
    assert_eq!(UiLanguage::from_locale("ja-JP"), UiLanguage::English);
}

#[test]
fn preference_save_retries_after_a_temporary_name_collision() {
    let directory = TemporaryDirectory::new("preference-temp-collision");
    let path = directory.path().join("preferences.yaml");
    let colliding_path = directory
        .path()
        .join(format!(".preferences.yaml.{}.0.tmp", std::process::id()));
    write_private_file(&colliding_path, b"occupied");

    CadxPreferences::for_language(UiLanguage::SimplifiedChinese)
        .save(&path)
        .unwrap();

    assert_eq!(
        CadxPreferences::load(&path).unwrap().language,
        UiLanguage::SimplifiedChinese
    );
    assert_eq!(fs::read(&colliding_path).unwrap(), b"occupied");
}

#[cfg(unix)]
#[test]
fn preferences_reject_insecure_permissions_and_symbolic_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = TemporaryDirectory::new("preference-security");
    let target = directory.path().join("target.yaml");
    let linked = directory.path().join("linked.yaml");
    write_private_file(&target, b"version: 1\nlanguage: english\n");
    symlink(&target, &linked).unwrap();
    assert!(matches!(
        CadxPreferences::load(&linked),
        Err(ConfigError::PathIsSymlink(_))
    ));
    assert!(matches!(
        CadxPreferences::for_language(UiLanguage::English).save(&linked),
        Err(ConfigError::PathIsSymlink(_))
    ));

    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        CadxPreferences::load(&target),
        Err(ConfigError::InsecurePermissions(_))
    ));
}
