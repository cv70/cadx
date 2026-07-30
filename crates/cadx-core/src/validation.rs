use std::fmt::Write as _;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CadDocument, CheckResult, CheckStatus, ConstraintSolverSettings, PackLock, ValidationReport,
    solve_constraints,
};

pub const CORE_VALIDATOR_ID: &str = "cadx.core.candidate";
pub const CORE_VALIDATOR_VERSION: u32 = 1;
pub const MAX_CANDIDATE_STATE_BYTES: u64 = 64 * 1024 * 1024;

const STATE_HASH_DOMAIN: &[u8] = b"CADX-CANDIDATE-STATE\0canonical-json-v1\0";

/// Local validation output bound to one exact candidate document state.
///
/// Fields are intentionally private. Callers and planners can submit claims,
/// but only this crate can construct evidence admitted by semantic history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationEvidence {
    validator_id: String,
    validator_version: u32,
    #[serde(default)]
    pack_lock_hash: [u8; 32],
    candidate_state_hash: [u8; 32],
    report: ValidationReport,
}

impl ValidationEvidence {
    pub fn validator_id(&self) -> &str {
        &self.validator_id
    }

    pub const fn validator_version(&self) -> u32 {
        self.validator_version
    }

    pub const fn candidate_state_hash(&self) -> [u8; 32] {
        self.candidate_state_hash
    }

    pub const fn pack_lock_hash(&self) -> [u8; 32] {
        self.pack_lock_hash
    }

    pub fn pack_lock_hash_hex(&self) -> String {
        encode_hash(self.pack_lock_hash)
    }

    pub fn candidate_state_hash_hex(&self) -> String {
        encode_hash(self.candidate_state_hash)
    }

    pub fn checks(&self) -> &[CheckResult] {
        &self.report.checks
    }

    pub fn passed(&self) -> bool {
        self.report.passed()
    }

    pub fn summary(&self) -> String {
        let passed = self
            .report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Passed)
            .count();
        let warnings = self
            .report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Warning)
            .count();
        let failures = self
            .report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
            .count();
        format!("{passed} passed, {warnings} warning(s), {failures} failed")
    }

    pub(crate) fn is_current(&self) -> bool {
        self.validator_id == CORE_VALIDATOR_ID
            && self.validator_version == CORE_VALIDATOR_VERSION
            && self.pack_lock_hash == PackLock::current().hash()
    }
}

pub(crate) fn validate_candidate(document: &CadDocument) -> Result<ValidationEvidence, String> {
    let pack_lock_hash = PackLock::current().hash();
    let mut hasher = Sha256::new();
    hasher.update(STATE_HASH_DOMAIN);
    let mut writer = BoundedHashWriter {
        hasher,
        bytes_written: 0,
        limit: MAX_CANDIDATE_STATE_BYTES,
    };
    serde_json::to_writer(&mut writer, document)
        .map_err(|error| format!("cannot encode candidate state for validation: {error}"))?;
    let candidate_state_hash = writer.hasher.finalize().into();

    let mut checks = Vec::with_capacity(2);
    match document.validate() {
        Ok(()) => checks.push(CheckResult {
            name: "Core document structure".into(),
            status: CheckStatus::Passed,
            detail: format!(
                "Validated schema {}, {} layer(s), {} entity/entities, {} parameter(s), and {} constraint(s).",
                document.schema_version,
                document.layers.len(),
                document.entities.len(),
                document.parameters.len(),
                document.constraints.len()
            ),
        }),
        Err(error) => {
            checks.push(CheckResult {
                name: "Core document structure".into(),
                status: CheckStatus::Failed,
                detail: error.to_string(),
            });
            return Ok(ValidationEvidence {
                validator_id: CORE_VALIDATOR_ID.into(),
                validator_version: CORE_VALIDATOR_VERSION,
                pack_lock_hash,
                candidate_state_hash,
                report: ValidationReport { checks },
            });
        }
    }

    let constraint_check = match solve_constraints(document, ConstraintSolverSettings::default()) {
        Ok(solution) if !solution.converged => CheckResult {
            name: "Sketch constraint system".into(),
            status: CheckStatus::Failed,
            detail: format!(
                "Driving constraints did not converge after {} iteration(s); maximum residual {:.6e}.",
                solution.iterations,
                solution.maximum_driving_residual()
            ),
        },
        Ok(solution) if !solution.updated_entities.is_empty() => CheckResult {
            name: "Sketch constraint system".into(),
            status: CheckStatus::Warning,
            detail: format!(
                "The constraint system converges, but {} entity/entities still have unapplied solver updates.",
                solution.updated_entities.len()
            ),
        },
        Ok(solution) => CheckResult {
            name: "Sketch constraint system".into(),
            status: CheckStatus::Passed,
            detail: format!(
                "{} constraint(s) are satisfied after {} solver iteration(s).",
                document.constraints.len(),
                solution.iterations
            ),
        },
        Err(error) => CheckResult {
            name: "Sketch constraint system".into(),
            status: CheckStatus::Failed,
            detail: error.to_string(),
        },
    };
    checks.push(constraint_check);

    Ok(ValidationEvidence {
        validator_id: CORE_VALIDATOR_ID.into(),
        validator_version: CORE_VALIDATOR_VERSION,
        pack_lock_hash,
        candidate_state_hash,
        report: ValidationReport { checks },
    })
}

fn encode_hash(hash: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(hash.len() * 2);
    for byte in hash {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

struct BoundedHashWriter {
    hasher: Sha256,
    bytes_written: u64,
    limit: u64,
}

impl Write for BoundedHashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("candidate state chunk length exceeds u64"))?;
        let next = self
            .bytes_written
            .checked_add(length)
            .ok_or_else(|| io::Error::other("candidate state byte count overflow"))?;
        if next > self.limit {
            return Err(io::Error::other(format!(
                "candidate state exceeds the {}-byte validation limit",
                self.limit
            )));
        }
        self.hasher.update(bytes);
        self.bytes_written = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_hash_writer_enforces_its_byte_limit_before_hashing() {
        let mut writer = BoundedHashWriter {
            hasher: Sha256::new(),
            bytes_written: 0,
            limit: 3,
        };

        writer.write_all(b"abc").unwrap();
        assert_eq!(writer.bytes_written, 3);
        let error = writer.write_all(b"d").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.to_string().contains("3-byte validation limit"));
        assert_eq!(writer.bytes_written, 3);
    }
}
