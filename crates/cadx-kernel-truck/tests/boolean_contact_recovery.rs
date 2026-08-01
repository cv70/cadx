use cadx_core::{
    diagnostics::{BooleanFailureReason, BooleanHealingStatus},
    domain::{
        BooleanOperation, CadDocument, ModelCommand, SketchLoop2D, SketchPlane, SketchRegion2D,
        SketchSegment2D,
    },
    kernel::{CadKernel, ExchangeKernel, KernelError},
    topology::{CurveKind, SurfaceKind},
};
use cadx_io::validate_step;
use cadx_kernel_truck::TruckKernel;
use truck_modeling::Point3;
use truck_stepio::{
    r#in::{
        Table,
        alias::{Curve3D, Surface as StepSurface},
    },
    out::{CompleteStepDisplay, StepHeaderDescriptor, StepModels},
};

#[test]
fn coincident_planar_solids_resolve_without_reentering_the_panicking_path() {
    let union = box_boolean(BooleanOperation::Union, [0.0; 3]);
    let scene = TruckKernel::default().evaluate(&union).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 6);

    let intersection = box_boolean(BooleanOperation::Intersect, [0.0; 3]);
    let scene = TruckKernel::default().evaluate(&intersection).unwrap();
    assert_eq!(scene.parts[0].faces.len(), 6);

    let subtraction = box_boolean(BooleanOperation::Subtract, [0.0; 3]);
    let diagnostic = boolean_error(&subtraction);
    assert_eq!(diagnostic.reason, BooleanFailureReason::EmptyResult);
    assert_eq!(
        diagnostic.attempts[0].result_healing,
        BooleanHealingStatus::Applied
    );
}

#[test]
fn bounded_planar_gap_sewing_builds_a_closed_exportable_union() {
    let document = box_boolean(BooleanOperation::Union, [10.01, 0.0, 0.0]);
    let kernel = TruckKernel::default();
    let scene = kernel.evaluate(&document).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 10);
    assert_eq!(
        mesh_bounds(&scene.parts[0].mesh.positions),
        ([0.0; 3], [20.01, 10.0, 10.0])
    );

    let step = kernel
        .encode_step(&document, "planar-gap-union.step")
        .unwrap();
    validate_step(&step).unwrap();
    assert!(step.contains("CLOSED_SHELL"));
}

#[test]
fn planar_contact_keeps_empty_results_typed() {
    let intersection = box_boolean(BooleanOperation::Intersect, [10.0, 0.0, 0.0]);
    let diagnostic = boolean_error(&intersection);
    assert_eq!(diagnostic.reason, BooleanFailureReason::EmptyResult);
    assert_eq!(diagnostic.attempts.len(), 1);
    assert_eq!(
        diagnostic.attempts[0].result_healing,
        BooleanHealingStatus::Applied
    );

    let subtraction = box_boolean(BooleanOperation::Subtract, [10.0, 0.0, 0.0]);
    let scene = TruckKernel::default().evaluate(&subtraction).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 6);
}

#[test]
fn coincident_curved_primitives_follow_boolean_set_semantics() {
    for kind in ["cylinder", "sphere", "cone", "torus"] {
        let mut source = CadDocument::default();
        source
            .apply(curved_command(kind, "source", [0.0; 3]))
            .unwrap();
        let source_scene = TruckKernel::default().evaluate(&source).unwrap();
        let expected_faces = source_scene.parts[0].faces.len();

        for operation in [BooleanOperation::Union, BooleanOperation::Intersect] {
            let document = curved_boolean(kind, operation, [0.0; 3]);
            let scene = TruckKernel::default().evaluate(&document).unwrap();
            assert_eq!(scene.parts.len(), 1, "{kind} {operation:?} part count");
            assert_eq!(
                scene.parts[0].faces.len(),
                expected_faces,
                "{kind} {operation:?} face count"
            );
        }

        let subtraction = curved_boolean(kind, BooleanOperation::Subtract, [0.0; 3]);
        let diagnostic = boolean_error(&subtraction);
        assert_eq!(
            diagnostic.reason,
            BooleanFailureReason::EmptyResult,
            "{kind}"
        );
        assert_eq!(diagnostic.attempts.len(), 1, "{kind}");
        assert_eq!(
            diagnostic.attempts[0].result_healing,
            BooleanHealingStatus::Applied,
            "{kind}"
        );
    }
}

