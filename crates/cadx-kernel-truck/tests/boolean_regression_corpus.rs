use cadx_core::{
    diagnostics::{BooleanFailureReason, BooleanFailureStage, BooleanHealingStatus},
    domain::{BooleanOperation, CadDocument, ModelCommand},
    kernel::{CadKernel, KernelError},
    tolerance::BooleanTolerancePolicy,
};
use cadx_kernel_truck::TruckKernel;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Corpus {
    version: u32,
    cases: Vec<CorpusCase>,
}

#[derive(Debug, Deserialize)]
struct CorpusCase {
    id: String,
    operation: BooleanOperation,
    left: BoxSpec,
    right: BoxSpec,
    policy: BooleanTolerancePolicy,
    expected: ExpectedOutcome,
}

#[derive(Debug, Deserialize)]
struct BoxSpec {
    size: [f64; 3],
    position: [f64; 3],
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum ExpectedOutcome {
    Success {
        faces: usize,
    },
    Failure {
        stage: BooleanFailureStage,
        reason: BooleanFailureReason,
        tolerances_mm: Vec<f64>,
        operand_healing: Vec<BooleanHealingStatus>,
        result_healing: Vec<BooleanHealingStatus>,
    },
}

#[test]
fn boolean_regression_corpus_is_reproducible_and_typed() {
    let corpus: Corpus =
        serde_json::from_str(include_str!("fixtures/boolean-regression-corpus.json")).unwrap();
    assert_eq!(corpus.version, 2);
    assert!(!corpus.cases.is_empty());

    for case in corpus.cases {
        let document = corpus_document(&case);
        let kernel = TruckKernel::default()
            .with_boolean_tolerance_policy(case.policy)
            .unwrap();
        let first = kernel.evaluate(&document);
        let second = kernel.evaluate(&document);
        assert_eq!(first, second, "{} changed between rebuilds", case.id);

        match (first, case.expected) {
            (Ok(scene), ExpectedOutcome::Success { faces }) => {
                assert_eq!(scene.parts.len(), 1, "{} part count", case.id);
                assert_eq!(scene.parts[0].faces.len(), faces, "{} face count", case.id);
            }
            (
                Err(KernelError::Boolean(diagnostic)),
                ExpectedOutcome::Failure {
                    stage,
                    reason,
                    tolerances_mm,
                    operand_healing,
                    result_healing,
                },
            ) => {
                assert_eq!(diagnostic.stage, stage, "{} stage", case.id);
                assert_eq!(diagnostic.reason, reason, "{} reason", case.id);
                assert_eq!(
                    diagnostic
                        .attempts
                        .iter()
                        .map(|attempt| attempt.tolerance_mm)
                        .collect::<Vec<_>>(),
                    tolerances_mm,
                    "{} tolerance sequence",
                    case.id
                );
                assert_eq!(
                    diagnostic
                        .attempts
                        .iter()
                        .map(|attempt| attempt.operand_healing)
                        .collect::<Vec<_>>(),
                    operand_healing,
                    "{} operand healing sequence",
                    case.id
                );
                assert_eq!(
                    diagnostic
                        .attempts
                        .iter()
                        .map(|attempt| attempt.result_healing)
                        .collect::<Vec<_>>(),
                    result_healing,
                    "{} result healing sequence",
                    case.id
                );
                assert!(diagnostic.left_bounds.is_some(), "{} left bounds", case.id);
                assert!(
                    diagnostic.right_bounds.is_some(),
                    "{} right bounds",
                    case.id
                );
                assert!(!diagnostic.detail.is_empty(), "{} detail", case.id);
            }
            (Ok(_), ExpectedOutcome::Failure { .. }) => {
                panic!("{} unexpectedly succeeded", case.id)
            }
            (Err(error), ExpectedOutcome::Success { .. }) => {
                panic!("{} unexpectedly failed: {error}", case.id)
            }
            (Err(error), ExpectedOutcome::Failure { .. }) => {
                panic!("{} returned an untyped failure: {error}", case.id)
            }
        }
    }
}

fn corpus_document(case: &CorpusCase) -> CadDocument {
    let mut document = CadDocument::default();
    let operands = document
        .apply_transaction([
            ModelCommand::CreateBox {
                name: format!("{} left", case.id),
                size: case.left.size,
                position: case.left.position,
            },
            ModelCommand::CreateBox {
                name: format!("{} right", case.id),
                size: case.right.size,
                position: case.right.position,
            },
        ])
        .unwrap();
    document
        .apply(ModelCommand::CreateBoolean {
            name: case.id.clone(),
            operation: case.operation,
            left: operands[0],
            right: operands[1],
        })
        .unwrap();
    document
}
