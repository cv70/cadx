//! Bounded publish-subscribe event log shared by every core adapter.
//!
//! [`EventDispatcher`] is independent of the command bus: it fans one
//! [`CoreEvent`] out to live subscribers and retains a bounded replay log.

use std::collections::VecDeque;

use cadx_core::domain::FeatureId;

use crate::{DocumentDiff, DocumentState};

use super::{StreamId, TransactionId, TransactionSource};

pub const DEFAULT_EVENT_LOG_LIMIT: usize = 256;

type EventSubscriber = Box<dyn FnMut(&CoreEvent) + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    TransactionStarted {
        transaction_id: TransactionId,
        source: TransactionSource,
        label: String,
        command_count: usize,
    },
    TransactionCommitted {
        transaction_id: TransactionId,
        source: TransactionSource,
        label: String,
        revision: u64,
        state: DocumentState,
        command_count: usize,
        created_features: Vec<FeatureId>,
    },
    TransactionRejected {
        transaction_id: TransactionId,
        source: TransactionSource,
        label: String,
        command_count: usize,
        error: String,
    },
    PreviewPrepared {
        source: TransactionSource,
        label: String,
        base_revision: u64,
        command_count: usize,
        diff: DocumentDiff,
    },
    PreviewDiscarded {
        source: TransactionSource,
        label: String,
        base_revision: u64,
        command_count: usize,
    },
    UndoApplied {
        revision: u64,
        state: DocumentState,
    },
    RedoApplied {
        revision: u64,
        state: DocumentState,
    },
    DocumentReplaced {
        revision: u64,
        state: DocumentState,
    },
    DocumentSaved {
        revision: u64,
        state: DocumentState,
    },
    StreamStarted {
        stream_id: StreamId,
        source: TransactionSource,
        label: String,
    },
    StreamCommandBuffered {
        stream_id: StreamId,
        command_count: usize,
    },
    StreamCommitted {
        stream_id: StreamId,
        transaction_id: TransactionId,
        revision: u64,
        command_count: usize,
        created_features: Vec<FeatureId>,
    },
    StreamCanceled {
        stream_id: StreamId,
        discarded_command_count: usize,
    },
}

#[derive(Default)]
pub struct EventDispatcher {
    subscribers: Vec<EventSubscriber>,
    events: VecDeque<CoreEvent>,
    event_log_limit: usize,
}

impl EventDispatcher {
    #[must_use]
    pub fn new(event_log_limit: usize) -> Self {
        Self {
            subscribers: Vec::new(),
            events: VecDeque::new(),
            event_log_limit,
        }
    }

    pub fn subscribe(&mut self, subscriber: impl FnMut(&CoreEvent) + Send + 'static) {
        self.subscribers.push(Box::new(subscriber));
    }

    pub fn publish(&mut self, event: CoreEvent) {
        for subscriber in &mut self.subscribers {
            subscriber(&event);
        }
        if self.event_log_limit == 0 {
            return;
        }
        if self.events.len() == self.event_log_limit {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn drain(&mut self) -> Vec<CoreEvent> {
        self.events.drain(..).collect()
    }
}
