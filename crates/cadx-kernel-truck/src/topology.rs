use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadx_core::{
    domain::{Feature, Primitive, SketchLoop2D, SketchRegion2D},
    kernel::{KernelError, TriangleMesh},
    topology::{
        CurveKind, EdgeGeometry, EdgeRef, EvaluatedEdge, EvaluatedFace, EvaluatedVertex,
        FaceGeometry, FaceRef, PlaneGeometry, PrimitiveFace, SurfaceKind, VertexGeometry,
        VertexRef,
    },
};
use truck_meshalgo::prelude::*;
use truck_modeling::{
    Curve, Edge, EdgeID, Matrix4, Point3, Solid, Surface, Transformed, Vector3, Vertex, VertexID,
};

#[derive(Debug, Clone)]
pub(crate) struct NamedFace {
    reference: FaceRef,
    surface: SurfaceKind,
}

#[derive(Debug, Clone)]
pub(crate) struct NamedSolid {
    pub(crate) solid: Solid,
    faces: Vec<NamedFace>,
}

#[derive(Debug, Clone)]
pub(crate) enum BooleanSourceGeometry {
    Identity,
    Translation(Vector3),
    SurfaceReplacements(Vec<(Surface, Surface)>),
}

impl NamedSolid {
    pub(crate) fn new(
        feature: &Feature,
        solid: Solid,
        faces: Vec<NamedFace>,
    ) -> Result<Self, KernelError> {
        let actual = solid.face_iter().count();
        if actual != faces.len() {
            return Err(topology_error(
                feature,
                format!(
                    "kernel transformation changed the face count from {} to {actual}",
                    faces.len()
                ),
            ));
        }
        Ok(Self { solid, faces })
    }
}

