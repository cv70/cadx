use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CORE_VALIDATOR_ID, CORE_VALIDATOR_VERSION, CURRENT_SCHEMA_VERSION, Capability};

pub const PACK_LOCK_VERSION: u32 = 1;
pub const PACK_ABI_VERSION: u32 = 1;
pub const PACK_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MAX_LOCKED_PACKS: usize = 32;
pub const MAX_LOCKED_DEPENDENCIES: usize = 128;
pub const MAX_PACK_DESCRIPTORS: usize = 256;
pub const MAX_PACK_STRING_BYTES: usize = 256;

const PACK_LOCK_HASH_DOMAIN: &[u8] = b"CADX-PACK-LOCK\0canonical-json-v1\0";
const ARTIFACT_HASH_DOMAIN: &[u8] = b"CADX-BUILTIN-ARTIFACT\0v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackTrust {
    BuiltIn,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackOperationDescriptor {
    pub id: String,
    pub schema_version: u32,
    pub capability: Capability,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackQueryDescriptor {
    pub id: String,
    pub schema_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackValidatorDescriptor {
    pub id: String,
    pub version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackManifest {
    pub id: String,
    pub version: String,
    pub abi_version: u32,
    pub schema_version: u32,
    pub operations: Vec<PackOperationDescriptor>,
    pub queries: Vec<PackQueryDescriptor>,
    pub validators: Vec<PackValidatorDescriptor>,
    pub migrations: Vec<String>,
    pub artifact_hash: [u8; 32],
    pub publisher: String,
    pub trust: PackTrust,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockedDependencyKind {
    Core,
    GeometryKernel,
    Solver,
    Validator,
    RuleSet,
    MaterialLibrary,
    UnitDatabase,
    ExchangeProfile,
    ReleasePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDependency {
    pub kind: LockedDependencyKind,
    pub id: String,
    pub version: String,
    pub schema_version: u32,
    pub content_hash: [u8; 32],
}

/// Exact semantic dependency set used to prepare, validate, and replay a project.
///
/// The current host only accepts the built-in compatibility lock. The type is
/// intentionally complete enough for persistence and evidence binding; native
/// artifact loading and explicit Pack migrations remain separate host work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackLock {
    pub lock_version: u32,
    pub packs: Vec<PackManifest>,
    pub dependencies: Vec<LockedDependency>,
}

impl PackLock {
    pub fn current() -> Self {
        let mut packs = vec![
            compatibility_pack(
                "cadx.pack.architecture.compat",
                Capability::Architecture,
                &["create_entity", "update_entity", "delete_entity"],
            ),
            compatibility_pack(
                "cadx.pack.drafting.compat",
                Capability::Drafting,
                &[
                    "create_layer",
                    "update_layer",
                    "delete_layer",
                    "create_entity",
                    "update_entity",
                    "delete_entity",
                ],
            ),
            compatibility_pack(
                "cadx.pack.mechanical.compat",
                Capability::Mechanical,
                &[
                    "create_entity",
                    "update_entity",
                    "delete_entity",
                    "create_constraint",
                    "update_constraint",
                    "delete_constraint",
                ],
            ),
            compatibility_pack(
                "cadx.pack.parameters.compat",
                Capability::Parameters,
                &["set_parameter", "delete_parameter"],
            ),
        ];
        packs.sort_by(|left, right| left.id.cmp(&right.id));

        let mut dependencies = vec![
            locked_dependency(
                LockedDependencyKind::Core,
                "cadx.core",
                env!("CARGO_PKG_VERSION"),
                CURRENT_SCHEMA_VERSION,
                "typed-document+history+workspace-v1",
            ),
            locked_dependency(
                LockedDependencyKind::GeometryKernel,
                "cadx.geometry.extrusion-mesh",
                "1",
                1,
                "bounded-profile-extrusion-v1",
            ),
            locked_dependency(
                LockedDependencyKind::Solver,
                "cadx.solver.sketch2d",
                "1",
                1,
                "deterministic-small-sketch-solver-v1",
            ),
            locked_dependency(
                LockedDependencyKind::Validator,
                CORE_VALIDATOR_ID,
                &CORE_VALIDATOR_VERSION.to_string(),
                1,
                "document-structure+sketch-constraints-v1",
            ),
            locked_dependency(
                LockedDependencyKind::RuleSet,
                "cadx.rules.compatibility-draft",
                "1",
                1,
                "core-hard-errors+draft-warnings-v1",
            ),
            locked_dependency(
                LockedDependencyKind::MaterialLibrary,
                "cadx.materials.none",
                "1",
                1,
                "no-authoritative-material-library",
            ),
            locked_dependency(
                LockedDependencyKind::UnitDatabase,
                "cadx.units.builtin",
                "1",
                1,
                "millimeter-inch-conversion-v1",
            ),
            locked_dependency(
                LockedDependencyKind::ExchangeProfile,
                "cadx.exchange.dxf-r2013-subset",
                "1",
                1,
                "bounded-2d-dxf-profile-v1",
            ),
            locked_dependency(
                LockedDependencyKind::ExchangeProfile,
                "cadx.exchange.pdf-vector-2d",
                "1",
                1,
                "single-page-vector-pdf-profile-v1",
            ),
            locked_dependency(
                LockedDependencyKind::ReleasePolicy,
                "cadx.release.draft-only",
                "1",
                1,
                "no-release-attestation-supported",
            ),
        ];
        dependencies.sort();
        Self {
            lock_version: PACK_LOCK_VERSION,
            packs,
            dependencies,
        }
    }

    pub fn validate(&self) -> Result<(), PackLockError> {
        if self.lock_version != PACK_LOCK_VERSION {
            return Err(PackLockError::UnsupportedVersion(self.lock_version));
        }
        if self.packs.is_empty() || self.packs.len() > MAX_LOCKED_PACKS {
            return Err(PackLockError::Invalid("invalid locked Pack count"));
        }
        if self.dependencies.is_empty() || self.dependencies.len() > MAX_LOCKED_DEPENDENCIES {
            return Err(PackLockError::Invalid("invalid locked dependency count"));
        }
        if !strictly_sorted_unique(self.packs.iter().map(|pack| pack.id.as_str())) {
            return Err(PackLockError::Invalid(
                "Pack manifests must be sorted by unique id",
            ));
        }
        if !strictly_sorted_unique(self.dependencies.iter()) {
            return Err(PackLockError::Invalid(
                "dependencies must be sorted and unique",
            ));
        }
        for pack in &self.packs {
            validate_string(&pack.id, "Pack id")?;
            validate_string(&pack.version, "Pack version")?;
            validate_string(&pack.publisher, "Pack publisher")?;
            if pack.abi_version != PACK_ABI_VERSION || pack.schema_version == 0 {
                return Err(PackLockError::Invalid(
                    "Pack ABI and schema versions must be supported",
                ));
            }
            if pack.operations.is_empty()
                || pack.operations.len() > MAX_PACK_DESCRIPTORS
                || pack.queries.is_empty()
                || pack.queries.len() > MAX_PACK_DESCRIPTORS
                || pack.validators.is_empty()
                || pack.validators.len() > MAX_PACK_DESCRIPTORS
                || pack.migrations.len() > MAX_PACK_DESCRIPTORS
            {
                return Err(PackLockError::Invalid("Pack descriptor counts are invalid"));
            }
            if !strictly_sorted_unique(pack.operations.iter())
                || !strictly_sorted_unique(pack.queries.iter())
                || !strictly_sorted_unique(pack.validators.iter())
                || !strictly_sorted_unique(pack.migrations.iter())
            {
                return Err(PackLockError::Invalid(
                    "Pack descriptors must be sorted and unique",
                ));
            }
            for operation in &pack.operations {
                validate_string(&operation.id, "operation id")?;
                if operation.schema_version == 0 {
                    return Err(PackLockError::Invalid(
                        "operation schema version must be non-zero",
                    ));
                }
            }
            for query in &pack.queries {
                validate_string(&query.id, "query id")?;
                if query.schema_version == 0 {
                    return Err(PackLockError::Invalid(
                        "query schema version must be non-zero",
                    ));
                }
            }
            for validator in &pack.validators {
                validate_string(&validator.id, "validator id")?;
                if validator.version == 0 {
                    return Err(PackLockError::Invalid("validator version must be non-zero"));
                }
            }
            for migration in &pack.migrations {
                validate_string(migration, "migration id")?;
            }
            if pack.artifact_hash == [0; 32] {
                return Err(PackLockError::Invalid(
                    "Pack artifact hash must be non-zero",
                ));
            }
        }
        for dependency in &self.dependencies {
            validate_string(&dependency.id, "dependency id")?;
            validate_string(&dependency.version, "dependency version")?;
            if dependency.schema_version == 0 || dependency.content_hash == [0; 32] {
                return Err(PackLockError::Invalid(
                    "dependency schema version and hash must be present",
                ));
            }
        }
        Ok(())
    }

    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(PACK_LOCK_HASH_DOMAIN);
        serde_json::to_writer(HashWriter(&mut hasher), self)
            .expect("PackLock serialization into SHA-256 cannot fail");
        hasher.finalize().into()
    }

    pub fn hash_hex(&self) -> String {
        let hash = self.hash();
        let mut encoded = String::with_capacity(hash.len() * 2);
        for byte in hash {
            write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
        }
        encoded
    }
}

fn compatibility_pack(id: &str, capability: Capability, operation_names: &[&str]) -> PackManifest {
    let mut operations = operation_names
        .iter()
        .map(|name| PackOperationDescriptor {
            id: format!("{id}.{name}"),
            schema_version: 1,
            capability,
        })
        .collect::<Vec<_>>();
    operations.sort();
    let mut queries = vec![
        PackQueryDescriptor {
            id: format!("{id}.document_summary"),
            schema_version: 1,
        },
        PackQueryDescriptor {
            id: format!("{id}.object_summary"),
            schema_version: 1,
        },
    ];
    queries.sort();
    PackManifest {
        id: id.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        abi_version: PACK_ABI_VERSION,
        schema_version: PACK_MANIFEST_SCHEMA_VERSION,
        operations,
        queries,
        validators: vec![PackValidatorDescriptor {
            id: CORE_VALIDATOR_ID.into(),
            version: CORE_VALIDATOR_VERSION,
        }],
        migrations: Vec::new(),
        artifact_hash: builtin_artifact_hash(id),
        publisher: "cadx.project".into(),
        trust: PackTrust::BuiltIn,
    }
}

fn locked_dependency(
    kind: LockedDependencyKind,
    id: &str,
    version: &str,
    schema_version: u32,
    semantic_contract: &str,
) -> LockedDependency {
    LockedDependency {
        kind,
        id: id.into(),
        version: version.into(),
        schema_version,
        content_hash: builtin_artifact_hash(semantic_contract),
    }
}

fn builtin_artifact_hash(value: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ARTIFACT_HASH_DOMAIN);
    hasher.update(value.as_bytes());
    hasher.finalize().into()
}

fn validate_string(value: &str, field: &'static str) -> Result<(), PackLockError> {
    if value.trim().is_empty() || value.len() > MAX_PACK_STRING_BYTES {
        return Err(PackLockError::Invalid(field));
    }
    Ok(())
}

fn strictly_sorted_unique<T: Ord>(values: impl IntoIterator<Item = T>) -> bool {
    let mut previous = None;
    for value in values {
        if previous.as_ref().is_some_and(|previous| previous >= &value) {
            return false;
        }
        previous = Some(value);
    }
    true
}

struct HashWriter<'hasher>(&'hasher mut Sha256);

impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackLockError {
    UnsupportedVersion(u32),
    Invalid(&'static str),
}

impl fmt::Display for PackLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported PackLock version {version}")
            }
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for PackLockError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_pack_lock_is_valid_canonical_and_hash_stable() {
        let lock = PackLock::current();
        lock.validate().unwrap();
        assert_eq!(lock.hash(), PackLock::current().hash());
        assert_eq!(lock.hash_hex().len(), 64);
        assert!(lock.packs.iter().any(|pack| {
            pack.id == "cadx.pack.mechanical.compat"
                && pack
                    .operations
                    .iter()
                    .any(|operation| operation.id.ends_with("create_constraint"))
        }));
    }

    #[test]
    fn pack_lock_rejects_reordering_duplicates_and_unbounded_strings() {
        let mut reordered = PackLock::current();
        reordered.packs.swap(0, 1);
        assert!(reordered.validate().is_err());

        let mut duplicate = PackLock::current();
        duplicate
            .dependencies
            .push(duplicate.dependencies[0].clone());
        duplicate.dependencies.sort();
        assert!(duplicate.validate().is_err());

        let mut oversized = PackLock::current();
        oversized.packs[0].publisher = "x".repeat(MAX_PACK_STRING_BYTES + 1);
        assert!(oversized.validate().is_err());
    }
}
