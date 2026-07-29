use std::collections::BTreeSet;

use cadx_core::{CadCommand, CadDocument, CommandTransaction, Entity, EntityKind, Layer, Point2};

use super::*;

fn entity(id: u64, kind: EntityKind) -> Entity {
    Entity {
        id,
        layer: 1,
        name: format!("Entity {id}"),
        visible: true,
        kind,
        parameter_refs: BTreeSet::new(),
    }
}

fn mechanical_document(points: Vec<Point2>, distance: f64) -> CadDocument {
    let mut document = CadDocument::new("Mechanical scene");
    CommandTransaction::new(vec![
        CadCommand::CreateEntity {
            entity: entity(
                1,
                EntityKind::SketchProfile {
                    points,
                    closed: true,
                },
            ),
        },
        CadCommand::CreateEntity {
            entity: entity(
                2,
                EntityKind::Extrude {
                    profile: 1,
                    distance,
                },
            ),
        },
    ])
    .apply(&mut document)
    .unwrap();
    document
}

#[test]
fn scene_extracts_only_visible_layers_and_entities() {
    let mut document = CadDocument::new("Scene");
    CommandTransaction::new(vec![
        CadCommand::CreateLayer {
            layer: Layer {
                id: 2,
                name: "Hidden".into(),
                visible: false,
                locked: false,
                color: [255, 0, 0, 255],
            },
        },
        CadCommand::CreateEntity {
            entity: entity(
                1,
                EntityKind::Line {
                    start: Point2::new(-10.0, 0.0),
                    end: Point2::new(10.0, 0.0),
                },
            ),
        },
        CadCommand::CreateEntity {
            entity: Entity {
                layer: 2,
                ..entity(
                    2,
                    EntityKind::Circle {
                        center: Point2::new(50.0, 50.0),
                        radius: 10.0,
                    },
                )
            },
        },
    ])
    .apply(&mut document)
    .unwrap();

    let scene = RenderScene::from_document(&document);

    assert_eq!(scene.items.len(), 1);
    assert_eq!(scene.items[0].entity_id, 1);
    assert_eq!(scene.bounds.unwrap().min, Point2::new(-10.0, 0.0));
    assert_eq!(scene.bounds.unwrap().max, Point2::new(10.0, 0.0));
}

#[test]
fn camera_round_trip_and_zoom_preserve_anchor() {
    let viewport = ViewportSize::new(1200.0, 800.0);
    let mut transform = ViewTransform::new(Point2::new(15.0, -8.0), 4.0);
    let world = Point2::new(33.0, 21.0);
    let screen = transform.project(world, viewport);

    assert_close_point(transform.unproject(screen, viewport), world);
    transform.zoom_at(screen, viewport, 2.0);
    assert_close_point(transform.unproject(screen, viewport), world);
}

#[test]
fn picker_prefers_the_nearest_visible_geometry() {
    let scene = RenderScene {
        items: vec![
            RenderItem {
                entity_id: 1,
                layer_id: 1,
                color: [255, 255, 255, 255],
                locked: false,
                primitive: RenderPrimitive::Line {
                    start: Point2::new(-10.0, 0.0),
                    end: Point2::new(10.0, 0.0),
                },
            },
            RenderItem {
                entity_id: 2,
                layer_id: 1,
                color: [255, 255, 255, 255],
                locked: false,
                primitive: RenderPrimitive::Circle {
                    center: Point2::new(0.0, 8.0),
                    radius: 2.0,
                },
            },
        ],
        bounds: None,
    };

    assert_eq!(scene.pick(Point2::new(1.0, 0.3), 1.0).unwrap().entity_id, 1);
    assert_eq!(scene.pick(Point2::new(0.0, 6.2), 1.0).unwrap().entity_id, 2);
    assert!(scene.pick(Point2::new(100.0, 100.0), 1.0).is_none());
}