#[test]
fn stacked_cylinders_sew_one_closed_exportable_shell() {
    let document = curved_boolean("cylinder", BooleanOperation::Union, [0.0, 0.0, 10.0]);
    let kernel = TruckKernel::default();
    let scene = kernel.evaluate(&document).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 8);
    assert_eq!(
        mesh_axis_bounds(&scene.parts[0].mesh.positions, 2),
        (0.0, 20.0)
    );

    let step = kernel
        .encode_step(&document, "stacked-cylinders.step")
        .unwrap();
    validate_step(&step).unwrap();
    assert_eq!(step.matches("CLOSED_SHELL").count(), 1);
}

#[test]
fn gapped_cylinders_align_one_closed_exportable_shell() {
    let document = curved_boolean("cylinder", BooleanOperation::Union, [0.0, 0.0, 10.01]);
    let kernel = TruckKernel::default();
    let scene = kernel.evaluate(&document).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 8);
    let (min_z, max_z) = mesh_axis_bounds(&scene.parts[0].mesh.positions, 2);
    assert!((min_z - 0.0).abs() <= 1.0e-5);
    assert!((max_z - 20.01).abs() <= 1.0e-5);

    let step = kernel
        .encode_step(&document, "aligned-cylinders.step")
        .unwrap();
    validate_step(&step).unwrap();
    assert_eq!(step.matches("CLOSED_SHELL").count(), 1);
}

#[test]
fn coincident_imported_curved_shells_resolve_and_reexport() {
    let mut source_document = CadDocument::default();
    source_document
        .apply(curved_command("cylinder", "source", [0.0; 3]))
        .unwrap();
    let source = TruckKernel::default()
        .encode_step(&source_document, "curved-source.step")
        .unwrap();
    let shell_id = *Table::from_step(&source)
        .unwrap()
        .shell
        .keys()
        .next()
        .unwrap();

    let union = imported_boolean(&source, shell_id, BooleanOperation::Union);
    let kernel = TruckKernel::default();
    let scene = kernel.evaluate(&union).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 5);
    let step = kernel
        .encode_step(&union, "imported-curved-union.step")
        .unwrap();
    validate_step(&step).unwrap();

    let subtraction = imported_boolean(&source, shell_id, BooleanOperation::Subtract);
    let diagnostic = boolean_error(&subtraction);
    assert_eq!(diagnostic.reason, BooleanFailureReason::EmptyResult);
    assert_eq!(diagnostic.attempts.len(), 1);
    assert_eq!(
        diagnostic.attempts[0].result_healing,
        BooleanHealingStatus::Applied
    );
}

#[test]
fn imported_freeform_gap_refits_boundaries_without_moving_the_far_cap() {
    assert_imported_freeform_gap(false);
}

#[test]
fn imported_multi_row_freeform_gap_refits_and_reexports() {
    assert_imported_freeform_gap(true);
}

