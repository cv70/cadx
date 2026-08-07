//! Observable command bus: source tagging, streams, and event publication.

use std::{ops::Deref, sync::Arc};

use cadx_core::{
    domain::{CadDocument, ModelCommand},
    kernel::CadKernel,
};
use thiserror::Error;

use crate::{DocumentSession, SessionError, TransactionOutcome, TransactionPreview};

mod event;

pub use event::{CoreEvent, DEFAULT_EVENT_LOG_LIMIT, EventDispatcher};

pub type StreamId = u64;
pub type TransactionId = u64;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionSource {
    Ui,
    Ai,
    DomainPack,
    Import,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionMetadata {
    pub source: TransactionSource,
    pub label: String,
}

impl TransactionMetadata {
    #[must_use]
    pub fn new(source: TransactionSource, label: impl Into<String>) -> Self {
        Self {
            source,
            label: label.into(),
        }
    }

    #[must_use]
    pub fn from_commands(source: TransactionSource, commands: &[ModelCommand]) -> Self {
        let label = match commands {
            [] => "No-op transaction".into(),
            [command] => command.label().into(),
            [first, ..] => format!("{} batch", first.label()),
        };
        Self { source, label }
    }
}
impl Default for TransactionMetadata {
    fn default() -> Self {
        Self::new(TransactionSource::Ui, "Transaction")
    }
}

#[derive(Debug, Error)]
pub enum CoreBusError {
    #[error("a command stream is already active")]
    StreamAlreadyActive,
    #[error("command stream {0} is not active")]
    StreamNotActive(StreamId),
    #[error(transparent)]
    Session(#[from] SessionError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandStream {
    pub id: StreamId,
    pub source: TransactionSource,
    pub label: String,
    pub commands: Vec<ModelCommand>,
}

/// Observable command bus shared by UI, AI, domain packs, and import flows.
///
/// `CoreBus` deliberately delegates all document mutation to
/// [`DocumentSession`]. Its job is the architecture-level boundary around that
/// session: source tagging, command stream buffering, and publish-subscribe
/// events for render, analysis, and agent adapters.
pub struct CoreBus {
    session: DocumentSession,
    dispatcher: EventDispatcher,
    active_stream: Option<CommandStream>,
    next_stream_id: StreamId,
    next_transaction_id: TransactionId,
}

impl CoreBus {
    /// Creates a bus around a clean, kernel-evaluated document session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the kernel rejects the initial document.
    pub fn new(kernel: Arc<dyn CadKernel>, document: CadDocument) -> Result<Self, SessionError> {
        Self::with_history_limit(kernel, document, crate::DEFAULT_HISTORY_LIMIT)
    }

    /// Creates a bus with an explicit undo-history limit.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the kernel rejects the initial document.
    pub fn with_history_limit(
        kernel: Arc<dyn CadKernel>,
        document: CadDocument,
        history_limit: usize,
    ) -> Result<Self, SessionError> {
        Ok(Self {
            session: DocumentSession::with_history_limit(kernel, document, history_limit)?,
            dispatcher: EventDispatcher::new(DEFAULT_EVENT_LOG_LIMIT),
            active_stream: None,
            next_stream_id: 1,
            next_transaction_id: 1,
        })
    }

    #[must_use]
    pub const fn session(&self) -> &DocumentSession {
        &self.session
    }

    pub fn subscribe(&mut self, subscriber: impl FnMut(&CoreEvent) + Send + 'static) {
        self.dispatcher.subscribe(subscriber);
    }

    #[must_use]
    pub fn drain_events(&mut self) -> Vec<CoreEvent> {
        self.dispatcher.drain()
    }

    /// Executes a UI-originated command batch.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the active document when
    /// document validation or kernel evaluation fails.
    pub fn execute(
        &mut self,
        commands: Vec<ModelCommand>,
    ) -> Result<TransactionOutcome, SessionError> {
        self.execute_with_source(commands, TransactionSource::Ui)
    }

    /// Executes a command batch with an explicit source tag.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the active document when
    /// document validation or kernel evaluation fails.
    pub fn execute_with_source(
        &mut self,
        commands: Vec<ModelCommand>,
        source: TransactionSource,
    ) -> Result<TransactionOutcome, SessionError> {
        let metadata = TransactionMetadata::from_commands(source, &commands);
        self.execute_with_metadata(commands, metadata)
    }

    /// Executes a command batch with caller-provided transaction metadata.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the active document when
    /// document validation or kernel evaluation fails.
    pub fn execute_with_metadata(
        &mut self,
        commands: Vec<ModelCommand>,
        metadata: TransactionMetadata,
    ) -> Result<TransactionOutcome, SessionError> {
        if commands.is_empty() {
            return self.session.execute(commands);
        }

        let transaction_id = self.allocate_transaction_id();
        let command_count = commands.len();
        self.dispatcher.publish(CoreEvent::TransactionStarted {
            transaction_id,
            source: metadata.source,
            label: metadata.label.clone(),
            command_count,
        });

        match self.session.execute(commands) {
            Ok(outcome) => {
                self.dispatcher.publish(CoreEvent::TransactionCommitted {
                    transaction_id,
                    source: metadata.source,
                    label: metadata.label,
                    revision: self.session.revision(),
                    state: self.session.state(),
                    command_count,
                    created_features: outcome.created_features.clone(),
                });
                Ok(outcome)
            }
            Err(error) => {
                self.dispatcher.publish(CoreEvent::TransactionRejected {
                    transaction_id,
                    source: metadata.source,
                    label: metadata.label,
                    command_count,
                    error: error.to_string(),
                });
                Err(error)
            }
        }
    }

    /// Evaluates a command batch in an isolated copy-on-write sandbox.
    ///
    /// The active document, evaluated scene, revision, dirty state, and history
    /// are unchanged. The returned preview is bound to the active revision.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when document or kernel validation fails.
    pub fn preview_with_source(
        &mut self,
        commands: &[ModelCommand],
        source: TransactionSource,
    ) -> Result<TransactionPreview, SessionError> {
        let metadata = TransactionMetadata::from_commands(source, commands);
        self.preview_with_metadata(commands, metadata)
    }

    /// Evaluates a command batch in a sandbox with caller-provided metadata.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when document or kernel validation fails.
    pub fn preview_with_metadata(
        &mut self,
        commands: &[ModelCommand],
        metadata: TransactionMetadata,
    ) -> Result<TransactionPreview, SessionError> {
        let preview = self.session.preview(commands)?;
        self.dispatcher.publish(CoreEvent::PreviewPrepared {
            source: metadata.source,
            label: metadata.label,
            base_revision: preview.base_revision(),
            command_count: preview.command_count(),
            diff: preview.diff().clone(),
        });
        Ok(preview)
    }

    /// Commits a kernel-validated preview as one observable transaction.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::StalePreview`] when the active revision changed
    /// after preview evaluation. No document state is changed on failure.
    pub fn commit_preview_with_metadata(
        &mut self,
        preview: TransactionPreview,
        metadata: TransactionMetadata,
    ) -> Result<TransactionOutcome, SessionError> {
        let transaction_id = self.allocate_transaction_id();
        let command_count = preview.command_count();
        self.dispatcher.publish(CoreEvent::TransactionStarted {
            transaction_id,
            source: metadata.source,
            label: metadata.label.clone(),
            command_count,
        });
        match self.session.commit_preview(preview) {
            Ok(outcome) => {
                self.dispatcher.publish(CoreEvent::TransactionCommitted {
                    transaction_id,
                    source: metadata.source,
                    label: metadata.label,
                    revision: self.session.revision(),
                    state: self.session.state(),
                    command_count,
                    created_features: outcome.created_features.clone(),
                });
                Ok(outcome)
            }
            Err(error) => {
                self.dispatcher.publish(CoreEvent::TransactionRejected {
                    transaction_id,
                    source: metadata.source,
                    label: metadata.label,
                    command_count,
                    error: error.to_string(),
                });
                Err(error)
            }
        }
    }

    /// Records explicit rejection of a preview. The live session is unchanged.
    pub fn discard_preview(&mut self, preview: TransactionPreview, metadata: TransactionMetadata) {
        let base_revision = preview.base_revision();
        let command_count = preview.command_count();
        drop(preview);
        self.dispatcher.publish(CoreEvent::PreviewDiscarded {
            source: metadata.source,
            label: metadata.label,
            base_revision,
            command_count,
        });
    }

    /// Replaces the active document and clears undo/redo history.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing the active document when
    /// kernel evaluation fails.
    pub fn replace_document(&mut self, document: CadDocument) -> Result<(), SessionError> {
        self.session.replace_document(document)?;
        self.dispatcher.publish(CoreEvent::DocumentReplaced {
            revision: self.session.revision(),
            state: self.session.state(),
        });
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.session.mark_saved();
        self.dispatcher.publish(CoreEvent::DocumentSaved {
            revision: self.session.revision(),
            state: self.session.state(),
        });
    }

    /// Restores the previous revision and publishes an undo event.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing history when evaluation fails.
    pub fn undo(&mut self) -> Result<bool, SessionError> {
        let applied = self.session.undo()?;
        if applied {
            self.dispatcher.publish(CoreEvent::UndoApplied {
                revision: self.session.revision(),
                state: self.session.state(),
            });
        }
        Ok(applied)
    }

    /// Restores the next revision and publishes a redo event.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] without changing history when evaluation fails.
    pub fn redo(&mut self) -> Result<bool, SessionError> {
        let applied = self.session.redo()?;
        if applied {
            self.dispatcher.publish(CoreEvent::RedoApplied {
                revision: self.session.revision(),
                state: self.session.state(),
            });
        }
        Ok(applied)
    }

    /// Starts buffering streamable commands for later atomic commit.
    ///
    /// # Errors
    ///
    /// Returns [`CoreBusError::StreamAlreadyActive`] when another stream is
    /// already open.
    pub fn begin_stream(
        &mut self,
        source: TransactionSource,
        label: impl Into<String>,
    ) -> Result<StreamId, CoreBusError> {
        if self.active_stream.is_some() {
            return Err(CoreBusError::StreamAlreadyActive);
        }
        let id = self.allocate_stream_id();
        let label = label.into();
        self.active_stream = Some(CommandStream {
            id,
            source,
            label: label.clone(),
            commands: Vec::new(),
        });
        self.dispatcher.publish(CoreEvent::StreamStarted {
            stream_id: id,
            source,
            label,
        });
        Ok(id)
    }

    /// Buffers one command into an active command stream.
    ///
    /// # Errors
    ///
    /// Returns [`CoreBusError::StreamNotActive`] when `stream_id` does not match
    /// the active stream.
    pub fn push_stream_command(
        &mut self,
        stream_id: StreamId,
        command: ModelCommand,
    ) -> Result<usize, CoreBusError> {
        let Some(stream) = self
            .active_stream
            .as_mut()
            .filter(|stream| stream.id == stream_id)
        else {
            return Err(CoreBusError::StreamNotActive(stream_id));
        };
        stream.commands.push(command);
        let command_count = stream.commands.len();
        self.dispatcher.publish(CoreEvent::StreamCommandBuffered {
            stream_id,
            command_count,
        });
        Ok(command_count)
    }

    /// Atomically validates and commits all commands buffered in a stream.
    ///
    /// # Errors
    ///
    /// Returns [`CoreBusError`] when the stream id is invalid or the staged
    /// transaction fails document/kernel validation.
    pub fn commit_stream(
        &mut self,
        stream_id: StreamId,
    ) -> Result<TransactionOutcome, CoreBusError> {
        let stream = self.take_stream(stream_id)?;
        let transaction_id = self.allocate_transaction_id();
        let command_count = stream.commands.len();
        self.dispatcher.publish(CoreEvent::TransactionStarted {
            transaction_id,
            source: stream.source,
            label: stream.label.clone(),
            command_count,
        });

        match self.session.execute(stream.commands) {
            Ok(outcome) => {
                self.dispatcher.publish(CoreEvent::TransactionCommitted {
                    transaction_id,
                    source: stream.source,
                    label: stream.label,
                    revision: self.session.revision(),
                    state: self.session.state(),
                    command_count,
                    created_features: outcome.created_features.clone(),
                });
                self.dispatcher.publish(CoreEvent::StreamCommitted {
                    stream_id,
                    transaction_id,
                    revision: self.session.revision(),
                    command_count,
                    created_features: outcome.created_features.clone(),
                });
                Ok(outcome)
            }
            Err(error) => {
                self.dispatcher.publish(CoreEvent::TransactionRejected {
                    transaction_id,
                    source: stream.source,
                    label: stream.label,
                    command_count,
                    error: error.to_string(),
                });
                Err(error.into())
            }
        }
    }

    /// Discards an active command stream.
    ///
    /// # Errors
    ///
    /// Returns [`CoreBusError::StreamNotActive`] when `stream_id` does not match
    /// the active stream.
    pub fn cancel_stream(&mut self, stream_id: StreamId) -> Result<(), CoreBusError> {
        let stream = self.take_stream(stream_id)?;
        self.dispatcher.publish(CoreEvent::StreamCanceled {
            stream_id,
            discarded_command_count: stream.commands.len(),
        });
        Ok(())
    }

    #[must_use]
    pub fn active_stream(&self) -> Option<&CommandStream> {
        self.active_stream.as_ref()
    }

    fn take_stream(&mut self, stream_id: StreamId) -> Result<CommandStream, CoreBusError> {
        match self.active_stream.take() {
            Some(stream) if stream.id == stream_id => Ok(stream),
            Some(stream) => {
                self.active_stream = Some(stream);
                Err(CoreBusError::StreamNotActive(stream_id))
            }
            None => Err(CoreBusError::StreamNotActive(stream_id)),
        }
    }

    fn allocate_stream_id(&mut self) -> StreamId {
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.saturating_add(1);
        id
    }

    fn allocate_transaction_id(&mut self) -> TransactionId {
        let id = self.next_transaction_id;
        self.next_transaction_id = self.next_transaction_id.saturating_add(1);
        id
    }
}

impl Deref for CoreBus {
    type Target = DocumentSession;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentState;
    use cadx_core::kernel::{EvaluatedScene, KernelError};

    #[derive(Debug)]
    struct AcceptKernel;

    impl CadKernel for AcceptKernel {
        fn name(&self) -> &'static str {
            "accept"
        }

        fn evaluate(&self, _document: &CadDocument) -> Result<EvaluatedScene, KernelError> {
            Ok(EvaluatedScene::default())
        }
    }

    fn test_bus() -> CoreBus {
        CoreBus::new(Arc::new(AcceptKernel), CadDocument::default()).unwrap()
    }

    fn create_box(name: &str) -> ModelCommand {
        ModelCommand::CreateBox {
            name: name.into(),
            size: [10.0, 10.0, 10.0],
            position: [0.0, 0.0, 0.0],
        }
    }

    #[test]
    fn publishes_transaction_and_undo_events() {
        let mut bus = test_bus();

        let outcome = bus
            .execute_with_source(vec![create_box("part")], TransactionSource::Ui)
            .unwrap();
        assert_eq!(outcome.created_features, vec![1]);

        let events = bus.drain_events();
        assert!(matches!(
            &events[..],
            [
                CoreEvent::TransactionStarted {
                    source: TransactionSource::Ui,
                    command_count: 1,
                    ..
                },
                CoreEvent::TransactionCommitted {
                    source: TransactionSource::Ui,
                    revision: 2,
                    state: DocumentState::Dirty,
                    created_features,
                    ..
                }
            ] if created_features == &vec![1]
        ));

        assert!(bus.undo().unwrap());
        assert!(matches!(
            bus.drain_events().as_slice(),
            [CoreEvent::UndoApplied {
                revision: 1,
                state: DocumentState::Clean
            }]
        ));
    }

    #[test]
    fn stream_commits_buffered_ai_commands_as_one_transaction() {
        let mut bus = test_bus();

        let stream_id = bus
            .begin_stream(TransactionSource::Ai, "AI preview")
            .unwrap();
        assert_eq!(
            bus.push_stream_command(stream_id, create_box("first"))
                .unwrap(),
            1
        );
        assert_eq!(
            bus.push_stream_command(stream_id, create_box("second"))
                .unwrap(),
            2
        );

        let outcome = bus.commit_stream(stream_id).unwrap();
        assert_eq!(outcome.created_features, vec![1, 2]);
        assert!(bus.active_stream().is_none());

        let events = bus.drain_events();
        assert_eq!(events.len(), 6);
        assert!(matches!(
            events.last(),
            Some(CoreEvent::StreamCommitted {
                stream_id: id,
                revision: 2,
                command_count: 2,
                created_features,
                ..
            }) if *id == stream_id && created_features == &vec![1, 2]
        ));
    }

    #[test]
    fn preview_publishes_diff_and_commit_events_without_early_mutation() {
        let mut bus = test_bus();
        let preview = bus
            .preview_with_source(&[create_box("candidate")], TransactionSource::Ai)
            .unwrap();
        assert!(bus.document().features.is_empty());
        assert_eq!(bus.revision(), 1);
        assert!(matches!(
            bus.drain_events().as_slice(),
            [CoreEvent::PreviewPrepared {
                source: TransactionSource::Ai,
                base_revision: 1,
                command_count: 1,
                diff,
                ..
            }] if diff.added_features.len() == 1 && diff.added_features[0].id == 1
        ));

        bus.commit_preview_with_metadata(
            preview,
            TransactionMetadata::new(TransactionSource::Ai, "candidate"),
        )
        .unwrap();
        assert_eq!(bus.document().features.len(), 1);
        assert!(matches!(
            bus.drain_events().as_slice(),
            [
                CoreEvent::TransactionStarted {
                    source: TransactionSource::Ai,
                    command_count: 1,
                    ..
                },
                CoreEvent::TransactionCommitted {
                    source: TransactionSource::Ai,
                    revision: 2,
                    created_features,
                    ..
                }
            ] if created_features == &vec![1]
        ));
    }

    #[test]
    fn discarding_preview_is_observable_and_does_not_mutate_document() {
        let mut bus = test_bus();
        let preview = bus
            .preview_with_source(&[create_box("candidate")], TransactionSource::Ai)
            .unwrap();
        let _ = bus.drain_events();
        bus.discard_preview(
            preview,
            TransactionMetadata::new(TransactionSource::Ai, "candidate"),
        );

        assert!(bus.document().features.is_empty());
        assert_eq!(bus.revision(), 1);
        assert!(matches!(
            bus.drain_events().as_slice(),
            [CoreEvent::PreviewDiscarded {
                source: TransactionSource::Ai,
                base_revision: 1,
                command_count: 1,
                ..
            }]
        ));
    }
}