#[test]
fn snapper_returns_geometry_points_with_stable_tie_breaking() {
    let scene = RenderScene {
        items: vec![RenderItem {
            entity_id: 4,
            layer_id: 1,
            color: [255, 255, 255, 255],
            locked: false,
            primitive: RenderPrimitive::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, 0.0),
            },
        }],
        bounds: None,
    };
    let settings = SnapSettings::new(true, true, 5.0);

    let vertex = scene.snap(Point2::new(0.0, 0.0), 0.5, settings).unwrap();
    assert_eq!(vertex.point, Point2::new(0.0, 0.0));
    assert_eq!(vertex.kind, SnapKind::Vertex);
    assert_eq!(vertex.entity_id, Some(4));

    let midpoint = scene.snap(Point2::new(5.1, 0.1), 0.5, settings).unwrap();
    assert_eq!(midpoint.point, Point2::new(5.0, 0.0));
    assert_eq!(midpoint.kind, SnapKind::Midpoint);
    assert_eq!(midpoint.entity_id, Some(4));
}

#[test]
fn snapper_honors_grid_settings_and_tolerance() {
    let scene = RenderScene {
        items: Vec::new(),
        bounds: None,
    };
    let settings = SnapSettings::new(false, true, 5.0);

    let hit = scene.snap(Point2::new(12.3, -7.6), 4.0, settings).unwrap();
    assert_eq!(hit.point, Point2::new(10.0, -10.0));
    assert_eq!(hit.kind, SnapKind::Grid);
    assert_eq!(hit.entity_id, None);
    assert!(scene.snap(Point2::new(12.3, -7.6), 0.5, settings).is_none());
    assert!(
        scene
            .snap(
                Point2::new(12.3, -7.6),
                4.0,
                SnapSettings::new(false, false, 5.0)
            )
            .is_none()
    );
    assert!(
        scene
            .snap(
                Point2::new(12.3, -7.6),
                4.0,
                SnapSettings::new(false, true, 0.0)
            )
            .is_none()
    );
}

#[test]
fn locked_geometry_is_visible_and_snappable_but_not_pickable() {
    let scene = RenderScene {
        items: vec![RenderItem {
            entity_id: 7,
            layer_id: 2,
            color: [180, 180, 180, 255],
            locked: true,
            primitive: RenderPrimitive::Line {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, 0.0),
            },
        }],
        bounds: None,
    };

    assert!(scene.pick(Point2::new(5.0, 0.0), 1.0).is_none());
    let snap = scene
        .snap(
            Point2::new(0.1, 0.1),
            1.0,
            SnapSettings::new(true, false, 5.0),
        )
        .unwrap();
    assert_eq!(snap.entity_id, Some(7));
    assert_eq!(snap.kind, SnapKind::Vertex);
}

#[test]
fn arc_bounds_picking_and_snaps_follow_only_the_angular_span() {
    let document = {
        let mut document = CadDocument::new("Arc scene");
        CommandTransaction::new(vec![CadCommand::CreateEntity {
            entity: entity(
                1,
                EntityKind::Arc {
                    center: Point2::new(0.0, 0.0),
                    radius: 10.0,
                    start_angle: 0.0,
                    sweep_angle: std::f64::consts::FRAC_PI_2,
                },
            ),
        }])
        .apply(&mut document)
        .unwrap();
        document
    };
    let scene = RenderScene::from_document(&document);
    let bounds = scene.bounds.unwrap();

    assert_close_point(bounds.min, Point2::new(0.0, 0.0));
    assert_close_point(bounds.max, Point2::new(10.0, 10.0));
    let diagonal = 10.0 / 2.0_f64.sqrt();
    assert_eq!(
        scene
            .pick(Point2::new(diagonal, diagonal), 1.0e-8)
            .unwrap()
            .entity_id,
        1
    );
    assert!(scene.pick(Point2::new(-10.0, 0.0), 0.1).is_none());

    let settings = SnapSettings::new(true, false, 5.0);
    let start = scene.snap(Point2::new(10.0, 0.0), 0.1, settings).unwrap();
    assert_eq!(start.kind, SnapKind::Vertex);
    let midpoint = scene
        .snap(Point2::new(diagonal, diagonal), 0.1, settings)
        .unwrap();
    assert_eq!(midpoint.kind, SnapKind::Midpoint);
    let center = scene.snap(Point2::new(0.0, 0.0), 0.1, settings).unwrap();
    assert_eq!(center.kind, SnapKind::Center);
}