fn assert_imported_freeform_gap(elevate_cross_degree: bool) {
    let mut source_document = CadDocument::default();
    let sketch = source_document
        .apply(ModelCommand::CreateSketchRegion {
            name: "freeform profile".into(),
            plane: SketchPlane::WorldXy,
            region: freeform_region(),
            construction: Vec::new(),
            constraints: Vec::new(),
            position: [0.0; 3],
        })
        .unwrap()
        .unwrap();
    source_document
        .apply(ModelCommand::CreateExtrusionFromSketch {
            name: "freeform source".into(),
            sketch_id: sketch,
            height: 10.0,
            position: [0.0; 3],
        })
        .unwrap();

    let kernel = TruckKernel::default();
    let mut source = kernel
        .encode_step(&source_document, "freeform-source.step")
        .unwrap();
    assert!(source.contains("B_SPLINE_SURFACE"));
    assert!(source.contains("RATIONAL_B_SPLINE_SURFACE"));
    if elevate_cross_degree {
        source = degree_elevated_freeform_step(&source);
    }
    let shell_id = *Table::from_step(&source)
        .unwrap()
        .shell
        .keys()
        .next()
        .unwrap();

    let mut document = CadDocument::default();
    let operands = document
        .apply_transaction([
            ModelCommand::ImportStep {
                name: "lower freeform".into(),
                source: source.clone(),
                shell_id,
                position: [0.0; 3],
            },
            ModelCommand::ImportStep {
                name: "upper freeform".into(),
                source,
                shell_id,
                position: [0.0, 0.0, 10.01],
            },
        ])
        .unwrap();
    document
        .apply(ModelCommand::CreateBoolean {
            name: "aligned freeform union".into(),
            operation: BooleanOperation::Union,
            left: operands[0],
            right: operands[1],
        })
        .unwrap();

    let scene = kernel.evaluate(&document).unwrap();
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(scene.parts[0].faces.len(), 10);
    assert!(
        scene.parts[0]
            .faces
            .iter()
            .any(|face| face.geometry.surface == SurfaceKind::Swept)
    );
    assert!(
        scene.parts[0]
            .edges
            .iter()
            .any(|edge| edge.geometry.curve == CurveKind::BSpline)
    );
    assert!(
        scene.parts[0]
            .edges
            .iter()
            .any(|edge| edge.geometry.curve == CurveKind::Nurbs)
    );
    let (min_z, max_z) = mesh_axis_bounds(&scene.parts[0].mesh.positions, 2);
    assert!((min_z - 0.0).abs() <= 1.0e-5);
    assert!((max_z - 20.01).abs() <= 1.0e-5);

    let step = kernel
        .encode_step(&document, "aligned-freeform-union.step")
        .unwrap();
    validate_step(&step).unwrap();
    assert_eq!(step.matches("CLOSED_SHELL").count(), 1);
    assert!(step.contains("B_SPLINE_SURFACE"));
    assert!(step.contains("RATIONAL_B_SPLINE_SURFACE"));
}

fn degree_elevated_freeform_step(source: &str) -> String {
    let table = Table::from_step(source).unwrap();
    let mut shell = table
        .to_compressed_shell(table.shell.values().next().unwrap())
        .unwrap();
    let mut elevated = 0;
    for face in &mut shell.faces {
        match &mut face.surface {
            StepSurface::BSplineSurface(surface) => {
                assert_eq!(surface.control_points()[0].len(), 2);
                surface.elevate_vdegree().elevate_vdegree();
                elevated += 1;
            }
            StepSurface::NurbsSurface(surface) => {
                assert_eq!(surface.control_points()[0].len(), 2);
                surface.elevate_vdegree().elevate_vdegree();
                elevated += 1;
            }
            StepSurface::ElementarySurface(_) | StepSurface::SweptCurve(_) => {}
        }
    }
    assert_eq!(elevated, 2);

    let models: StepModels<'_, Point3, Curve3D, StepSurface> = std::iter::once(&shell).collect();
    let source = CompleteStepDisplay::new(
        models,
        StepHeaderDescriptor {
            file_name: "degree-elevated-freeform.step".into(),
            organization_system: "CADX test".into(),
            ..StepHeaderDescriptor::default()
        },
    )
    .to_string();
    validate_step(&source).unwrap();

    let table = Table::from_step(&source).unwrap();
    let shell = table
        .to_compressed_shell(table.shell.values().next().unwrap())
        .unwrap();
    let elevated = shell
        .faces
        .iter()
        .filter(|face| match &face.surface {
            StepSurface::BSplineSurface(surface) => surface.udegree() > 1 && surface.vdegree() > 1,
            StepSurface::NurbsSurface(surface) => surface.udegree() > 1 && surface.vdegree() > 1,
            StepSurface::ElementarySurface(_) | StepSurface::SweptCurve(_) => false,
        })
        .count();
    assert_eq!(elevated, 2);
    source
}