#[derive(Debug)]
pub(crate) struct ResolvedEdge {
    pub(crate) edge: Edge,
    pub(crate) geometry: EdgeGeometry,
    pub(crate) adjacent_faces: [FaceGeometry; 2],
    pub(crate) adjacent_edge_directions: [[f64; 3]; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeResolutionFailure {
    Lost,
    Ambiguous,
    InvalidTopology,
}

#[derive(Debug)]
pub(crate) struct EdgeResolutionError {
    pub(crate) failure: EdgeResolutionFailure,
    pub(crate) detail: String,
}

impl EdgeResolutionError {
    fn invalid(error: &KernelError) -> Self {
        Self {
            failure: EdgeResolutionFailure::InvalidTopology,
            detail: error.to_string(),
        }
    }
}

pub(crate) fn resolve_edge(
    feature: &Feature,
    named: &NamedSolid,
    reference: &EdgeRef,
    tolerance: f64,
) -> Result<ResolvedEdge, EdgeResolutionError> {
    let (candidates, fragments) = edge_candidates(feature, named, tolerance)
        .map_err(|error| EdgeResolutionError::invalid(&error))?;
    let matches = candidates
        .iter()
        .zip(fragments)
        .filter(|(candidate, fragment)| {
            EdgeRef::new(
                feature.id,
                candidate.adjacent_faces[0].clone(),
                candidate.adjacent_faces[1].clone(),
                *fragment,
            ) == *reference
        })
        .map(|(candidate, _)| candidate)
        .collect::<Vec<_>>();
    let [candidate] = matches.as_slice() else {
        let (failure, detail) = if matches.is_empty() {
            (
                EdgeResolutionFailure::Lost,
                format!("persistent edge reference {reference} was lost"),
            )
        } else {
            (
                EdgeResolutionFailure::Ambiguous,
                format!("persistent edge reference {reference} is ambiguous"),
            )
        };
        return Err(EdgeResolutionError { failure, detail });
    };

    let face_meshes = tessellate_faces(feature, &named.solid, tolerance)
        .map_err(|error| EdgeResolutionError::invalid(&error))?;
    let mut adjacent_faces = Vec::with_capacity(2);
    let mut adjacent_edge_directions = Vec::with_capacity(2);
    for ((face, named_face), mesh) in named.solid.face_iter().zip(&named.faces).zip(face_meshes) {
        if reference.adjacent_faces.contains(&named_face.reference) {
            let mut geometry = face_geometry(&mesh);
            geometry.surface = named_face.surface;
            geometry.plane = plane_geometry(&face.surface());
            adjacent_faces.push(geometry);
            let oriented = face
                .edge_iter()
                .find(|edge| edge.id() == candidate.edge.id())
                .ok_or_else(|| EdgeResolutionError {
                    failure: EdgeResolutionFailure::InvalidTopology,
                    detail: format!("persistent edge reference {reference} lost a face incidence"),
                })?;
            let front = point_array(oriented.front().point());
            let back = point_array(oriented.back().point());
            let direction = normalize(sub(back, front)).ok_or_else(|| EdgeResolutionError {
                failure: EdgeResolutionFailure::InvalidTopology,
                detail: format!("persistent edge reference {reference} has a degenerate incidence"),
            })?;
            adjacent_edge_directions.push(direction);
        }
    }
    let adjacent_faces: [FaceGeometry; 2] =
        adjacent_faces
            .try_into()
            .map_err(|faces: Vec<_>| EdgeResolutionError {
                failure: EdgeResolutionFailure::InvalidTopology,
                detail: format!(
                    "persistent edge reference {reference} resolved to {} adjacent faces instead of two",
                    faces.len()
                ),
            })?;
    let adjacent_edge_directions = adjacent_edge_directions.try_into().map_err(
        |directions: Vec<_>| {
            EdgeResolutionError {
                failure: EdgeResolutionFailure::InvalidTopology,
                detail: format!(
                    "persistent edge reference {reference} resolved to {} oriented incidences instead of two",
                    directions.len()
                ),
            }
        },
    )?;
    Ok(ResolvedEdge {
        edge: candidate.edge.clone(),
        geometry: candidate.geometry.clone(),
        adjacent_faces,
        adjacent_edge_directions,
    })
}

pub(crate) fn name_primitive_faces(
    feature: &Feature,
    primitive: &Primitive,
    solid: &Solid,
    tolerance: f64,
) -> Result<Vec<NamedFace>, KernelError> {
    let meshes = tessellate_faces(feature, solid, tolerance)?;
    let metrics = meshes.iter().map(face_geometry).collect::<Vec<_>>();
    let roles = unique_primitive_roles(
        feature,
        primitive_roles(feature, primitive, &metrics, tolerance)?,
        &metrics,
        None,
    )?;
    named_faces_from_roles(feature, primitive, solid, roles)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn name_extrusion_faces(
    feature: &Feature,
    region: &SketchRegion2D,
    height: f64,
    solid: &Solid,
    origin: [f64; 3],
    x_dir: [f64; 3],
    y_dir: [f64; 3],
    normal: [f64; 3],
    tolerance: f64,
) -> Result<Vec<NamedFace>, KernelError> {
    let meshes = tessellate_faces(feature, solid, tolerance)?;
    let metrics = meshes.iter().map(face_geometry).collect::<Vec<_>>();
    let roles = extrusion_roles_in_frame(
        feature, &metrics, region, height, origin, x_dir, y_dir, normal, tolerance,
    )?;
    validate_extrusion_role_coverage(feature, region, &roles)?;
    let roles = unique_primitive_roles(feature, roles, &metrics, Some((origin, x_dir, y_dir)))?;
    named_faces_from_roles(
        feature,
        &Primitive::Extrusion {
            profile: region.profile.vertices(),
            height,
        },
        solid,
        roles,
    )
}

pub(crate) fn name_loft_faces(
    feature: &Feature,
    primitive: &Primitive,
    solid: &Solid,
) -> Result<Vec<NamedFace>, KernelError> {
    let Primitive::LoftFromSketches { profiles, .. } = primitive else {
        return Err(topology_error(
            feature,
            "loft face naming requires a loft primitive",
        ));
    };
    let segment_count = profiles.first().map_or(0, |profile| profile.segments.len());
    let transition_count = profiles.len().saturating_sub(1);
    let expected = transition_count
        .checked_mul(segment_count)
        .and_then(|sides| sides.checked_add(2))
        .ok_or_else(|| topology_error(feature, "loft face count overflowed usize"))?;
    if solid.face_iter().count() != expected {
        return Err(topology_error(
            feature,
            format!(
                "loft produced {} faces instead of the expected {expected}",
                solid.face_iter().count()
            ),
        ));
    }
    let mut roles = Vec::with_capacity(expected);
    roles.push(PrimitiveFace::StartCap);
    for transition in 0..transition_count {
        let transition = u32::try_from(transition)
            .map_err(|_| topology_error(feature, "loft has more than u32::MAX transitions"))?;
        for segment in 0..segment_count {
            let segment = u32::try_from(segment)
                .map_err(|_| topology_error(feature, "loft has more than u32::MAX segments"))?;
            roles.push(PrimitiveFace::LoftSide {
                transition,
                segment,
            });
        }
    }
    roles.push(PrimitiveFace::EndCap);
    solid
        .face_iter()
        .zip(roles)
        .map(|(face, role)| {
            Ok(NamedFace {
                reference: FaceRef::primitive(feature.id, role),
                surface: surface_kind(primitive, &face.surface()),
            })
        })
        .collect()
}

fn validate_extrusion_role_coverage(
    feature: &Feature,
    region: &SketchRegion2D,
    roles: &[PrimitiveFace],
) -> Result<(), KernelError> {
    let covered = |expected: &PrimitiveFace| roles.iter().any(|role| role == expected);
    if !covered(&PrimitiveFace::StartCap) || !covered(&PrimitiveFace::EndCap) {
        return Err(topology_error(
            feature,
            "extrusion did not preserve both semantic cap faces",
        ));
    }
    for (segment, _) in region.profile.segments.iter().enumerate() {
        let segment = u32::try_from(segment)
            .map_err(|_| topology_error(feature, "extrusion has more than u32::MAX segments"))?;
        if !covered(&PrimitiveFace::ProfileSide { segment }) {
            return Err(topology_error(
                feature,
                format!("extrusion lost outer profile side {segment}"),
            ));
        }
    }
    for (hole, loop_) in region.holes.iter().enumerate() {
        let hole = u32::try_from(hole)
            .map_err(|_| topology_error(feature, "extrusion has more than u32::MAX holes"))?;
        for (segment, _) in loop_.segments.iter().enumerate() {
            let segment = u32::try_from(segment).map_err(|_| {
                topology_error(feature, "extrusion hole has more than u32::MAX segments")
            })?;
            if !covered(&PrimitiveFace::HoleSide { hole, segment }) {
                return Err(topology_error(
                    feature,
                    format!("extrusion lost hole {hole} side {segment}"),
                ));
            }
        }
    }
    Ok(())
}

fn named_faces_from_roles(
    feature: &Feature,
    primitive: &Primitive,
    solid: &Solid,
    roles: Vec<PrimitiveFace>,
) -> Result<Vec<NamedFace>, KernelError> {
    let surfaces = solid
        .face_iter()
        .map(|face| surface_kind(primitive, &face.surface()))
        .collect::<Vec<_>>();

    if roles.len() != surfaces.len() {
        return Err(topology_error(
            feature,
            "primitive face classification did not cover every kernel face",
        ));
    }

    let mut faces = roles
        .clone()
        .into_iter()
        .zip(surfaces)
        .map(|(role, surface)| NamedFace {
            reference: FaceRef::primitive(feature.id, role),
            surface,
        })
        .collect::<Vec<_>>();
    faces.sort_by_key(|face| face.reference.clone());
    faces.dedup_by(|left, right| left.reference == right.reference);
    if faces.len() != solid.face_iter().count() {
        return Err(topology_error(
            feature,
            format!("primitive generated duplicate semantic face names: {roles:?}"),
        ));
    }

    // Restore kernel traversal order after checking uniqueness.
    Ok(roles
        .into_iter()
        .zip(
            solid
                .face_iter()
                .map(|face| surface_kind(primitive, &face.surface())),
        )
        .map(|(role, surface)| NamedFace {
            reference: FaceRef::primitive(feature.id, role),
            surface,
        })
        .collect())
}

pub(crate) fn name_boolean_faces(
    feature: &Feature,
    solid: &Solid,
    sources: [&NamedSolid; 2],
    source_geometries: [BooleanSourceGeometry; 2],
    tolerance: f64,
) -> Result<Vec<NamedFace>, KernelError> {
    let mut source_by_surface = BTreeMap::<String, Vec<&NamedFace>>::new();
    for (source, geometry) in sources.into_iter().zip(source_geometries) {
        for (face, named) in source.solid.face_iter().zip(&source.faces) {
            let source_surface = face.surface();
            let surface = match &geometry {
                BooleanSourceGeometry::Identity => source_surface,
                BooleanSourceGeometry::Translation(translation) => {
                    source_surface.transformed(Matrix4::from_translation(*translation))
                }
                BooleanSourceGeometry::SurfaceReplacements(replacements) => {
                    let source_key = surface_key(&source_surface);
                    let matches = replacements
                        .iter()
                        .filter(|(candidate, _)| surface_key(candidate) == source_key)
                        .collect::<Vec<_>>();
                    match matches.as_slice() {
                        [] => source_surface,
                        [(_, replacement)] => replacement.clone(),
                        _ => {
                            return Err(topology_error(
                                feature,
                                "boolean healing produced ambiguous source-surface replacements",
                            ));
                        }
                    }
                }
            };
            source_by_surface
                .entry(surface_key(&surface))
                .or_default()
                .push(named);
        }
    }

    let meshes = tessellate_faces(feature, solid, tolerance)?;
    let geometries = meshes.iter().map(face_geometry).collect::<Vec<_>>();
    let mut origins = Vec::with_capacity(geometries.len());
    let mut surface_kinds = Vec::with_capacity(geometries.len());
    for face in solid.face_iter() {
        let key = surface_key(&face.surface());
        let Some(candidates) = source_by_surface.get(&key) else {
            return Err(topology_error(
                feature,
                "boolean result contains a face with no traceable upstream surface",
            ));
        };
        let mut references = candidates
            .iter()
            .map(|candidate| candidate.reference.clone())
            .collect::<Vec<_>>();
        references.sort_unstable();
        references.dedup();
        origins.push(references);
        surface_kinds.push(candidates[0].surface);
    }

    let mut groups = BTreeMap::<Vec<FaceRef>, Vec<usize>>::new();
    for (index, source_faces) in origins.iter().enumerate() {
        groups.entry(source_faces.clone()).or_default().push(index);
    }

    let mut fragments = vec![0_u32; origins.len()];
    for indices in groups.values_mut() {
        indices.sort_by(|left, right| compare_geometry(&geometries[*left], &geometries[*right]));
        for (fragment, index) in indices.iter().copied().enumerate() {
            fragments[index] = u32::try_from(fragment).map_err(|_| {
                topology_error(
                    feature,
                    "a source face was split into more than u32::MAX pieces",
                )
            })?;
        }
    }

    Ok(origins
        .into_iter()
        .zip(fragments)
        .zip(surface_kinds)
        .map(|((sources, fragment), surface)| NamedFace {
            reference: FaceRef::derived_from(feature.id, sources, fragment),
            surface,
        })
        .collect())
}

pub(crate) fn name_edge_modifier_faces(
    feature: &Feature,
    solid: &Solid,
    source: &NamedSolid,
    generated: &[GeneratedFace],
    operation: &str,
    tolerance: f64,
) -> Result<Vec<NamedFace>, KernelError> {
    let mut source_by_surface = BTreeMap::<String, Vec<&NamedFace>>::new();
    for (face, named) in source.solid.face_iter().zip(&source.faces) {
        source_by_surface
            .entry(surface_key(&face.surface()))
            .or_default()
            .push(named);
    }
    let generated_by_surface = generated
        .iter()
        .map(|face| (face.surface_key.as_str(), face))
        .collect::<BTreeMap<_, _>>();
    if generated_by_surface.len() != generated.len() {
        return Err(topology_error(
            feature,
            format!("{operation} generated duplicate supporting surfaces"),
        ));
    }
    let meshes = tessellate_faces(feature, solid, tolerance)?;
    let geometries = meshes.iter().map(face_geometry).collect::<Vec<_>>();
    let mut origins = Vec::with_capacity(geometries.len());
    let mut surface_kinds = Vec::with_capacity(geometries.len());
    let mut resolved_generated = BTreeSet::new();
    for face in solid.face_iter() {
        let key = surface_key(&face.surface());
        if let Some(candidates) = source_by_surface.get(&key) {
            let mut references = candidates
                .iter()
                .map(|candidate| candidate.reference.clone())
                .collect::<Vec<_>>();
            references.sort_unstable();
            references.dedup();
            origins.push(references);
            surface_kinds.push(candidates[0].surface);
        } else if let Some(generated) = generated_by_surface.get(key.as_str()) {
            resolved_generated.insert(key);
            origins.push(generated.sources.clone());
            surface_kinds.push(generated.surface);
        } else {
            return Err(topology_error(
                feature,
                format!("{operation} produced a face with unrecognized lineage"),
            ));
        }
    }
    if resolved_generated.len() != generated.len() {
        return Err(topology_error(
            feature,
            format!(
                "{operation} retained {} of {} generated edge faces",
                resolved_generated.len(),
                generated.len()
            ),
        ));
    }

    let mut groups = BTreeMap::<Vec<FaceRef>, Vec<usize>>::new();
    for (index, source_faces) in origins.iter().enumerate() {
        groups.entry(source_faces.clone()).or_default().push(index);
    }
    let mut fragments = vec![0_u32; origins.len()];
    for indices in groups.values_mut() {
        indices.sort_by(|left, right| compare_geometry(&geometries[*left], &geometries[*right]));
        for (fragment, index) in indices.iter().copied().enumerate() {
            fragments[index] = u32::try_from(fragment).map_err(|_| {
                topology_error(
                    feature,
                    format!("a {operation} source face split into too many fragments"),
                )
            })?;
        }
    }

    Ok(origins
        .into_iter()
        .zip(fragments)
        .zip(surface_kinds)
        .map(|((sources, fragment), surface)| NamedFace {
            reference: FaceRef::derived_from(feature.id, sources, fragment),
            surface,
        })
        .collect())
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedFace {
    surface_key: String,
    sources: Vec<FaceRef>,
    surface: SurfaceKind,
}

pub(crate) fn generated_face(
    face: &truck_modeling::Face,
    edge: &EdgeRef,
    surface: SurfaceKind,
) -> GeneratedFace {
    generated_face_from_surface(&face.surface(), edge, surface)
}

pub(crate) fn generated_face_from_surface(
    face_surface: &Surface,
    edge: &EdgeRef,
    surface: SurfaceKind,
) -> GeneratedFace {
    GeneratedFace {
        surface_key: surface_key(face_surface),
        sources: edge.adjacent_faces.to_vec(),
        surface,
    }
}

pub(crate) fn unique_generated_face(
    feature: &Feature,
    solid: &Solid,
    source: &Solid,
    operation: &str,
) -> Result<truck_modeling::Face, KernelError> {
    let source_surfaces = source
        .face_iter()
        .map(|face| surface_key(&face.surface()))
        .collect::<Vec<_>>();
    let generated = solid
        .face_iter()
        .filter(|face| !source_surfaces.contains(&surface_key(&face.surface())))
        .collect::<Vec<_>>();
    let [face] = generated.as_slice() else {
        return Err(topology_error(
            feature,
            format!(
                "{operation} produced {} generated faces instead of one",
                generated.len()
            ),
        ));
    };
    Ok((*face).clone())
}

type EvaluatedTopology = (
    TriangleMesh,
    Vec<EvaluatedFace>,
    Vec<EvaluatedEdge>,
    Vec<EvaluatedVertex>,
);

pub(crate) fn evaluated_topology(
    feature: &Feature,
    named: &NamedSolid,
    tolerance: f64,
) -> Result<EvaluatedTopology, KernelError> {
    let face_meshes = tessellate_faces(feature, &named.solid, tolerance)?;
    if face_meshes.len() != named.faces.len() {
        return Err(topology_error(
            feature,
            "tessellation does not match the named B-Rep faces",
        ));
    }

    let mut mesh = TriangleMesh::default();
    let mut faces = Vec::with_capacity(face_meshes.len());
    for ((face_mesh, named_face), kernel_face) in face_meshes
        .into_iter()
        .zip(&named.faces)
        .zip(named.solid.face_iter())
    {
        let start = u32::try_from(mesh.triangle_count()).map_err(|_| KernelError::MeshTooLarge)?;
        let geometry = face_geometry(&face_mesh);
        let plane = plane_geometry(&kernel_face.surface());
        append_mesh(&mut mesh, face_mesh)?;
        let end = u32::try_from(mesh.triangle_count()).map_err(|_| KernelError::MeshTooLarge)?;
        faces.push(EvaluatedFace {
            reference: named_face.reference.clone(),
            geometry: FaceGeometry {
                surface: named_face.surface,
                plane,
                ..geometry
            },
            triangles: start..end,
        });
    }
    let (edges, vertices) = evaluated_edges_and_vertices(feature, named, tolerance)?;
    Ok((mesh, faces, edges, vertices))
}

#[derive(Debug)]
struct EdgeCandidate {
    edge: Edge,
    vertices: [Vertex; 2],
    adjacent_faces: [FaceRef; 2],
    geometry: EdgeGeometry,
}

fn evaluated_edges_and_vertices(
    feature: &Feature,
    named: &NamedSolid,
    tolerance: f64,
) -> Result<(Vec<EvaluatedEdge>, Vec<EvaluatedVertex>), KernelError> {
    let (candidates, fragments) = edge_candidates(feature, named, tolerance)?;
    let edges = candidates
        .iter()
        .zip(fragments)
        .map(|(candidate, fragment)| EvaluatedEdge {
            reference: EdgeRef::new(
                feature.id,
                candidate.adjacent_faces[0].clone(),
                candidate.adjacent_faces[1].clone(),
                fragment,
            ),
            geometry: candidate.geometry.clone(),
        })
        .collect::<Vec<_>>();
    let vertices = evaluated_vertices(feature, &candidates, &edges)?;
    Ok((edges, vertices))
}

fn edge_candidates(
    feature: &Feature,
    named: &NamedSolid,
    tolerance: f64,
) -> Result<(Vec<EdgeCandidate>, Vec<u32>), KernelError> {
    let mut by_id = HashMap::<EdgeID, (Edge, Vec<FaceRef>)>::new();
    let mut order = Vec::new();
    for (face, named_face) in named.solid.face_iter().zip(&named.faces) {
        for edge in face.edge_iter() {
            let id = edge.id();
            let entry = by_id.entry(id).or_insert_with(|| {
                order.push(id);
                (edge.clone(), Vec::new())
            });
            entry.1.push(named_face.reference.clone());
        }
    }

    let mut candidates = Vec::with_capacity(order.len());
    for id in order {
        let (edge, mut adjacent_faces) = by_id
            .remove(&id)
            .ok_or_else(|| topology_error(feature, "edge adjacency index became inconsistent"))?;
        if adjacent_faces.len() != 2 {
            return Err(topology_error(
                feature,
                format!(
                    "B-Rep edge has {} face incidences instead of two",
                    adjacent_faces.len()
                ),
            ));
        }
        adjacent_faces.sort_unstable();
        let adjacent_faces: [FaceRef; 2] = adjacent_faces.try_into().map_err(|_| {
            topology_error(feature, "B-Rep edge adjacency could not be canonicalized")
        })?;
        candidates.push(EdgeCandidate {
            edge: edge.clone(),
            vertices: [edge.front().clone(), edge.back().clone()],
            adjacent_faces,
            geometry: edge_geometry(feature, &edge, tolerance)?,
        });
    }

    let mut groups = BTreeMap::<[FaceRef; 2], Vec<usize>>::new();
    for (index, candidate) in candidates.iter().enumerate() {
        groups
            .entry(candidate.adjacent_faces.clone())
            .or_default()
            .push(index);
    }
    let mut fragments = vec![0_u32; candidates.len()];
    for indices in groups.values_mut() {
        indices.sort_by(|left, right| {
            compare_edge_geometry(&candidates[*left].geometry, &candidates[*right].geometry)
        });
        for (fragment, index) in indices.iter().copied().enumerate() {
            fragments[index] = u32::try_from(fragment).map_err(|_| {
                topology_error(
                    feature,
                    "a face pair shares more than u32::MAX edge fragments",
                )
            })?;
        }
    }

    Ok((candidates, fragments))
}

fn evaluated_vertices(
    feature: &Feature,
    edge_candidates: &[EdgeCandidate],
    edges: &[EvaluatedEdge],
) -> Result<Vec<EvaluatedVertex>, KernelError> {
    let mut by_id = HashMap::<VertexID, (Vertex, Vec<EdgeRef>)>::new();
    let mut order = Vec::new();
    for (candidate, edge) in edge_candidates.iter().zip(edges) {
        for vertex in &candidate.vertices {
            let id = vertex.id();
            let entry = by_id.entry(id).or_insert_with(|| {
                order.push(id);
                (vertex.clone(), Vec::new())
            });
            entry.1.push(edge.reference.clone());
        }
    }

    let mut candidates = Vec::with_capacity(order.len());
    for id in order {
        let (vertex, incident_edges) = by_id
            .remove(&id)
            .ok_or_else(|| topology_error(feature, "vertex incidence index became inconsistent"))?;
        let position = point_array(vertex.point());
        if !position.iter().all(|value| value.is_finite()) {
            return Err(topology_error(
                feature,
                "B-Rep vertex has a non-finite position",
            ));
        }
        let reference = VertexRef::new(feature.id, incident_edges, 0);
        if reference.incident_edges.is_empty() {
            return Err(topology_error(
                feature,
                "B-Rep vertex has no incident edges",
            ));
        }
        candidates.push((reference.incident_edges, position));
    }

    let mut groups = BTreeMap::<Vec<EdgeRef>, Vec<usize>>::new();
    for (index, (incident_edges, _)) in candidates.iter().enumerate() {
        groups
            .entry(incident_edges.clone())
            .or_default()
            .push(index);
    }
    let mut fragments = vec![0_u32; candidates.len()];
    for indices in groups.values_mut() {
        indices.sort_by(|left, right| compare_point(candidates[*left].1, candidates[*right].1));
        for (fragment, index) in indices.iter().copied().enumerate() {
            fragments[index] = u32::try_from(fragment).map_err(|_| {
                topology_error(
                    feature,
                    "an incident edge set contains more than u32::MAX vertex fragments",
                )
            })?;
        }
    }

    Ok(candidates
        .into_iter()
        .zip(fragments)
        .map(|((incident_edges, position), fragment)| EvaluatedVertex {
            reference: VertexRef::new(feature.id, incident_edges, fragment),
            geometry: VertexGeometry { position },
        })
        .collect())
}

fn edge_geometry(
    feature: &Feature,
    edge: &Edge,
    tolerance: f64,
) -> Result<EdgeGeometry, KernelError> {
    let curve = edge.curve();
    let (parameters, points) = curve.parameter_division(curve.range_tuple(), tolerance);
    let polyline = points.into_iter().map(point_array).collect::<Vec<_>>();
    if polyline.len() < 2 || polyline.iter().flatten().any(|value| !value.is_finite()) {
        return Err(topology_error(
            feature,
            "B-Rep edge could not be sampled into a finite polyline",
        ));
    }
    let polyline_length = polyline
        .windows(2)
        .map(|segment| point_distance(segment[0], segment[1]))
        .sum::<f64>();
    if !polyline_length.is_finite() || polyline_length <= f64::EPSILON {
        return Err(topology_error(feature, "B-Rep edge is degenerate"));
    }
    let endpoints = [
        point_array(edge.front().point()),
        point_array(edge.back().point()),
    ];
    let (curve_kind, length, length_error_estimate) = match &curve {
        Curve::Line(_) => (
            CurveKind::Line,
            point_distance(endpoints[0], endpoints[1]),
            Some(0.0),
        ),
        Curve::BSplineCurve(_) => integrated_curve_length(&curve, &parameters).map_or(
            (CurveKind::BSpline, polyline_length, None),
            |(length, error)| (CurveKind::BSpline, length, Some(error)),
        ),
        Curve::NurbsCurve(_) => integrated_curve_length(&curve, &parameters).map_or(
            (CurveKind::Nurbs, polyline_length, None),
            |(length, error)| (CurveKind::Nurbs, length, Some(error)),
        ),
        Curve::IntersectionCurve(_) => integrated_curve_length(&curve, &parameters).map_or(
            (CurveKind::Intersection, polyline_length, None),
            |(length, error)| (CurveKind::Intersection, length, Some(error)),
        ),
    };
    if !length.is_finite() || length <= f64::EPSILON {
        return Err(topology_error(feature, "B-Rep edge is degenerate"));
    }
    let midpoint = polyline_midpoint(&polyline, polyline_length);
    Ok(EdgeGeometry {
        curve: curve_kind,
        endpoints,
        midpoint,
        length,
        length_error_estimate,
        polyline,
    })
}

fn integrated_curve_length(curve: &Curve, parameters: &[f64]) -> Option<(f64, f64)> {
    const ABSOLUTE_TOLERANCE: f64 = 1.0e-8;
    const MAX_DEPTH: u8 = 12;

    let intervals = parameters
        .windows(2)
        .filter(|range| range[0].total_cmp(&range[1]).is_ne())
        .count();
    if intervals == 0 {
        return None;
    }
    let intervals = u32::try_from(intervals).ok()?;
    let interval_tolerance = ABSOLUTE_TOLERANCE / f64::from(intervals);
    let mut length = 0.0;
    let mut error = 0.0;
    for range in parameters
        .windows(2)
        .filter(|range| range[0].total_cmp(&range[1]).is_ne())
    {
        let (value, estimate) =
            adaptive_gauss_kronrod(curve, range[0], range[1], interval_tolerance, MAX_DEPTH)?;
        length += value;
        error += estimate;
    }
    (length.is_finite() && error.is_finite()).then_some((length, error))
}

fn adaptive_gauss_kronrod(
    curve: &Curve,
    start: f64,
    end: f64,
    tolerance: f64,
    depth: u8,
) -> Option<(f64, f64)> {
    let (value, error) = gauss_kronrod_15(curve, start, end)?;
    if error <= tolerance {
        return Some((value, error));
    }
    if depth == 0 {
        return None;
    }
    let middle = (start + end) * 0.5;
    let (left, left_error) =
        adaptive_gauss_kronrod(curve, start, middle, tolerance * 0.5, depth - 1)?;
    let (right, right_error) =
        adaptive_gauss_kronrod(curve, middle, end, tolerance * 0.5, depth - 1)?;
    Some((left + right, left_error + right_error))
}

fn gauss_kronrod_15(curve: &Curve, start: f64, end: f64) -> Option<(f64, f64)> {
    const ABSCISSA: [f64; 8] = [
        0.991_455_371_120_812_6,
        0.949_107_912_342_758_5,
        0.864_864_423_359_769_1,
        0.741_531_185_599_394_5,
        0.586_087_235_467_691_1,
        0.405_845_151_377_397_2,
        0.207_784_955_007_898_48,
        0.0,
    ];
    const KRONROD_WEIGHT: [f64; 8] = [
        0.022_935_322_010_529_224,
        0.063_092_092_629_978_55,
        0.104_790_010_322_250_19,
        0.140_653_259_715_525_92,
        0.169_004_726_639_267_9,
        0.190_350_578_064_785_4,
        0.204_432_940_075_298_89,
        0.209_482_141_084_727_82,
    ];
    const GAUSS_WEIGHT: [f64; 4] = [
        0.129_484_966_168_869_7,
        0.279_705_391_489_276_67,
        0.381_830_050_505_118_9,
        0.417_959_183_673_469_4,
    ];

    let center = (start + end) * 0.5;
    let half_length = (end - start) * 0.5;
    let center_speed = curve.der(center).magnitude();
    if !center_speed.is_finite() {
        return None;
    }
    let mut kronrod = KRONROD_WEIGHT[7] * center_speed;
    let mut gauss = GAUSS_WEIGHT[3] * center_speed;
    for index in 0..7 {
        let offset = half_length * ABSCISSA[index];
        let first = curve.der(center - offset).magnitude();
        let second = curve.der(center + offset).magnitude();
        if !first.is_finite() || !second.is_finite() {
            return None;
        }
        let pair = first + second;
        kronrod += KRONROD_WEIGHT[index] * pair;
        match index {
            1 => gauss += GAUSS_WEIGHT[0] * pair,
            3 => gauss += GAUSS_WEIGHT[1] * pair,
            5 => gauss += GAUSS_WEIGHT[2] * pair,
            _ => {}
        }
    }
    let kronrod = kronrod * half_length.abs();
    let gauss = gauss * half_length.abs();
    Some((kronrod, (kronrod - gauss).abs()))
}

fn polyline_midpoint(polyline: &[[f64; 3]], length: f64) -> [f64; 3] {
    let target = length / 2.0;
    let mut traversed = 0.0;
    for segment in polyline.windows(2) {
        let segment_length = point_distance(segment[0], segment[1]);
        if traversed + segment_length >= target {
            let factor = (target - traversed) / segment_length;
            return std::array::from_fn(|axis| {
                (segment[1][axis] - segment[0][axis]).mul_add(factor, segment[0][axis])
            });
        }
        traversed += segment_length;
    }
    *polyline
        .last()
        .expect("edge polyline contains at least two points")
}

fn compare_edge_geometry(left: &EdgeGeometry, right: &EdgeGeometry) -> std::cmp::Ordering {
    compare_point(left.midpoint, right.midpoint)
        .then_with(|| left.length.total_cmp(&right.length))
        .then_with(|| compare_point(canonical_start(left), canonical_start(right)))
        .then_with(|| compare_point(canonical_end(left), canonical_end(right)))
}

fn canonical_start(geometry: &EdgeGeometry) -> [f64; 3] {
    if compare_point(geometry.endpoints[0], geometry.endpoints[1]).is_le() {
        geometry.endpoints[0]
    } else {
        geometry.endpoints[1]
    }
}

fn canonical_end(geometry: &EdgeGeometry) -> [f64; 3] {
    if compare_point(geometry.endpoints[0], geometry.endpoints[1]).is_le() {
        geometry.endpoints[1]
    } else {
        geometry.endpoints[0]
    }
}

fn compare_point(left: [f64; 3], right: [f64; 3]) -> std::cmp::Ordering {
    left[0]
        .total_cmp(&right[0])
        .then_with(|| left[1].total_cmp(&right[1]))
        .then_with(|| left[2].total_cmp(&right[2]))
}

fn point_distance(left: [f64; 3], right: [f64; 3]) -> f64 {
    (left[0] - right[0])
        .mul_add(
            left[0] - right[0],
            (left[1] - right[1]).mul_add(
                left[1] - right[1],
                (left[2] - right[2]) * (left[2] - right[2]),
            ),
        )
        .sqrt()
}

fn point_array(point: Point3) -> [f64; 3] {
    [point.x, point.y, point.z]
}

fn primitive_roles(
    feature: &Feature,
    primitive: &Primitive,
    geometries: &[FaceGeometry],
    tolerance: f64,
) -> Result<Vec<PrimitiveFace>, KernelError> {
    match primitive {
        Primitive::Box { .. } => geometries
            .iter()
            .map(|geometry| box_role(feature, geometry))
            .collect(),
        Primitive::Cylinder { .. } | Primitive::Cone { .. } => {
            cap_and_lateral_roles(feature, geometries, tolerance)
        }
        Primitive::Extrusion {
            profile, height, ..
        } => extrusion_roles(feature, geometries, profile, &[], *height, tolerance),
        Primitive::ExtrusionFromSketch { region, height, .. } => extrusion_roles_in_frame(
            feature,
            geometries,
            region,
            *height,
            feature.translation.as_array(),
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            tolerance,
        ),
        Primitive::Sphere { .. }
        | Primitive::Torus { .. }
        | Primitive::RevolveFromSketch { .. }
        | Primitive::ImportedStep { .. } => geometries
            .iter()
            .enumerate()
            .map(|(index, _)| {
                u32::try_from(index)
                    .map(|index| PrimitiveFace::Patch { index })
                    .map_err(|_| topology_error(feature, "primitive has more than u32::MAX faces"))
            })
            .collect(),
        Primitive::LoftFromSketches { .. } => Err(topology_error(
            feature,
            "loft faces must be named from ordered section semantics",
        )),
        Primitive::Boolean { .. } => Err(topology_error(
            feature,
            "boolean faces must be named from upstream topology",
        )),
        Primitive::Chamfer { .. } => Err(topology_error(
            feature,
            "chamfer faces must be named from upstream topology",
        )),
        Primitive::Fillet { .. } => Err(topology_error(
            feature,
            "fillet faces must be named from upstream topology",
        )),
        Primitive::DatumPlane { .. } => Err(topology_error(
            feature,
            "datum planes do not have solid faces",
        )),
        Primitive::DatumPoint { .. } => Err(topology_error(
            feature,
            "datum points do not have solid faces",
        )),
        Primitive::Sketch { .. } => Err(topology_error(
            feature,
            "a sketch does not have solid faces",
        )),
    }
}

fn unique_primitive_roles(
    feature: &Feature,
    mut roles: Vec<PrimitiveFace>,
    geometries: &[FaceGeometry],
    frame: Option<([f64; 3], [f64; 3], [f64; 3])>,
) -> Result<Vec<PrimitiveFace>, KernelError> {
    let mut groups = BTreeMap::<PrimitiveFace, Vec<usize>>::new();
    for (index, role) in roles.iter().cloned().enumerate() {
        groups.entry(role).or_default().push(index);
    }
    for (role, indices) in groups {
        if indices.len() <= 1 {
            continue;
        }
        let mut indices = indices;
        indices.sort_by(|left, right| {
            angular_key(feature, &geometries[*left], frame)
                .total_cmp(&angular_key(feature, &geometries[*right], frame))
                .then_with(|| compare_geometry(&geometries[*left], &geometries[*right]))
        });
        for (patch, index) in indices.into_iter().enumerate() {
            let patch = u32::try_from(patch)
                .map_err(|_| topology_error(feature, "primitive has more than u32::MAX patches"))?;
            roles[index] = match role {
                PrimitiveFace::StartCap => PrimitiveFace::StartCapPatch { patch },
                PrimitiveFace::EndCap => PrimitiveFace::EndCapPatch { patch },
                PrimitiveFace::Lateral => PrimitiveFace::LateralPatch { patch },
                PrimitiveFace::ProfileSide { segment } => {
                    PrimitiveFace::ProfileSidePatch { segment, patch }
                }
                PrimitiveFace::HoleSide { hole, segment } => PrimitiveFace::HoleSidePatch {
                    hole,
                    segment,
                    patch,
                },
                _ => {
                    return Err(topology_error(
                        feature,
                        format!("kernel split unsupported primitive face role {role:?}"),
                    ));
                }
            };
        }
    }
    Ok(roles)
}

fn angular_key(
    feature: &Feature,
    geometry: &FaceGeometry,
    frame: Option<([f64; 3], [f64; 3], [f64; 3])>,
) -> f64 {
    let (x, y) = frame.map_or_else(
        || {
            (
                geometry.centroid[0] - feature.translation.x,
                geometry.centroid[1] - feature.translation.y,
            )
        },
        |(origin, x_dir, y_dir)| {
            let relative = sub(geometry.centroid, origin);
            (dot(relative, x_dir), dot(relative, y_dir))
        },
    );
    y.atan2(x).rem_euclid(std::f64::consts::TAU)
}

fn box_role(feature: &Feature, geometry: &FaceGeometry) -> Result<PrimitiveFace, KernelError> {
    let normal = geometry.mean_normal;
    let (axis, magnitude) = normal
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.abs().total_cmp(&right.abs()))
        .map_or((0, 0.0), |(axis, value)| (axis, value.abs()));
    if magnitude < 0.9 {
        return Err(topology_error(
            feature,
            "box face does not have an axis-aligned outward normal",
        ));
    }
    Ok(match (axis, normal[axis].is_sign_positive()) {
        (0, false) => PrimitiveFace::BoxXMin,
        (0, true) => PrimitiveFace::BoxXMax,
        (1, false) => PrimitiveFace::BoxYMin,
        (1, true) => PrimitiveFace::BoxYMax,
        (2, false) => PrimitiveFace::BoxZMin,
        (2, true) => PrimitiveFace::BoxZMax,
        _ => unreachable!("a three-component vector has only axes 0, 1, and 2"),
    })
}

fn cap_and_lateral_roles(
    feature: &Feature,
    geometries: &[FaceGeometry],
    tolerance: f64,
) -> Result<Vec<PrimitiveFace>, KernelError> {
    let center_z = feature.translation.z
        + match feature.primitive {
            Primitive::Cylinder { height, .. } | Primitive::Cone { height, .. } => height / 2.0,
            _ => unreachable!("only cylinders and cones use cap_and_lateral_roles"),
        };
    geometries
        .iter()
        .map(|geometry| {
            if geometry.mean_normal[2].abs() > 0.9 {
                Ok(if geometry.centroid[2] < center_z {
                    PrimitiveFace::StartCap
                } else {
                    PrimitiveFace::EndCap
                })
            } else {
                Ok(PrimitiveFace::Lateral)
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|roles| {
            if roles
                .iter()
                .any(|role| matches!(role, PrimitiveFace::StartCap))
                && roles
                    .iter()
                    .any(|role| matches!(role, PrimitiveFace::Lateral))
            {
                Ok(roles)
            } else {
                Err(topology_error(
                    feature,
                    format!("could not identify caps and lateral faces at tolerance {tolerance}"),
                ))
            }
        })
}

fn extrusion_roles(
    feature: &Feature,
    geometries: &[FaceGeometry],
    profile: &[[f64; 2]],
    holes: &[Vec<[f64; 2]>],
    height: f64,
    tolerance: f64,
) -> Result<Vec<PrimitiveFace>, KernelError> {
    let region = SketchRegion2D::from_polygons(profile.to_vec(), holes.to_vec());
    extrusion_roles_in_frame(
        feature,
        geometries,
        &region,
        height,
        feature.translation.as_array(),
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        tolerance,
    )
}

#[allow(clippy::too_many_arguments)]
fn extrusion_roles_in_frame(
    feature: &Feature,
    geometries: &[FaceGeometry],
    region: &SketchRegion2D,
    height: f64,
    origin: [f64; 3],
    x_dir: [f64; 3],
    y_dir: [f64; 3],
    normal: [f64; 3],
    tolerance: f64,
) -> Result<Vec<PrimitiveFace>, KernelError> {
    geometries
        .iter()
        .map(|geometry| {
            let relative = sub(geometry.centroid, origin);
            let sweep_distance = dot(relative, normal);
            if sweep_distance.abs() <= tolerance {
                return Ok(PrimitiveFace::StartCap);
            }
            if (sweep_distance - height).abs() <= tolerance {
                return Ok(PrimitiveFace::EndCap);
            }
            let point = [dot(relative, x_dir), dot(relative, y_dir)];
            nearest_profile_side(feature, geometry, point, region, height, tolerance)
        })
        .collect()
}

fn nearest_profile_side(
    feature: &Feature,
    geometry: &FaceGeometry,
    point: [f64; 2],
    region: &SketchRegion2D,
    height: f64,
    tolerance: f64,
) -> Result<PrimitiveFace, KernelError> {
    const AREA_ERROR_TOLERANCE: f64 = 1.0e-6;

    let mut best = None::<(f64, f64, PrimitiveFace)>;
    let distance_tolerance = tolerance.max(1.0e-9).powi(2);
    let mut consider = |loop_: &SketchLoop2D, role: &dyn Fn(u32) -> PrimitiveFace| {
        for (index, segment) in loop_.segments.iter().enumerate() {
            let distance = segment.distance_squared_to(point);
            let expected_area = segment.length() * height.abs();
            let area_error = (geometry.area - expected_area).abs() / expected_area.max(1.0e-9);
            let Ok(index) = u32::try_from(index) else {
                return false;
            };
            if best
                .as_ref()
                .is_none_or(|(best_area_error, best_distance, _)| {
                    area_error + AREA_ERROR_TOLERANCE < *best_area_error
                        || ((area_error - *best_area_error).abs() <= AREA_ERROR_TOLERANCE
                            && distance + distance_tolerance < *best_distance)
                })
            {
                best = Some((area_error, distance, role(index)));
            }
        }
        true
    };
    if !consider(&region.profile, &|segment| PrimitiveFace::ProfileSide {
        segment,
    }) {
        return Err(topology_error(
            feature,
            "extrusion outer profile has more than u32::MAX segments",
        ));
    }
    for (hole, loop_) in region.holes.iter().enumerate() {
        let hole = u32::try_from(hole)
            .map_err(|_| topology_error(feature, "extrusion has more than u32::MAX holes"))?;
        if !consider(loop_, &|segment| PrimitiveFace::HoleSide { hole, segment }) {
            return Err(topology_error(
                feature,
                "extrusion hole has more than u32::MAX segments",
            ));
        }
    }
    best.map(|(_, _, role)| role)
        .ok_or_else(|| topology_error(feature, "extrusion profile contains no segments"))
}

fn tessellate_faces(
    feature: &Feature,
    solid: &Solid,
    tolerance: f64,
) -> Result<Vec<TriangleMesh>, KernelError> {
    let meshed = solid.triangulation(tolerance);
    meshed
        .face_iter()
        .map(|face| {
            let mut polygon = face.surface().ok_or_else(|| {
                topology_error(feature, "kernel could not tessellate a B-Rep face")
            })?;
            if !face.orientation() {
                polygon.invert();
            }
            super::polygon_to_render_mesh(&polygon)
        })
        .collect()
}

fn append_mesh(target: &mut TriangleMesh, source: TriangleMesh) -> Result<(), KernelError> {
    let offset = u32::try_from(target.positions.len()).map_err(|_| KernelError::MeshTooLarge)?;
    target.positions.extend(source.positions);
    target.normals.extend(source.normals);
    target.indices.reserve(source.indices.len());
    for index in source.indices {
        target
            .indices
            .push(index.checked_add(offset).ok_or(KernelError::MeshTooLarge)?);
    }
    Ok(())
}

fn face_geometry(mesh: &TriangleMesh) -> FaceGeometry {
    let mut area = 0.0;
    let mut centroid = [0.0; 3];
    let mut normal = [0.0; 3];
    for triangle in mesh.indices.chunks_exact(3) {
        let points = [
            mesh.positions[triangle[0] as usize].map(f64::from),
            mesh.positions[triangle[1] as usize].map(f64::from),
            mesh.positions[triangle[2] as usize].map(f64::from),
        ];
        let cross = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        let double_area = length(cross);
        let triangle_area = double_area / 2.0;
        let triangle_centroid = [
            (points[0][0] + points[1][0] + points[2][0]) / 3.0,
            (points[0][1] + points[1][1] + points[2][1]) / 3.0,
            (points[0][2] + points[1][2] + points[2][2]) / 3.0,
        ];
        area += triangle_area;
        for axis in 0..3 {
            centroid[axis] += triangle_centroid[axis] * triangle_area;
            normal[axis] += cross[axis] / 2.0;
        }
    }
    if area > f64::EPSILON {
        for value in &mut centroid {
            *value /= area;
        }
    }
    let normal_length = length(normal);
    if normal_length > f64::EPSILON {
        for value in &mut normal {
            *value /= normal_length;
        }
    } else {
        normal = [0.0; 3];
    }
    FaceGeometry {
        surface: SurfaceKind::Swept,
        plane: None,
        area,
        centroid,
        mean_normal: normal,
    }
}

fn plane_geometry(surface: &Surface) -> Option<PlaneGeometry> {
    let Surface::Plane(plane) = surface else {
        return None;
    };
    let origin = point_array(plane.origin());
    let normal = point_array(Point3::origin() + plane.normal());
    let x_direction = normalize(point_array(Point3::origin() + plane.u_axis()))?;
    let y_direction = normalize(cross(normal, x_direction))?;
    (origin
        .iter()
        .chain(&normal)
        .chain(&x_direction)
        .chain(&y_direction)
        .all(|value| value.is_finite()))
    .then_some(PlaneGeometry {
        origin,
        x_direction,
        y_direction,
        normal,
    })
}

fn compare_geometry(left: &FaceGeometry, right: &FaceGeometry) -> std::cmp::Ordering {
    left.centroid[0]
        .total_cmp(&right.centroid[0])
        .then_with(|| left.centroid[1].total_cmp(&right.centroid[1]))
        .then_with(|| left.centroid[2].total_cmp(&right.centroid[2]))
        .then_with(|| left.area.total_cmp(&right.area))
}

fn surface_kind(primitive: &Primitive, surface: &Surface) -> SurfaceKind {
    match surface {
        Surface::Plane(_) => SurfaceKind::Plane,
        Surface::RevolutedCurve(_) => match primitive {
            Primitive::Cylinder { .. } => SurfaceKind::Cylinder,
            Primitive::Cone { .. } => SurfaceKind::Cone,
            Primitive::Sphere { .. } => SurfaceKind::Sphere,
            Primitive::Torus { .. } => SurfaceKind::Torus,
            _ => SurfaceKind::Swept,
        },
        Surface::BSplineSurface(_) | Surface::NurbsSurface(_) => SurfaceKind::Swept,
    }
}

fn surface_key(surface: &Surface) -> String {
    format!("{surface:?}")
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1].mul_add(right[2], -left[2] * right[1]),
        left[2].mul_add(right[0], -left[0] * right[2]),
        left[0].mul_add(right[1], -left[1] * right[0]),
    ]
}

fn length(vector: [f64; 3]) -> f64 {
    vector[0]
        .mul_add(
            vector[0],
            vector[1].mul_add(vector[1], vector[2] * vector[2]),
        )
        .sqrt()
}

fn normalize(vector: [f64; 3]) -> Option<[f64; 3]> {
    let magnitude = length(vector);
    (magnitude.is_finite() && magnitude > f64::EPSILON)
        .then(|| vector.map(|component| component / magnitude))
}

fn topology_error(feature: &Feature, message: impl Into<String>) -> KernelError {
    KernelError::TopologyNaming {
        feature_id: feature.id,
        message: message.into(),
    }
}