#[test]
fn aligned_dimension_layout_bounds_picking_snaps_and_text_are_consistent() {
    let mut document = CadDocument::new("Dimension scene");
    CommandTransaction::new(vec![CadCommand::CreateEntity {
        entity: entity(
            1,
            EntityKind::AlignedDimension {
                start: Point2::new(0.0, 0.0),
                end: Point2::new(10.0, 0.0),
                offset: 4.0,
                text_override: Some("REF <>".into()),
            },
        ),
    }])
    .apply(&mut document)
    .unwrap();
    let scene = RenderScene::from_document(&document);
    let bounds = scene.bounds.unwrap();

    assert_close_point(bounds.min, Point2::new(0.0, 0.0));
    assert_close_point(bounds.max, Point2::new(10.0, 4.0));
    assert_eq!(
        scene.items[0].primitive,
        RenderPrimitive::AlignedDimension {
            start: Point2::new(0.0, 0.0),
            end: Point2::new(10.0, 0.0),
            offset: 4.0,
            label: "REF 10.00".into(),
        }
    );
    assert_eq!(scene.pick(Point2::new(5.0, 4.0), 0.1).unwrap().entity_id, 1);
    assert!(scene.pick(Point2::new(5.0, 0.0), 0.1).is_none());

    let settings = SnapSettings::new(true, false, 5.0);
    let source = scene.snap(Point2::new(0.0, 0.0), 0.1, settings).unwrap();
    assert_eq!(source.kind, SnapKind::Vertex);
    let label = scene.snap(Point2::new(5.0, 4.0), 0.1, settings).unwrap();
    assert_eq!(label.kind, SnapKind::Insertion);

    let geometry =
        aligned_dimension_geometry(Point2::new(1.0, 2.0), Point2::new(4.0, 6.0), -2.0).unwrap();
    assert!((geometry.measurement - 5.0).abs() < 1.0e-9);
    assert!(
        (aligned_dimension_offset(geometry.start, geometry.end, geometry.dimension_start).unwrap()
            + 2.0)
            .abs()
            < 1.0e-9
    );
}

#[test]
fn square_extrusion_has_closed_mesh_topology_and_exact_bounds() {
    let mesh = SolidMesh::extrude(
        &[
            Point2::new(-2.0, -1.0),
            Point2::new(4.0, -1.0),
            Point2::new(4.0, 3.0),
            Point2::new(-2.0, 3.0),
        ],
        8.0,
    )
    .unwrap();

    assert_eq!(mesh.vertices.len(), 8);
    assert_eq!(mesh.triangles.len(), 12);
    assert_eq!(mesh.feature_edges.len(), 12);
    assert_eq!(mesh.bounds.min, Point3::new(-2.0, -1.0, 0.0));
    assert_eq!(mesh.bounds.max, Point3::new(4.0, 3.0, 8.0));
    assert!(
        mesh.triangles
            .iter()
            .flatten()
            .all(|index| (*index as usize) < mesh.vertices.len())
    );
    for pair in mesh.triangles[..4].chunks_exact(2) {
        let normal_z = |triangle: [u32; 3]| {
            let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
            (b.x - a.x).mul_add(c.y - a.y, -(b.y - a.y) * (c.x - a.x))
        };
        assert!(normal_z(pair[0]) > 0.0, "top cap must face positive Z");
        assert!(normal_z(pair[1]) < 0.0, "bottom cap must face negative Z");
    }
}