fn box_boolean(operation: BooleanOperation, right_position: [f64; 3]) -> CadDocument {
    let mut document = CadDocument::default();
    let operands = document
        .apply_transaction([
            ModelCommand::CreateBox {
                name: "left".into(),
                size: [10.0; 3],
                position: [0.0; 3],
            },
            ModelCommand::CreateBox {
                name: "right".into(),
                size: [10.0; 3],
                position: right_position,
            },
        ])
        .unwrap();
    document
        .apply(ModelCommand::CreateBoolean {
            name: "contact".into(),
            operation,
            left: operands[0],
            right: operands[1],
        })
        .unwrap();
    document
}

fn curved_boolean(
    kind: &str,
    operation: BooleanOperation,
    right_position: [f64; 3],
) -> CadDocument {
    let mut document = CadDocument::default();
    let operands = document
        .apply_transaction([
            curved_command(kind, "left", [0.0; 3]),
            curved_command(kind, "right", right_position),
        ])
        .unwrap();
    document
        .apply(ModelCommand::CreateBoolean {
            name: format!("{kind} contact"),
            operation,
            left: operands[0],
            right: operands[1],
        })
        .unwrap();
    document
}

fn curved_command(kind: &str, name: &str, position: [f64; 3]) -> ModelCommand {
    match kind {
        "cylinder" => ModelCommand::CreateCylinder {
            name: name.into(),
            radius: 5.0,
            height: 10.0,
            position,
        },
        "sphere" => ModelCommand::CreateSphere {
            name: name.into(),
            radius: 5.0,
            position,
        },
        "cone" => ModelCommand::CreateCone {
            name: name.into(),
            bottom_radius: 5.0,
            top_radius: 2.0,
            height: 10.0,
            position,
        },
        "torus" => ModelCommand::CreateTorus {
            name: name.into(),
            major_radius: 8.0,
            minor_radius: 2.0,
            position,
        },
        _ => panic!("unsupported curved primitive {kind}"),
    }
}

fn imported_boolean(source: &str, shell_id: u64, operation: BooleanOperation) -> CadDocument {
    let mut document = CadDocument::default();
    let operands = document
        .apply_transaction([
            ModelCommand::ImportStep {
                name: "left imported".into(),
                source: source.into(),
                shell_id,
                position: [0.0; 3],
            },
            ModelCommand::ImportStep {
                name: "right imported".into(),
                source: source.into(),
                shell_id,
                position: [0.0; 3],
            },
        ])
        .unwrap();
    document
        .apply(ModelCommand::CreateBoolean {
            name: "imported contact".into(),
            operation,
            left: operands[0],
            right: operands[1],
        })
        .unwrap();
    document
}

fn freeform_region() -> SketchRegion2D {
    SketchRegion2D {
        profile: SketchLoop2D {
            segments: vec![
                SketchSegment2D::CubicBezier {
                    start: [0.0, 0.0],
                    control1: [3.0, -2.0],
                    control2: [7.0, -2.0],
                    end: [10.0, 0.0],
                },
                SketchSegment2D::Line {
                    start: [10.0, 0.0],
                    end: [10.0, 10.0],
                },
                SketchSegment2D::RationalQuadratic {
                    start: [10.0, 10.0],
                    control: [5.0, 14.0],
                    end: [0.0, 10.0],
                    weight: 0.8,
                },
                SketchSegment2D::Line {
                    start: [0.0, 10.0],
                    end: [0.0, 0.0],
                },
            ],
        },
        holes: Vec::new(),
    }
}

fn boolean_error(document: &CadDocument) -> cadx_core::diagnostics::BooleanDiagnostic {
    match TruckKernel::default().evaluate(document) {
        Err(KernelError::Boolean(diagnostic)) => *diagnostic,
        Ok(_) => panic!("expected an empty boolean result"),
        Err(error) => panic!("expected a typed boolean diagnostic, got {error}"),
    }
}

fn mesh_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    (min, max)
}

fn mesh_axis_bounds(positions: &[[f32; 3]], axis: usize) -> (f32, f32) {
    positions
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), point| {
            (min.min(point[axis]), max.max(point[axis]))
        })
}
