use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Capability, EntityId, RemoteDataCategory};

pub type RemoteGrantId = u64;

pub const MAX_REMOTE_SELECTED_ENTITY_IDS: usize = 1_024;
const MAX_ENDPOINT_BYTES: usize = 2_048;
const MAX_MODEL_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(Uuid);

impl ProjectId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProjectId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteObjectScope {
    ProjectSummary,
    SelectedEntities { entity_ids: BTreeSet<EntityId> },
}

impl RemoteObjectScope {
    pub fn from_selected_entities(entity_ids: impl IntoIterator<Item = EntityId>) -> Self {
        let entity_ids = entity_ids.into_iter().collect::<BTreeSet<_>>();
        if entity_ids.is_empty() {
            Self::ProjectSummary
        } else {
            Self::SelectedEntities { entity_ids }
        }
    }

    pub fn permits(&self, selected_entity_ids: &[EntityId]) -> bool {
        match self {
            Self::ProjectSummary => selected_entity_ids.is_empty(),
            Self::SelectedEntities { entity_ids } => selected_entity_ids
                .iter()
                .all(|entity_id| entity_ids.contains(entity_id)),
        }
    }

    fn validate(&self) -> Result<(), RemotePolicyError> {
        if let Self::SelectedEntities { entity_ids } = self
            && (entity_ids.is_empty() || entity_ids.len() > MAX_REMOTE_SELECTED_ENTITY_IDS)
        {
            return Err(RemotePolicyError::InvalidGrant(
                "selected-entity scope must contain between 1 and 1024 identifiers".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteAccessGrantRequest {
    pub endpoint: String,
    pub model: String,
    pub allowed_data_categories: BTreeSet<RemoteDataCategory>,
    pub allowed_capabilities: BTreeSet<Capability>,
    pub object_scope: RemoteObjectScope,
    pub max_payload_bytes: usize,
    pub granted_at_unix_seconds: u64,
    pub expires_at_unix_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct RemoteAccessCheck<'a> {
    pub project_id: ProjectId,
    pub endpoint: &'a str,
    pub model: &'a str,
    pub data_categories: &'a BTreeSet<RemoteDataCategory>,
    pub capabilities: &'a BTreeSet<Capability>,
    pub selected_entity_ids: &'a [EntityId],
    pub payload_bytes: usize,
    pub unix_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteAccessGrant {
    pub id: RemoteGrantId,
    pub project_id: ProjectId,
    pub endpoint: String,
    pub model: String,
    pub allowed_data_categories: BTreeSet<RemoteDataCategory>,
    pub allowed_capabilities: BTreeSet<Capability>,
    pub object_scope: RemoteObjectScope,
    pub max_payload_bytes: usize,
    pub granted_at_unix_seconds: u64,
    pub expires_at_unix_seconds: Option<u64>,
    pub revoked_at_unix_seconds: Option<u64>,
}

impl RemoteAccessGrant {
    pub fn is_active_at(&self, unix_seconds: u64) -> bool {
        unix_seconds >= self.granted_at_unix_seconds
            && self
                .expires_at_unix_seconds
                .is_none_or(|expires_at| unix_seconds < expires_at)
            && self
                .revoked_at_unix_seconds
                .is_none_or(|revoked_at| unix_seconds < revoked_at)
    }

    pub fn authorizes(&self, check: RemoteAccessCheck<'_>) -> bool {
        self.project_id == check.project_id
            && self.endpoint == check.endpoint
            && self.model == check.model
            && check
                .data_categories
                .is_subset(&self.allowed_data_categories)
            && check.capabilities.is_subset(&self.allowed_capabilities)
            && self.object_scope.permits(check.selected_entity_ids)
            && check.payload_bytes <= self.max_payload_bytes
            && self.is_active_at(check.unix_seconds)
    }

    fn validate(&self, project_id: ProjectId) -> Result<(), RemotePolicyError> {
        if self.project_id != project_id {
            return Err(RemotePolicyError::InvalidGrant(
                "grant is bound to a different project".into(),
            ));
        }
        validate_text("provider endpoint", &self.endpoint, MAX_ENDPOINT_BYTES)?;
        validate_text("provider model", &self.model, MAX_MODEL_BYTES)?;
        if self.allowed_data_categories.is_empty() {
            return Err(RemotePolicyError::InvalidGrant(
                "grant must allow at least one remote data category".into(),
            ));
        }
        self.object_scope.validate()?;
        if self.max_payload_bytes == 0 || self.max_payload_bytes > crate::MAX_REMOTE_CONTEXT_BYTES {
            return Err(RemotePolicyError::InvalidGrant(format!(
                "grant payload limit must be between 1 and {} bytes",
                crate::MAX_REMOTE_CONTEXT_BYTES
            )));
        }
        if self
            .expires_at_unix_seconds
            .is_some_and(|expires_at| expires_at <= self.granted_at_unix_seconds)
        {
            return Err(RemotePolicyError::InvalidGrant(
                "grant expiry must be later than its creation time".into(),
            ));
        }
        if self
            .revoked_at_unix_seconds
            .is_some_and(|revoked_at| revoked_at < self.granted_at_unix_seconds)
        {
            return Err(RemotePolicyError::InvalidGrant(
                "grant revocation cannot predate its creation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemotePolicyEvent {
    Granted {
        grant: RemoteAccessGrant,
    },
    Revoked {
        grant_id: RemoteGrantId,
        revoked_at_unix_seconds: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteAccessPolicy {
    grants: BTreeMap<RemoteGrantId, RemoteAccessGrant>,
    events: Vec<RemotePolicyEvent>,
    next_grant_id: RemoteGrantId,
}

impl Default for RemoteAccessPolicy {
    fn default() -> Self {
        Self {
            grants: BTreeMap::new(),
            events: Vec::new(),
            next_grant_id: 1,
        }
    }
}

impl RemoteAccessPolicy {
    pub fn grants(&self) -> &BTreeMap<RemoteGrantId, RemoteAccessGrant> {
        &self.grants
    }

    pub fn events(&self) -> &[RemotePolicyEvent] {
        &self.events
    }

    pub fn create_grant(
        &mut self,
        project_id: ProjectId,
        request: RemoteAccessGrantRequest,
    ) -> Result<RemoteGrantId, RemotePolicyError> {
        if self.next_grant_id == RemoteGrantId::MAX {
            return Err(RemotePolicyError::IdSpaceExhausted);
        }
        let grant = RemoteAccessGrant {
            id: self.next_grant_id,
            project_id,
            endpoint: request.endpoint,
            model: request.model,
            allowed_data_categories: request.allowed_data_categories,
            allowed_capabilities: request.allowed_capabilities,
            object_scope: request.object_scope,
            max_payload_bytes: request.max_payload_bytes,
            granted_at_unix_seconds: request.granted_at_unix_seconds,
            expires_at_unix_seconds: request.expires_at_unix_seconds,
            revoked_at_unix_seconds: None,
        };
        grant.validate(project_id)?;
        let id = grant.id;
        self.next_grant_id += 1;
        self.grants.insert(id, grant.clone());
        self.events.push(RemotePolicyEvent::Granted { grant });
        Ok(id)
    }

    pub fn revoke_grant(
        &mut self,
        grant_id: RemoteGrantId,
        revoked_at_unix_seconds: u64,
    ) -> Result<(), RemotePolicyError> {
        let grant = self
            .grants
            .get_mut(&grant_id)
            .ok_or(RemotePolicyError::GrantMissing(grant_id))?;
        if grant.revoked_at_unix_seconds.is_some() {
            return Err(RemotePolicyError::GrantAlreadyRevoked(grant_id));
        }
        if revoked_at_unix_seconds < grant.granted_at_unix_seconds {
            return Err(RemotePolicyError::InvalidGrant(
                "grant revocation cannot predate its creation".into(),
            ));
        }
        grant.revoked_at_unix_seconds = Some(revoked_at_unix_seconds);
        self.events.push(RemotePolicyEvent::Revoked {
            grant_id,
            revoked_at_unix_seconds,
        });
        Ok(())
    }

    pub fn validate(&self, project_id: ProjectId) -> Result<(), RemotePolicyError> {
        let mut replayed = BTreeMap::<RemoteGrantId, RemoteAccessGrant>::new();
        for event in &self.events {
            match event {
                RemotePolicyEvent::Granted { grant } => {
                    grant.validate(project_id)?;
                    if grant.revoked_at_unix_seconds.is_some()
                        || replayed.insert(grant.id, grant.clone()).is_some()
                    {
                        return Err(RemotePolicyError::InvalidLedger(format!(
                            "grant {} has an invalid creation event",
                            grant.id
                        )));
                    }
                }
                RemotePolicyEvent::Revoked {
                    grant_id,
                    revoked_at_unix_seconds,
                } => {
                    let grant = replayed
                        .get_mut(grant_id)
                        .ok_or(RemotePolicyError::GrantMissing(*grant_id))?;
                    if grant.revoked_at_unix_seconds.is_some()
                        || *revoked_at_unix_seconds < grant.granted_at_unix_seconds
                    {
                        return Err(RemotePolicyError::InvalidLedger(format!(
                            "grant {grant_id} has an invalid revocation event"
                        )));
                    }
                    grant.revoked_at_unix_seconds = Some(*revoked_at_unix_seconds);
                }
            }
        }
        if replayed != self.grants {
            return Err(RemotePolicyError::InvalidLedger(
                "remote grant state does not match its event ledger".into(),
            ));
        }
        let minimum_next = self
            .grants
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(RemotePolicyError::IdSpaceExhausted)?;
        if self.next_grant_id < minimum_next || self.next_grant_id == RemoteGrantId::MAX {
            return Err(RemotePolicyError::InvalidLedger(
                "next remote grant id is invalid".into(),
            ));
        }
        Ok(())
    }
}

fn validate_text(label: &str, value: &str, max_bytes: usize) -> Result<(), RemotePolicyError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(RemotePolicyError::InvalidGrant(format!(
            "{label} must be non-empty, bounded UTF-8 without NUL"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemotePolicyError {
    GrantMissing(RemoteGrantId),
    GrantAlreadyRevoked(RemoteGrantId),
    InvalidGrant(String),
    InvalidLedger(String),
    IdSpaceExhausted,
}

impl fmt::Display for RemotePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GrantMissing(id) => write!(formatter, "remote access grant {id} does not exist"),
            Self::GrantAlreadyRevoked(id) => {
                write!(formatter, "remote access grant {id} is already revoked")
            }
            Self::InvalidGrant(message) => {
                write!(formatter, "invalid remote access grant: {message}")
            }
            Self::InvalidLedger(message) => {
                write!(formatter, "invalid remote policy ledger: {message}")
            }
            Self::IdSpaceExhausted => {
                formatter.write_str("remote access grant id space is exhausted")
            }
        }
    }
}

impl std::error::Error for RemotePolicyError {}