#[test]
fn clockwise_and_concave_profiles_produce_consistent_meshes() {
    let counter_clockwise = vec![
        Point2::new(0.0, 0.0),
        Point2::new(6.0, 0.0),
        Point2::new(6.0, 5.0),
        Point2::new(3.0, 2.0),
        Point2::new(0.0, 5.0),
    ];
    let clockwise = counter_clockwise.iter().copied().rev().collect::<Vec<_>>();

    let forward = SolidMesh::extrude(&counter_clockwise, 3.0).unwrap();
    let reversed = SolidMesh::extrude(&clockwise, 3.0).unwrap();

    assert_eq!(forward, reversed);
    assert_eq!(forward.vertices.len(), 10);
    assert_eq!(forward.triangles.len(), 16);
}

#[test]
fn extrusion_rejects_degenerate_profiles_and_distances() {
    let collinear = [
        Point2::new(0.0, 0.0),
        Point2::new(2.0, 0.0),
        Point2::new(4.0, 0.0),
    ];
    assert!(matches!(
        SolidMesh::extrude(&collinear, 5.0),
        Err(MechanicalSceneError::InvalidProfile(_))
    ));
    assert!(matches!(
        SolidMesh::extrude(
            &[
                Point2::new(0.0, 0.0),
                Point2::new(2.0, 0.0),
                Point2::new(0.0, 2.0),
            ],
            0.0,
        ),
        Err(MechanicalSceneError::InvalidProfile(_))
    ));
}

#[test]
fn mechanical_scene_resolves_profiles_and_honors_visibility_and_locking() {
    let mut document = mechanical_document(
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 6.0),
            Point2::new(0.0, 6.0),
        ],
        4.0,
    );
    document.layers.get_mut(&1).unwrap().locked = true;
    let scene = MechanicalScene::from_document(&document).unwrap();

    assert_eq!(scene.items.len(), 1);
    assert_eq!(scene.items[0].entity_id, 2);
    assert!(scene.items[0].locked);
    assert_eq!(scene.items[0].color, document.layers[&1].color);
    assert!(scene.bounds.is_some());

    document.entities.get_mut(&2).unwrap().visible = false;
    assert!(
        MechanicalScene::from_document(&document)
            .unwrap()
            .items
            .is_empty()
    );

    document.entities.get_mut(&2).unwrap().visible = true;
    document.layers.get_mut(&1).unwrap().visible = false;
    assert!(
        MechanicalScene::from_document(&document)
            .unwrap()
            .items
            .is_empty()
    );

    document.layers.get_mut(&1).unwrap().visible = true;
    let EntityKind::Extrude { profile, .. } = &mut document.entities.get_mut(&2).unwrap().kind
    else {
        panic!("expected an extrusion");
    };
    *profile = 99;
    assert!(matches!(
        MechanicalScene::from_document(&document),
        Err(MechanicalSceneError::ProfileMissing {
            extrude: 2,
            profile: 99
        })
    ));
}

#[test]
fn orbit_projection_fit_and_picking_are_finite_and_interactive() {
    let document = mechanical_document(
        vec![
            Point2::new(-8.0, -5.0),
            Point2::new(8.0, -5.0),
            Point2::new(8.0, 5.0),
            Point2::new(-8.0, 5.0),
        ],
        7.0,
    );
    let scene = MechanicalScene::from_document(&document).unwrap();
    let viewport = ViewportSize::new(960.0, 640.0);
    let mut camera = OrbitCamera::default();
    camera.fit_bounds(scene.bounds.unwrap(), viewport, 0.12);
    let triangles = scene.projected_triangles(camera, viewport);

    assert!(!triangles.is_empty());
    assert!(triangles.len() < scene.items[0].mesh.triangles.len());
    assert!(
        triangles
            .iter()
            .any(|triangle| triangle.edges.contains(&false))
    );
    assert!(triangles.iter().all(|triangle| {
        triangle
            .points
            .iter()
            .all(|point| point.x.is_finite() && point.y.is_finite())
    }));
    for vertex in &scene.items[0].mesh.vertices {
        let point = camera.project_point(*vertex, viewport).unwrap();
        assert!((0.0..=viewport.width).contains(&point.x));
        assert!((0.0..=viewport.height).contains(&point.y));
    }

    let front = triangles.last().unwrap();
    let pick_point = ScreenPoint::new(
        (front.points[0].x + front.points[1].x + front.points[2].x) / 3.0,
        (front.points[0].y + front.points[1].y + front.points[2].y) / 3.0,
    );
    assert_eq!(
        scene.pick(camera, viewport, pick_point).unwrap().entity_id,
        2
    );

    let probe = scene.items[0].mesh.vertices[0];
    let before_orbit = camera.project_point(probe, viewport).unwrap();
    camera.orbit_pixels(45.0, -18.0);
    let after_orbit = camera.project_point(probe, viewport).unwrap();
    assert!((before_orbit.x - after_orbit.x).hypot(before_orbit.y - after_orbit.y) > 0.1);

    let first = scene.items[0].mesh.vertices[0];
    let second = scene.items[0].mesh.vertices[1];
    let projected_length = |camera: OrbitCamera| {
        let first = camera.project_point(first, viewport).unwrap();
        let second = camera.project_point(second, viewport).unwrap();
        (first.x - second.x).hypot(first.y - second.y)
    };
    let before_zoom = projected_length(camera);
    camera.zoom(1.5);
    assert!(projected_length(camera) > before_zoom);
}

