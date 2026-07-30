use std::fmt;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::validation::validate_candidate;
use crate::{
    ActionSource, AgentRunId, CommandError, CommandTransaction, CommitId, DocumentSnapshot,
    ObjectPrecondition, PromptChangeSetId, TaskId,
};

const IDEMPOTENCY_DOMAIN: &[u8] = b"CADX-ACTION-IDEMPOTENCY\0v3-pack-bound\0";
const RUN_BOUND_IDEMPOTENCY_DOMAIN: &[u8] = b"CADX-ACTION-IDEMPOTENCY\0v3-run-and-pack-bound\0";
const MAX_IDEMPOTENCY_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionIdempotencyKey([u8; 32]);

impl ActionIdempotencyKey {
    pub(crate) fn derive(
        pack_lock_hash: [u8; 32],
        input_state_hash: [u8; 32],
        candidate_state_hash: [u8; 32],
        source: Option<ActionSource>,
        transaction: &CommandTransaction,
    ) -> Result<Self, String> {
        let mut writer = BoundedDigestWriter {
            hasher: Sha256::new(),
            bytes_written: 0,
            limit: MAX_IDEMPOTENCY_INPUT_BYTES,
        };
        writer
            .hasher
            .update(if source.is_some_and(ActionSource::is_run_bound) {
                RUN_BOUND_IDEMPOTENCY_DOMAIN
            } else {
                IDEMPOTENCY_DOMAIN
            });
        writer.hasher.update(pack_lock_hash);
        writer.hasher.update(input_state_hash);
        writer.hasher.update(candidate_state_hash);
        let task_id = source.map(|source| source.task_id);
        writer.hasher.update([u8::from(source.is_some())]);
        writer
            .hasher
            .update(task_id.unwrap_or_default().to_le_bytes());
        if let Some(source) = source.filter(|source| source.is_run_bound()) {
            writer
                .hasher
                .update(source.change_set_id.unwrap_or_default().to_le_bytes());
            writer
                .hasher
                .update(source.agent_run_id.unwrap_or_default().to_le_bytes());
        }
        serde_json::to_writer(&mut writer, transaction)
            .map_err(|error| format!("cannot encode prepared action identity: {error}"))?;
        Ok(Self(writer.hasher.finalize().into()))
    }

    pub fn to_hex(self) -> String {
        use std::fmt::Write as _;

        let mut encoded = String::with_capacity(64);
        for byte in self.0 {
            write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
        }
        encoded
    }
}

