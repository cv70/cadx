//! The deterministic design document, command, task, and history contracts.
//!
//! This crate deliberately has no renderer, AI provider, or window-system
//! dependency. Every document mutation flows through [`CommandTransaction`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

pub type LayerId = u64;
pub type EntityId = u64;
pub type ParameterId = u64;
pub type TaskId = u64;
pub type CommitId = u64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: String,
    pub description: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Units {
    #[default]
    Millimeters,
    Meters,
    Inches,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub color: [u8; 4],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntityKind {
    Line {
        start: Point2,
        end: Point2,
    },
    Circle {
        center: Point2,
        radius: f64,
    },
    Rectangle {
        origin: Point2,
        width: f64,
        height: f64,
    },
    SketchProfile {
        points: Vec<Point2>,
        closed: bool,
    },
    Extrude {
        profile: EntityId,
        distance: f64,
    },
    Wall {
        start: Point2,
        end: Point2,
        thickness: f64,
    },
    Room {
        boundary: Vec<Point2>,
        area: f64,
    },
    Text {
        position: Point2,
        content: String,
    },
}

impl EntityKind {
    pub fn domain(&self) -> Domain {
        match self {
            Self::Line { .. }
            | Self::Circle { .. }
            | Self::Rectangle { .. }
            | Self::Text { .. } => Domain::Drafting,
            Self::SketchProfile { .. } | Self::Extrude { .. } => Domain::Mechanical,
            Self::Wall { .. } | Self::Room { .. } => Domain::Architecture,
        }
    }

    fn validate(&self, document: &CadDocument) -> Result<(), CommandError> {
        match self {
            Self::Line { start, end } | Self::Wall { start, end, .. }
                if !finite_point(*start) || !finite_point(*end) =>
            {
                Err(CommandError::InvalidGeometry(
                    "line endpoints must be finite".into(),
                ))
            }
            Self::Circle { center, radius } if !finite_point(*center) || !positive(*radius) => Err(
                CommandError::InvalidGeometry("circle radius must be positive".into()),
            ),
            Self::Rectangle {
                origin,
                width,
                height,
            } if !finite_point(*origin) || !positive(*width) || !positive(*height) => {
                Err(CommandError::InvalidGeometry(
                    "rectangle dimensions must be finite and positive".into(),
                ))
            }
            Self::SketchProfile { points, closed } if *closed && points.len() < 3 => Err(
                CommandError::InvalidGeometry("a closed sketch needs at least three points".into()),
            ),
            Self::SketchProfile { points, .. }
                if points.iter().any(|point| !finite_point(*point)) =>
            {
                Err(CommandError::InvalidGeometry(
                    "sketch points must be finite".into(),
                ))
            }
            Self::Extrude { profile, distance } if !positive(*distance) => Err(
                CommandError::InvalidGeometry("extrude distance must be positive".into()),
            ),
            Self::Extrude { profile, .. } => match document.entities.get(profile) {
                Some(Entity {
                    kind: EntityKind::SketchProfile { closed: true, .. },
                    ..
                }) => Ok(()),
                Some(_) => Err(CommandError::InvalidReference(
                    "an extrude requires a closed sketch profile".into(),
                )),
                None => Err(CommandError::EntityMissing(*profile)),
            },
            Self::Wall { thickness, .. } if !positive(*thickness) => Err(
                CommandError::InvalidGeometry("wall thickness must be positive".into()),
            ),
            Self::Room { boundary, area }
                if boundary.len() < 3
                    || !positive(*area)
                    || boundary.iter().any(|point| !finite_point(*point)) =>
            {
                Err(CommandError::InvalidGeometry(
                    "room boundary and area must be valid".into(),
                ))
            }
            Self::Text { position, content }
                if !finite_point(*position) || content.trim().is_empty() =>
            {
                Err(CommandError::InvalidGeometry(
                    "text needs a position and content".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Domain {
    Drafting,
    Mechanical,
    Architecture,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub layer: LayerId,
    pub name: String,
    pub visible: bool,
    pub kind: EntityKind,
    pub parameter_refs: BTreeSet<ParameterId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub id: ParameterId,
    pub name: String,
    pub value: f64,
    pub unit: Units,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CadDocument {
    pub schema_version: u32,
    pub metadata: DocumentMetadata,
    pub units: Units,
    pub layers: BTreeMap<LayerId, Layer>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub parameters: BTreeMap<ParameterId, Parameter>,
    next_layer_id: LayerId,
    next_entity_id: EntityId,
    next_parameter_id: ParameterId,
}

impl CadDocument {
    pub fn new(title: impl Into<String>) -> Self {
        let mut layers = BTreeMap::new();
        layers.insert(
            1,
            Layer {
                id: 1,
                name: "Concept".into(),
                visible: true,
                color: [73, 184, 165, 255],
            },
        );
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            metadata: DocumentMetadata {
                title: title.into(),
                description: String::new(),
            },
            units: Units::Millimeters,
            layers,
            entities: BTreeMap::new(),
            parameters: BTreeMap::new(),
            next_layer_id: 2,
            next_entity_id: 1,
            next_parameter_id: 1,
        }
    }

    pub fn next_entity_id(&self) -> EntityId {
        self.next_entity_id
    }

    pub fn next_layer_id(&self) -> LayerId {
        self.next_layer_id
    }

    pub fn next_parameter_id(&self) -> ParameterId {
        self.next_parameter_id
    }

    pub fn summary(&self) -> DocumentSummary {
        let mut domains = BTreeSet::new();
        for entity in self.entities.values() {
            domains.insert(entity.kind.domain());
        }
        DocumentSummary {
            title: self.metadata.title.clone(),
            entity_count: self.entities.len(),
            layer_count: self.layers.len(),
            domains: domains.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub title: String,
    pub entity_count: usize,
    pub layer_count: usize,
    pub domains: Vec<Domain>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CadCommand {
    CreateLayer { layer: Layer },
    CreateEntity { entity: Entity },
    UpdateEntity { entity: Entity },
    DeleteEntity { id: EntityId },
    SetParameter { parameter: Parameter },
}

impl CadCommand {
    pub fn validate(&self, document: &CadDocument) -> Result<(), CommandError> {
        match self {
            Self::CreateLayer { layer } => {
                if layer.name.trim().is_empty() {
                    return Err(CommandError::InvalidLayer(
                        "layer name cannot be empty".into(),
                    ));
                }
                if document.layers.contains_key(&layer.id) {
                    return Err(CommandError::LayerExists(layer.id));
                }
                Ok(())
            }
            Self::CreateEntity { entity } => {
                if document.entities.contains_key(&entity.id) {
                    return Err(CommandError::EntityExists(entity.id));
                }
                validate_entity(entity, document)
            }
            Self::UpdateEntity { entity } => {
                if !document.entities.contains_key(&entity.id) {
                    return Err(CommandError::EntityMissing(entity.id));
                }
                validate_entity(entity, document)
            }
            Self::DeleteEntity { id } => {
                if document.entities.contains_key(id) {
                    Ok(())
                } else {
                    Err(CommandError::EntityMissing(*id))
                }
            }
            Self::SetParameter { parameter } => {
                if parameter.name.trim().is_empty() || !parameter.value.is_finite() {
                    return Err(CommandError::InvalidParameter(
                        "parameter name and value must be valid".into(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn apply(&self, document: &mut CadDocument) {
        match self {
            Self::CreateLayer { layer } => {
                document.next_layer_id = document.next_layer_id.max(layer.id + 1);
                document.layers.insert(layer.id, layer.clone());
            }
            Self::CreateEntity { entity } | Self::UpdateEntity { entity } => {
                document.next_entity_id = document.next_entity_id.max(entity.id + 1);
                document.entities.insert(entity.id, entity.clone());
            }
            Self::DeleteEntity { id } => {
                document.entities.remove(id);
            }
            Self::SetParameter { parameter } => {
                document.next_parameter_id = document.next_parameter_id.max(parameter.id + 1);
                document.parameters.insert(parameter.id, parameter.clone());
            }
        }
    }

    fn label(&self) -> String {
        match self {
            Self::CreateLayer { layer } => format!("Create layer {}", layer.name),
            Self::CreateEntity { entity } => format!("Create {}", entity.name),
            Self::UpdateEntity { entity } => format!("Update {}", entity.name),
            Self::DeleteEntity { id } => format!("Delete entity {id}"),
            Self::SetParameter { parameter } => format!("Set parameter {}", parameter.name),
        }
    }
}

fn validate_entity(entity: &Entity, document: &CadDocument) -> Result<(), CommandError> {
    if entity.name.trim().is_empty() {
        return Err(CommandError::InvalidGeometry(
            "entity name cannot be empty".into(),
        ));
    }
    if !document.layers.contains_key(&entity.layer) {
        return Err(CommandError::LayerMissing(entity.layer));
    }
    entity.kind.validate(document)
}

fn finite_point(point: Point2) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    LayerExists(LayerId),
    LayerMissing(LayerId),
    EntityExists(EntityId),
    EntityMissing(EntityId),
    InvalidLayer(String),
    InvalidGeometry(String),
    InvalidParameter(String),
    InvalidReference(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerExists(id) => write!(formatter, "layer {id} already exists"),
            Self::LayerMissing(id) => write!(formatter, "layer {id} does not exist"),
            Self::EntityExists(id) => write!(formatter, "entity {id} already exists"),
            Self::EntityMissing(id) => write!(formatter, "entity {id} does not exist"),
            Self::InvalidLayer(message)
            | Self::InvalidGeometry(message)
            | Self::InvalidParameter(message)
            | Self::InvalidReference(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for CommandError {}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentDiff {
    pub created_entities: Vec<EntityId>,
    pub updated_entities: Vec<EntityId>,
    pub deleted_entities: Vec<EntityId>,
    pub created_layers: Vec<LayerId>,
    pub updated_parameters: Vec<ParameterId>,
}

impl DocumentDiff {
    pub fn is_empty(&self) -> bool {
        self.created_entities.is_empty()
            && self.updated_entities.is_empty()
            && self.deleted_entities.is_empty()
            && self.created_layers.is_empty()
            && self.updated_parameters.is_empty()
    }

    pub fn summary(&self) -> String {
        let changes = self.created_entities.len()
            + self.updated_entities.len()
            + self.deleted_entities.len()
            + self.created_layers.len()
            + self.updated_parameters.len();
        format!("{changes} model changes")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandTransaction {
    pub commands: Vec<CadCommand>,
}

impl CommandTransaction {
    pub fn new(commands: Vec<CadCommand>) -> Self {
        Self { commands }
    }

    pub fn preview(&self, document: &CadDocument) -> Result<DocumentDiff, CommandError> {
        let mut temporary = document.clone();
        let mut diff = DocumentDiff::default();
        for command in &self.commands {
            command.validate(&temporary)?;
            collect_diff(&mut diff, command, &temporary);
            command.apply(&mut temporary);
        }
        Ok(diff)
    }

    pub fn apply(&self, document: &mut CadDocument) -> Result<DocumentDiff, CommandError> {
        let mut temporary = document.clone();
        let diff = self.preview(&temporary)?;
        for command in &self.commands {
            command.apply(&mut temporary);
        }
        *document = temporary;
        Ok(diff)
    }

    pub fn label(&self) -> String {
        match self.commands.as_slice() {
            [] => "No changes".into(),
            [command] => command.label(),
            commands => format!("{} actions", commands.len()),
        }
    }
}

fn collect_diff(diff: &mut DocumentDiff, command: &CadCommand, document: &CadDocument) {
    match command {
        CadCommand::CreateLayer { layer } => diff.created_layers.push(layer.id),
        CadCommand::CreateEntity { entity } => diff.created_entities.push(entity.id),
        CadCommand::UpdateEntity { entity } => {
            if document.entities.contains_key(&entity.id) {
                diff.updated_entities.push(entity.id);
            }
        }
        CadCommand::DeleteEntity { id } => diff.deleted_entities.push(*id),
        CadCommand::SetParameter { parameter } => diff.updated_parameters.push(parameter.id),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    Drafting,
    Mechanical,
    Architecture,
    Parameters,
    Import,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskAuthority {
    ReviewOnly,
    DirectWrite { capabilities: BTreeSet<Capability> },
}

impl TaskAuthority {
    pub fn all_direct() -> Self {
        Self::DirectWrite {
            capabilities: BTreeSet::from([
                Capability::Drafting,
                Capability::Mechanical,
                Capability::Architecture,
                Capability::Parameters,
                Capability::Import,
            ]),
        }
    }

    pub fn permits(&self, transaction: &CommandTransaction, document: &CadDocument) -> bool {
        let Self::DirectWrite { capabilities } = self else {
            return false;
        };
        let mut temporary = document.clone();
        transaction.commands.iter().all(|command| {
            let permitted = match command {
                CadCommand::CreateLayer { .. } => capabilities.contains(&Capability::Drafting),
                CadCommand::CreateEntity { entity } => {
                    permits_domain(capabilities, entity.kind.domain())
                }
                CadCommand::UpdateEntity { entity } => {
                    temporary.entities.get(&entity.id).is_some_and(|previous| {
                        permits_domain(capabilities, previous.kind.domain())
                            && permits_domain(capabilities, entity.kind.domain())
                    })
                }
                CadCommand::DeleteEntity { id } => temporary
                    .entities
                    .get(id)
                    .is_some_and(|entity| permits_domain(capabilities, entity.kind.domain())),
                CadCommand::SetParameter { .. } => capabilities.contains(&Capability::Parameters),
            };
            if permitted {
                command.apply(&mut temporary);
            }
            permitted
        })
    }
}

fn permits_domain(capabilities: &BTreeSet<Capability>, domain: Domain) -> bool {
    capabilities.contains(&match domain {
        Domain::Drafting => Capability::Drafting,
        Domain::Mechanical => Capability::Mechanical,
        Domain::Architecture => Capability::Architecture,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    Observed {
        entity_count: usize,
    },
    Planned {
        action_count: usize,
    },
    ToolCall {
        name: String,
        detail: String,
    },
    Committed {
        commit_id: CommitId,
        summary: String,
    },
    Validation {
        summary: String,
        passed: bool,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignTask {
    pub id: TaskId,
    pub title: String,
    pub goal: String,
    pub authority: TaskAuthority,
    pub status: TaskStatus,
    pub events: Vec<TaskEvent>,
    pub output_commits: Vec<CommitId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Passed,
    Warning,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub checks: Vec<CheckResult>,
}

impl ValidationReport {
    pub fn passed(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != CheckStatus::Failed)
    }

    pub fn summary(&self) -> String {
        let failures = self
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
            .count();
        if failures == 0 {
            format!("{} checks passed", self.checks.len())
        } else {
            format!("{failures} checks failed")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SemanticCommit {
    pub id: CommitId,
    pub parent: Option<CommitId>,
    pub task_id: Option<TaskId>,
    pub intent: String,
    pub transaction: CommandTransaction,
    pub diff: DocumentDiff,
    pub validation: ValidationReport,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub commit_id: CommitId,
    pub document: CadDocument,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesignBranch {
    pub name: String,
    pub head: CommitId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub commits: BTreeMap<CommitId, SemanticCommit>,
    pub snapshots: BTreeMap<CommitId, Snapshot>,
    pub branches: BTreeMap<String, DesignBranch>,
    pub active_branch: String,
    snapshot_interval: CommitId,
    next_commit_id: CommitId,
}

impl History {
    pub fn new(initial_document: CadDocument) -> Self {
        let root = SemanticCommit {
            id: 0,
            parent: None,
            task_id: None,
            intent: "Project created".into(),
            transaction: CommandTransaction::default(),
            diff: DocumentDiff::default(),
            validation: ValidationReport::default(),
        };
        let mut commits = BTreeMap::new();
        commits.insert(0, root);
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            0,
            Snapshot {
                commit_id: 0,
                document: initial_document,
            },
        );
        let mut branches = BTreeMap::new();
        branches.insert(
            "main".into(),
            DesignBranch {
                name: "main".into(),
                head: 0,
            },
        );
        Self {
            commits,
            snapshots,
            branches,
            active_branch: "main".into(),
            snapshot_interval: 4,
            next_commit_id: 1,
        }
    }

    pub fn head(&self) -> CommitId {
        self.branches[&self.active_branch].head
    }

    pub fn commit(
        &mut self,
        document: &CadDocument,
        task_id: Option<TaskId>,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
    ) -> Result<(CadDocument, CommitId), HistoryError> {
        let mut next_document = document.clone();
        let diff = transaction.apply(&mut next_document)?;
        let id = self.next_commit_id;
        self.next_commit_id += 1;
        let commit = SemanticCommit {
            id,
            parent: Some(self.head()),
            task_id,
            intent: intent.into(),
            transaction,
            diff,
            validation,
        };
        self.commits.insert(id, commit);
        self.branches
            .get_mut(&self.active_branch)
            .expect("active branch exists")
            .head = id;
        if id.is_multiple_of(self.snapshot_interval) {
            self.snapshots.insert(
                id,
                Snapshot {
                    commit_id: id,
                    document: next_document.clone(),
                },
            );
        }
        Ok((next_document, id))
    }

    pub fn create_branch(
        &mut self,
        name: impl Into<String>,
        from: CommitId,
    ) -> Result<(), HistoryError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(HistoryError::InvalidBranchName);
        }
        if self.branches.contains_key(&name) {
            return Err(HistoryError::BranchExists(name));
        }
        if !self.commits.contains_key(&from) {
            return Err(HistoryError::CommitMissing(from));
        }
        self.branches
            .insert(name.clone(), DesignBranch { name, head: from });
        Ok(())
    }

    pub fn checkout_branch(&mut self, name: &str) -> Result<CadDocument, HistoryError> {
        let branch = self
            .branches
            .get(name)
            .ok_or_else(|| HistoryError::BranchMissing(name.into()))?;
        let document = self.restore(branch.head)?;
        self.active_branch = name.into();
        Ok(document)
    }

    pub fn restore(&self, target: CommitId) -> Result<CadDocument, HistoryError> {
        if !self.commits.contains_key(&target) {
            return Err(HistoryError::CommitMissing(target));
        }
        let mut replay = Vec::new();
        let mut cursor = target;
        let snapshot = loop {
            if let Some(snapshot) = self.snapshots.get(&cursor) {
                break snapshot;
            }
            let commit = self.commits.get(&cursor).expect("verified commit exists");
            replay.push(cursor);
            cursor = commit.parent.expect("root is always snapshotted");
        };
        let mut document = snapshot.document.clone();
        for commit_id in replay.into_iter().rev() {
            let commit = self.commits.get(&commit_id).expect("replay commit exists");
            commit.transaction.apply(&mut document)?;
        }
        Ok(document)
    }

    pub fn ordered_commits(&self) -> Vec<&SemanticCommit> {
        self.commits.values().collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryError {
    Command(CommandError),
    CommitMissing(CommitId),
    BranchExists(String),
    BranchMissing(String),
    InvalidBranchName,
}

impl From<CommandError> for HistoryError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::CommitMissing(id) => write!(formatter, "commit {id} does not exist"),
            Self::BranchExists(name) => write!(formatter, "branch {name} already exists"),
            Self::BranchMissing(name) => write!(formatter, "branch {name} does not exist"),
            Self::InvalidBranchName => formatter.write_str("branch name cannot be empty"),
        }
    }
}

impl std::error::Error for HistoryError {}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskWorkspace {
    pub document: CadDocument,
    pub history: History,
    pub tasks: BTreeMap<TaskId, DesignTask>,
    next_task_id: TaskId,
}

impl TaskWorkspace {
    pub fn new(document: CadDocument) -> Self {
        Self {
            history: History::new(document.clone()),
            document,
            tasks: BTreeMap::new(),
            next_task_id: 1,
        }
    }

    pub fn create_task(
        &mut self,
        title: impl Into<String>,
        goal: impl Into<String>,
        authority: TaskAuthority,
    ) -> TaskId {
        let id = self.next_task_id;
        self.next_task_id += 1;
        self.tasks.insert(
            id,
            DesignTask {
                id,
                title: title.into(),
                goal: goal.into(),
                authority,
                status: TaskStatus::Queued,
                events: Vec::new(),
                output_commits: Vec::new(),
            },
        );
        id
    }

    pub fn begin_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        task.status = TaskStatus::Running;
        task.events.push(TaskEvent::Observed {
            entity_count: self.document.entities.len(),
        });
        Ok(())
    }

    pub fn record_event(
        &mut self,
        task_id: TaskId,
        event: TaskEvent,
    ) -> Result<(), WorkspaceError> {
        self.tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?
            .events
            .push(event);
        Ok(())
    }

    pub fn apply_task_transaction(
        &mut self,
        task_id: TaskId,
        intent: impl Into<String>,
        transaction: CommandTransaction,
        validation: ValidationReport,
    ) -> Result<CommitId, WorkspaceError> {
        let task = self
            .tasks
            .get(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        if !task.authority.permits(&transaction, &self.document) {
            return Err(WorkspaceError::Unauthorized(task_id));
        }
        let intent = intent.into();
        let (document, commit_id) = self.history.commit(
            &self.document,
            Some(task_id),
            intent.clone(),
            transaction,
            validation.clone(),
        )?;
        self.document = document;
        let task = self.tasks.get_mut(&task_id).expect("task checked above");
        task.output_commits.push(commit_id);
        task.events.push(TaskEvent::Committed {
            commit_id,
            summary: intent,
        });
        task.events.push(TaskEvent::Validation {
            summary: validation.summary(),
            passed: validation.passed(),
        });
        Ok(commit_id)
    }

    pub fn complete_task(&mut self, task_id: TaskId) -> Result<(), WorkspaceError> {
        self.tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?
            .status = TaskStatus::Completed;
        Ok(())
    }

    pub fn fail_task(
        &mut self,
        task_id: TaskId,
        message: impl Into<String>,
    ) -> Result<(), WorkspaceError> {
        let task = self
            .tasks
            .get_mut(&task_id)
            .ok_or(WorkspaceError::TaskMissing(task_id))?;
        task.status = TaskStatus::Failed;
        task.events.push(TaskEvent::Failed {
            message: message.into(),
        });
        Ok(())
    }

    pub fn fork_at(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        self.history.create_branch(name, commit_id)?;
        Ok(())
    }

    pub fn checkout_branch(&mut self, name: &str) -> Result<(), WorkspaceError> {
        self.document = self.history.checkout_branch(name)?;
        Ok(())
    }

    pub fn checkout_as_branch(
        &mut self,
        name: impl Into<String>,
        commit_id: CommitId,
    ) -> Result<(), WorkspaceError> {
        let name = name.into();
        if !self.history.branches.contains_key(&name) {
            self.history.create_branch(name.clone(), commit_id)?;
        }
        self.checkout_branch(&name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    History(HistoryError),
    TaskMissing(TaskId),
    Unauthorized(TaskId),
}

impl From<HistoryError> for WorkspaceError {
    fn from(error: HistoryError) -> Self {
        Self::History(error)
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::History(error) => error.fmt(formatter),
            Self::TaskMissing(id) => write!(formatter, "task {id} does not exist"),
            Self::Unauthorized(id) => write!(
                formatter,
                "task {id} is not authorized to write this change"
            ),
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rectangle(id: EntityId) -> Entity {
        Entity {
            id,
            layer: 1,
            name: "Base plate".into(),
            visible: true,
            kind: EntityKind::Rectangle {
                origin: Point2::new(0.0, 0.0),
                width: 80.0,
                height: 50.0,
            },
            parameter_refs: BTreeSet::new(),
        }
    }

    #[test]
    fn transactions_are_atomic_when_later_commands_fail() {
        let mut document = CadDocument::new("Atomic");
        let transaction = CommandTransaction::new(vec![
            CadCommand::CreateEntity {
                entity: rectangle(1),
            },
            CadCommand::CreateEntity {
                entity: Entity {
                    id: 2,
                    layer: 1,
                    name: "Invalid".into(),
                    visible: true,
                    kind: EntityKind::Circle {
                        center: Point2::new(0.0, 0.0),
                        radius: 0.0,
                    },
                    parameter_refs: BTreeSet::new(),
                },
            },
        ]);

        assert!(transaction.apply(&mut document).is_err());
        assert!(document.entities.is_empty());
    }

    #[test]
    fn snapshots_and_replay_restore_the_same_document() {
        let document = CadDocument::new("History");
        let mut history = History::new(document.clone());
        let mut current = document;
        for id in 1..=5 {
            let (next, _) = history
                .commit(
                    &current,
                    None,
                    "Add geometry",
                    CommandTransaction::new(vec![CadCommand::CreateEntity {
                        entity: rectangle(id),
                    }]),
                    ValidationReport::default(),
                )
                .unwrap();
            current = next;
        }

        assert!(history.snapshots.contains_key(&4));
        assert_eq!(history.restore(5).unwrap(), current);
        assert_eq!(history.restore(2).unwrap().entities.len(), 2);
    }

    #[test]
    fn branch_heads_are_isolated() {
        let document = CadDocument::new("Branching");
        let mut history = History::new(document.clone());
        let (main_document, first) = history
            .commit(
                &document,
                None,
                "Add base",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(1),
                }]),
                ValidationReport::default(),
            )
            .unwrap();
        history.create_branch("alternative", first).unwrap();
        let (_, main_head) = history
            .commit(
                &main_document,
                None,
                "Main option",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(2),
                }]),
                ValidationReport::default(),
            )
            .unwrap();
        let alternative_document = history.checkout_branch("alternative").unwrap();
        let (_, alternative_head) = history
            .commit(
                &alternative_document,
                None,
                "Alternative option",
                CommandTransaction::new(vec![CadCommand::CreateEntity {
                    entity: rectangle(3),
                }]),
                ValidationReport::default(),
            )
            .unwrap();

        assert_ne!(main_head, alternative_head);
        assert_eq!(
            history
                .restore(main_head)
                .unwrap()
                .entities
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            history
                .restore(alternative_head)
                .unwrap()
                .entities
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn review_only_tasks_cannot_mutate_the_document() {
        let mut workspace = TaskWorkspace::new(CadDocument::new("Protected"));
        let task = workspace.create_task("Inspect", "Review the plate", TaskAuthority::ReviewOnly);
        let result = workspace.apply_task_transaction(
            task,
            "Add plate",
            CommandTransaction::new(vec![CadCommand::CreateEntity {
                entity: rectangle(1),
            }]),
            ValidationReport::default(),
        );
        assert_eq!(result, Err(WorkspaceError::Unauthorized(task)));
        assert!(workspace.document.entities.is_empty());
    }

    #[test]
    fn scoped_authority_cannot_delete_another_domain_entity() {
        let mut document = CadDocument::new("Scoped");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: Entity {
                id: 1,
                layer: 1,
                name: "Room".into(),
                visible: true,
                kind: EntityKind::Room {
                    boundary: vec![
                        Point2::new(0.0, 0.0),
                        Point2::new(10.0, 0.0),
                        Point2::new(0.0, 10.0),
                    ],
                    area: 50.0,
                },
                parameter_refs: BTreeSet::new(),
            },
        }])
        .apply(&mut document)
        .unwrap();
        let authority = TaskAuthority::DirectWrite {
            capabilities: BTreeSet::from([Capability::Mechanical]),
        };

        assert!(!authority.permits(
            &CommandTransaction::new(vec![CadCommand::DeleteEntity { id: 1 }]),
            &document
        ));
    }
}