#[test]
fn gpu_camera_projection_matches_cpu_screen_projection() {
    let bounds = Bounds3 {
        min: Point3::new(-12.0, -8.0, 0.0),
        max: Point3::new(12.0, 8.0, 9.0),
    };
    let viewport = ViewportSize::new(1280.0, 720.0);
    let mut camera = OrbitCamera::default();
    camera.fit_bounds(bounds, viewport, 0.15);
    let projection = camera.projection(bounds, viewport).unwrap();

    assert!(projection.near > 0.0);
    assert!(projection.far > projection.near);
    for point in [
        bounds.min,
        bounds.max,
        Point3::new(bounds.min.x, bounds.max.y, bounds.min.z),
        Point3::new(bounds.max.x, bounds.min.y, bounds.max.z),
    ] {
        let cpu = camera.project_point(point, viewport).unwrap();
        let [x, y, z, w] = multiply_matrix_point(projection.view_projection, point);
        assert!(w > 0.0);
        let gpu = ScreenPoint::new(
            (f64::from(x / w) + 1.0) * viewport.width * 0.5,
            (1.0 - f64::from(y / w)) * viewport.height * 0.5,
        );
        assert!((cpu.x - gpu.x).abs() < 1.0e-3);
        assert!((cpu.y - gpu.y).abs() < 1.0e-3);
        assert!((0.0..=1.0).contains(&(z / w)));
    }
}

#[test]
fn locked_mechanical_items_render_but_cannot_be_picked() {
    let mut document = mechanical_document(
        vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ],
        5.0,
    );
    document.layers.get_mut(&1).unwrap().locked = true;
    let scene = MechanicalScene::from_document(&document).unwrap();
    let viewport = ViewportSize::new(800.0, 600.0);
    let mut camera = OrbitCamera::default();
    camera.fit_bounds(scene.bounds.unwrap(), viewport, 0.1);
    let triangles = scene.projected_triangles(camera, viewport);
    assert!(!triangles.is_empty());
    assert!(triangles.iter().all(|triangle| triangle.locked));
    let triangle = triangles.last().unwrap();
    let point = ScreenPoint::new(
        (triangle.points[0].x + triangle.points[1].x + triangle.points[2].x) / 3.0,
        (triangle.points[0].y + triangle.points[1].y + triangle.points[2].y) / 3.0,
    );
    assert!(scene.pick(camera, viewport, point).is_none());
}

fn multiply_matrix_point(matrix: [[f32; 4]; 4], point: Point3) -> [f32; 4] {
    let point = [point.x as f32, point.y as f32, point.z as f32, 1.0];
    std::array::from_fn(|row| {
        matrix
            .iter()
            .zip(point)
            .map(|(column, value)| column[row] * value)
            .sum()
    })
}

fn assert_close_point(actual: Point2, expected: Point2) {
    assert!((actual.x - expected.x).abs() < 1e-9);
    assert!((actual.y - expected.y).abs() < 1e-9);
}