impl fmt::Debug for ActionIdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ActionIdempotencyKey([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedActionRecord {
    base_revision: CommitId,
    preconditions: Vec<ObjectPrecondition>,
    #[serde(default)]
    pack_lock_hash: [u8; 32],
    input_state_hash: [u8; 32],
    candidate_state_hash: [u8; 32],
    idempotency_key: ActionIdempotencyKey,
}

impl PreparedActionRecord {
    pub const fn base_revision(&self) -> CommitId {
        self.base_revision
    }

    pub fn preconditions(&self) -> &[ObjectPrecondition] {
        &self.preconditions
    }

    pub const fn input_state_hash(&self) -> [u8; 32] {
        self.input_state_hash
    }

    pub const fn pack_lock_hash(&self) -> [u8; 32] {
        self.pack_lock_hash
    }

    pub const fn candidate_state_hash(&self) -> [u8; 32] {
        self.candidate_state_hash
    }

    pub const fn idempotency_key(&self) -> ActionIdempotencyKey {
        self.idempotency_key
    }
}

/// A short-lived, locally prepared candidate that cannot be deserialized from
/// an Agent or provider response.
#[derive(Clone, PartialEq)]
pub struct PreparedAction {
    base_revision: CommitId,
    source: Option<ActionSource>,
    transaction: CommandTransaction,
    preconditions: Vec<ObjectPrecondition>,
    pack_lock_hash: [u8; 32],
    input_state_hash: [u8; 32],
    candidate_state_hash: [u8; 32],
    idempotency_key: ActionIdempotencyKey,
}

impl PreparedAction {
    pub(crate) fn prepare(
        snapshot: &DocumentSnapshot,
        task_id: Option<TaskId>,
        transaction: CommandTransaction,
    ) -> Result<Self, PrepareError> {
        Self::prepare_with_source(
            snapshot,
            task_id.map(ActionSource::legacy_task),
            transaction,
        )
    }

    pub(crate) fn prepare_for_run(
        snapshot: &DocumentSnapshot,
        task_id: TaskId,
        change_set_id: PromptChangeSetId,
        agent_run_id: AgentRunId,
        transaction: CommandTransaction,
    ) -> Result<Self, PrepareError> {
        Self::prepare_with_source(
            snapshot,
            Some(ActionSource::for_run(task_id, change_set_id, agent_run_id)),
            transaction,
        )
    }

    pub(crate) fn prepare_with_source(
        snapshot: &DocumentSnapshot,
        source: Option<ActionSource>,
        transaction: CommandTransaction,
    ) -> Result<Self, PrepareError> {
        let input_evidence =
            validate_candidate(snapshot.document()).map_err(PrepareError::ValidationUnavailable)?;
        if !input_evidence.passed() {
            return Err(PrepareError::ValidationFailed(input_evidence.summary()));
        }
        let mut candidate = snapshot.document().clone();
        transaction.apply(&mut candidate)?;
        let evidence =
            validate_candidate(&candidate).map_err(PrepareError::ValidationUnavailable)?;
        if !evidence.passed() {
            return Err(PrepareError::ValidationFailed(evidence.summary()));
        }
        let preconditions = snapshot.preconditions_for(&transaction);
        let pack_lock_hash = evidence.pack_lock_hash();
        if input_evidence.pack_lock_hash() != pack_lock_hash {
            return Err(PrepareError::ValidationUnavailable(
                "candidate validation changed PackLock within one preparation".into(),
            ));
        }
        let input_state_hash = input_evidence.candidate_state_hash();
        let candidate_state_hash = evidence.candidate_state_hash();
        let idempotency_key = ActionIdempotencyKey::derive(
            pack_lock_hash,
            input_state_hash,
            candidate_state_hash,
            source,
            &transaction,
        )
        .map_err(PrepareError::ValidationUnavailable)?;
        Ok(Self {
            base_revision: snapshot.revision(),
            source,
            transaction,
            preconditions,
            pack_lock_hash,
            input_state_hash,
            candidate_state_hash,
            idempotency_key,
        })
    }

    pub const fn base_revision(&self) -> CommitId {
        self.base_revision
    }

    pub fn preconditions(&self) -> &[ObjectPrecondition] {
        &self.preconditions
    }

    pub const fn candidate_state_hash(&self) -> [u8; 32] {
        self.candidate_state_hash
    }

    pub const fn pack_lock_hash(&self) -> [u8; 32] {
        self.pack_lock_hash
    }

    pub const fn input_state_hash(&self) -> [u8; 32] {
        self.input_state_hash
    }

    pub const fn idempotency_key(&self) -> ActionIdempotencyKey {
        self.idempotency_key
    }

    pub(crate) const fn task_id(&self) -> Option<TaskId> {
        match self.source {
            Some(source) => Some(source.task_id),
            None => None,
        }
    }

    pub(crate) fn into_transaction(self) -> CommandTransaction {
        self.transaction
    }

    pub(crate) fn record(&self) -> PreparedActionRecord {
        PreparedActionRecord {
            base_revision: self.base_revision,
            preconditions: self.preconditions.clone(),
            pack_lock_hash: self.pack_lock_hash,
            input_state_hash: self.input_state_hash,
            candidate_state_hash: self.candidate_state_hash,
            idempotency_key: self.idempotency_key,
        }
    }

    pub(crate) fn from_record(
        source: ActionSource,
        transaction: CommandTransaction,
        record: PreparedActionRecord,
    ) -> Self {
        Self {
            base_revision: record.base_revision,
            source: Some(source),
            transaction,
            preconditions: record.preconditions,
            pack_lock_hash: record.pack_lock_hash,
            input_state_hash: record.input_state_hash,
            candidate_state_hash: record.candidate_state_hash,
            idempotency_key: record.idempotency_key,
        }
    }
}

impl fmt::Debug for PreparedAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedAction")
            .field("base_revision", &self.base_revision)
            .field("source", &self.source)
            .field("command_count", &self.transaction.commands.len())
            .field("precondition_count", &self.preconditions.len())
            .field("pack_lock_hash", &"[redacted]")
            .field("input_state_hash", &"[redacted]")
            .field("candidate_state_hash", &"[redacted]")
            .field("idempotency_key", &"[redacted]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrepareError {
    Command(CommandError),
    ValidationUnavailable(String),
    ValidationFailed(String),
}

impl From<CommandError> for PrepareError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl fmt::Display for PrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::ValidationUnavailable(message) | Self::ValidationFailed(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for PrepareError {}

struct BoundedDigestWriter {
    hasher: Sha256,
    bytes_written: u64,
    limit: u64,
}

impl Write for BoundedDigestWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = u64::try_from(bytes.len())
            .map_err(|_| io::Error::other("prepared action chunk length exceeds u64"))?;
        let next = self
            .bytes_written
            .checked_add(length)
            .ok_or_else(|| io::Error::other("prepared action byte count overflow"))?;
        if next > self.limit {
            return Err(io::Error::other(format!(
                "prepared action exceeds the {}-byte identity limit",
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
