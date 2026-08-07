//! Copy-on-write staged transactions and their structural document diff.

use std::collections::BTreeSet;

use cadx_core::{
    assembly::AssemblyId,
    domain::{CadDocument, FeatureId},
    kernel::EvaluatedScene,
};

/// Stable identity and display name for one feature affected by a preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureChange {
    pub id: FeatureId,
    pub name: String,
}

/// Structural difference between the active document and a staged transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentDiff {
    pub added_features: Vec<FeatureChange>,
    pub modified_features: Vec<FeatureChange>,
    pub removed_features: Vec<FeatureChange>,
    pub changed_assemblies: Vec<AssemblyId>,
    pub changed_domain_namespaces: Vec<String>,
    pub document_name_changed: bool,
}

impl DocumentDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added_features.is_empty()
            && self.modified_features.is_empty()
            && self.removed_features.is_empty()
            && self.changed_assemblies.is_empty()
            && self.changed_domain_namespaces.is_empty()
            && !self.document_name_changed
    }
}

/// Kernel-validated copy-on-write result that has not mutated the live session.
///
/// Fields are private so callers cannot manufacture a preview without passing
/// through document validation and the active CAD kernel.
#[derive(Debug)]
pub struct TransactionPreview {
    pub(super) base_revision: u64,
    pub(super) command_count: usize,
    pub(super) document: CadDocument,
    pub(super) scene: EvaluatedScene,
    pub(super) created_features: Vec<FeatureId>,
    pub(super) diff: DocumentDiff,
}

impl TransactionPreview {
    #[must_use]
    pub const fn base_revision(&self) -> u64 {
        self.base_revision
    }

    #[must_use]
    pub const fn command_count(&self) -> usize {
        self.command_count
    }

    #[must_use]
    pub const fn document(&self) -> &CadDocument {
        &self.document
    }

    #[must_use]
    pub const fn scene(&self) -> &EvaluatedScene {
        &self.scene
    }

    #[must_use]
    pub fn created_features(&self) -> &[FeatureId] {
        &self.created_features
    }

    #[must_use]
    pub const fn diff(&self) -> &DocumentDiff {
        &self.diff
    }
}

pub(super) fn document_diff(before: &CadDocument, after: &CadDocument) -> DocumentDiff {
    let mut diff = DocumentDiff {
        document_name_changed: before.name != after.name,
        ..DocumentDiff::default()
    };
    for feature in &after.features {
        match before.feature(feature.id) {
            None => diff.added_features.push(FeatureChange {
                id: feature.id,
                name: feature.name.clone(),
            }),
            Some(previous) if previous != feature => {
                diff.modified_features.push(FeatureChange {
                    id: feature.id,
                    name: feature.name.clone(),
                });
            }
            Some(_) => {}
        }
    }
    for feature in &before.features {
        if after.feature(feature.id).is_none() {
            diff.removed_features.push(FeatureChange {
                id: feature.id,
                name: feature.name.clone(),
            });
        }
    }

    let assembly_ids = before
        .assemblies
        .iter()
        .map(|assembly| assembly.id)
        .chain(after.assemblies.iter().map(|assembly| assembly.id))
        .collect::<BTreeSet<_>>();
    diff.changed_assemblies = assembly_ids
        .into_iter()
        .filter(|id| before.assembly(*id) != after.assembly(*id))
        .collect();

    let namespaces = before
        .domain_data
        .keys()
        .chain(after.domain_data.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    diff.changed_domain_namespaces = namespaces
        .into_iter()
        .filter(|namespace| before.domain_data.get(namespace) != after.domain_data.get(namespace))
        .collect();
    diff
}

#[cfg(test)]
mod tests {
    use crate::{DocumentState, SessionError};
    use cadx_core::domain::ModelCommand;

    use super::super::test_support::{create_box, session};

    #[test]
    fn preview_is_copy_on_write_and_commits_as_one_undoable_revision() {
        let mut session = session();
        let document_before = session.document().clone();
        let scene_before = session.scene().clone();
        let revision_before = session.revision();

        let preview = session
            .preview(&[
                create_box("previewed body"),
                ModelCommand::SetDomainData {
                    namespace: "mcad.intent".into(),
                    entity_key: "body-1".into(),
                    value: serde_json::json!({"purpose": "fixture"}),
                },
            ])
            .unwrap();

        assert_eq!(session.document(), &document_before);
        assert_eq!(session.scene(), &scene_before);
        assert_eq!(session.revision(), revision_before);
        assert_eq!(session.state(), DocumentState::Clean);
        assert!(!session.can_undo());
        assert_eq!(preview.created_features(), &[1]);
        assert_eq!(preview.diff().added_features[0].name, "previewed body");
        assert_eq!(
            preview.diff().changed_domain_namespaces,
            ["mcad.intent".to_owned()]
        );

        let outcome = session.commit_preview(preview).unwrap();
        assert_eq!(outcome.created_features, [1]);
        assert_eq!(session.revision(), revision_before + 1);
        assert_eq!(session.document().features.len(), 1);
        assert!(session.can_undo());
        assert!(session.undo().unwrap());
        assert_eq!(session.document(), &document_before);
    }

    #[test]
    fn preview_diff_classifies_added_modified_and_removed_features() {
        let mut session = session();
        let first = session
            .execute(vec![create_box("first")])
            .unwrap()
            .created_features[0];
        let second = session
            .execute(vec![create_box("second")])
            .unwrap()
            .created_features[0];

        let preview = session
            .preview(&[
                ModelCommand::Move {
                    id: first,
                    position: [4.0, 5.0, 6.0],
                },
                ModelCommand::Delete { id: second },
                create_box("third"),
            ])
            .unwrap();

        assert_eq!(
            preview
                .diff()
                .modified_features
                .iter()
                .map(|feature| feature.id)
                .collect::<Vec<_>>(),
            [first]
        );
        assert_eq!(
            preview
                .diff()
                .removed_features
                .iter()
                .map(|feature| feature.id)
                .collect::<Vec<_>>(),
            [second]
        );
        assert_eq!(
            preview
                .diff()
                .added_features
                .iter()
                .map(|feature| feature.id)
                .collect::<Vec<_>>(),
            [3]
        );
    }

    #[test]
    fn stale_preview_cannot_replace_a_newer_revision() {
        let mut session = session();
        let preview = session.preview(&[create_box("stale")]).unwrap();
        session.execute(vec![create_box("live")]).unwrap();
        let document_before = session.document().clone();
        let revision_before = session.revision();

        assert!(matches!(
            session.commit_preview(preview),
            Err(SessionError::StalePreview {
                preview_revision: 1,
                active_revision: 2,
            })
        ));
        assert_eq!(session.document(), &document_before);
        assert_eq!(session.revision(), revision_before);
    }

    #[test]
    fn preview_interference_analysis_is_read_only_and_rejects_stale_previews() {
        let mut session = session();
        let preview = session.preview(&[create_box("candidate")]).unwrap();
        let document_before = session.document().clone();
        let revision_before = session.revision();

        let report = session.analyze_preview_interference(&preview).unwrap();
        assert_eq!(report.candidate_feature_ids, vec![1]);
        assert_eq!(session.document(), &document_before);
        assert_eq!(session.revision(), revision_before);
        assert!(!session.can_undo());

        session.execute(vec![create_box("committed")]).unwrap();
        assert!(matches!(
            session.analyze_preview_interference(&preview),
            Err(SessionError::StalePreview {
                preview_revision: 1,
                active_revision: 2,
            })
        ));
    }
}
