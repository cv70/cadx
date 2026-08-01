use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

use bytemuck::{Pod, Zeroable};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use glam::{DVec3, Vec3, Vec4, camera};
use wgpu::util::DeviceExt;

use cadx_core::{
    domain::FeatureId,
    kernel::{EvaluatedScene, EvaluatedSketch},
    topology::{EdgeRef, FaceRef, VertexRef},
};

mod sketch_annotations;

pub use sketch_annotations::{
    ScreenSketchAnnotation, layout_sketch_annotations, paint_sketch_annotations,
    pick_sketch_dimension,
};

const SAMPLE_COUNT: u32 = 4;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
}

impl GpuVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4];

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
}

#[derive(Debug, Default)]
struct SceneBuffers {
    revision: u64,
    solid_vertices: Vec<GpuVertex>,
    solid_indices: Vec<u32>,
    grid_vertices: Vec<GpuVertex>,
    topology_vertices: Vec<GpuVertex>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TopologySelection<'a> {
    pub face: Option<&'a FaceRef>,
    pub edges: &'a [EdgeRef],
    pub vertex: Option<&'a VertexRef>,
    pub measurement_faces: &'a [FaceRef],
    pub measurement_edges: &'a [EdgeRef],
    pub measurement_vertices: &'a [VertexRef],
    pub measurement_guides: &'a [[[f64; 3]; 2]],
}

#[derive(Debug, Clone)]
pub struct ViewportScene {
    buffers: Arc<RwLock<SceneBuffers>>,
}

impl Default for ViewportScene {
    fn default() -> Self {
        let buffers = SceneBuffers {
            grid_vertices: build_grid(120.0, 10.0),
            ..Default::default()
        };
        Self {
            buffers: Arc::new(RwLock::new(buffers)),
        }
    }
}

impl ViewportScene {
    pub fn update(&self, scene: &EvaluatedScene, selected: Option<FeatureId>) {
        self.update_with_face(scene, selected, None);
    }

    pub fn update_with_face(
        &self,
        scene: &EvaluatedScene,
        selected: Option<FeatureId>,
        selected_face: Option<&FaceRef>,
    ) {
        self.update_with_topology(
            scene,
            selected,
            TopologySelection {
                face: selected_face,
                ..TopologySelection::default()
            },
        );
    }

    pub fn update_with_topology(
        &self,
        scene: &EvaluatedScene,
        selected: Option<FeatureId>,
        topology: TopologySelection<'_>,
    ) {
        let Ok(mut buffers) = self.buffers.write() else {
            return;
        };
        buffers.solid_vertices.clear();
        buffers.solid_indices.clear();
        buffers.topology_vertices.clear();

        for part in &scene.parts {
            let Ok(base_index) = u32::try_from(buffers.solid_vertices.len()) else {
                buffers.solid_vertices.clear();
                buffers.solid_indices.clear();
                return;
            };
            let highlighted_vertices = topology
                .face
                .filter(|face| face.feature_id == part.feature_id)
                .and_then(|reference| part.faces.iter().find(|face| &face.reference == reference))
                .map_or_else(HashSet::new, |face| {
                    let mut vertices = HashSet::new();
                    let start = face.triangles.start as usize * 3;
                    let end = face.triangles.end as usize * 3;
                    for index in part.mesh.indices.get(start..end).unwrap_or_default() {
                        vertices.insert(*index);
                    }
                    vertices
                });
            let mut measurement_vertices = HashSet::new();
            for reference in topology
                .measurement_faces
                .iter()
                .filter(|reference| reference.feature_id == part.feature_id)
            {
                if let Some(face) = part.face(reference) {
                    let start = face.triangles.start as usize * 3;
                    let end = face.triangles.end as usize * 3;
                    for index in part.mesh.indices.get(start..end).unwrap_or_default() {
                        measurement_vertices.insert(*index);
                    }
                }
            }
            let mut color = part.color;
            if selected == Some(part.feature_id) {
                color = [
                    (color[0] * 1.25).min(1.0),
                    (color[1] * 1.25).min(1.0),
                    (color[2] * 1.25).min(1.0),
                    1.0,
                ];
            }
            buffers.solid_vertices.extend(
                part.mesh
                    .positions
                    .iter()
                    .zip(&part.mesh.normals)
                    .enumerate()
                    .map(|(index, (&position, &normal))| {
                        let highlighted = u32::try_from(index)
                            .is_ok_and(|index| highlighted_vertices.contains(&index));
                        let measured = u32::try_from(index)
                            .is_ok_and(|index| measurement_vertices.contains(&index));
                        let vertex_color = if highlighted {
                            [1.0, 0.76, 0.18, 1.0]
                        } else if measured {
                            [0.18, 0.82, 0.92, 1.0]
                        } else {
                            color
                        };
                        GpuVertex {
                            position,
                            normal,
                            color: vertex_color,
                        }
                    }),
            );
            buffers
                .solid_indices
                .extend(part.mesh.indices.iter().map(|index| base_index + index));

            for reference in topology
                .measurement_edges
                .iter()
                .filter(|reference| reference.feature_id == part.feature_id)
            {
                if let Some(edge) = part.edge(reference) {
                    append_polyline(
                        &mut buffers.topology_vertices,
                        &edge.geometry.polyline,
                        [0.18, 0.82, 0.92, 1.0],
                    );
                }
            }
            for reference in topology
                .edges
                .iter()
                .filter(|reference| reference.feature_id == part.feature_id)
            {
                if let Some(edge) = part.edge(reference) {
                    append_polyline(
                        &mut buffers.topology_vertices,
                        &edge.geometry.polyline,
                        [1.0, 0.72, 0.12, 1.0],
                    );
                }
            }
            for reference in topology
                .measurement_vertices
                .iter()
                .filter(|reference| reference.feature_id == part.feature_id)
            {
                if let Some(vertex) = part.vertex(reference) {
                    append_vertex_marker(
                        &mut buffers.topology_vertices,
                        vertex.geometry.position,
                        vertex_marker_size(part, reference),
                        [0.18, 0.82, 0.92, 1.0],
                    );
                }
            }
            if let Some(reference) = topology
                .vertex
                .filter(|reference| reference.feature_id == part.feature_id)
                && let Some(vertex) = part.vertex(reference)
            {
                append_vertex_marker(
                    &mut buffers.topology_vertices,
                    vertex.geometry.position,
                    vertex_marker_size(part, reference),
                    [1.0, 0.72, 0.12, 1.0],
                );
            }
        }
        let datum_extent = reference_extent(scene);
        let datum_point_radius = (datum_extent * 0.12).clamp(0.25, 3.0);
        let sketch_point_radius = (datum_extent * 0.035).clamp(0.15, 1.5);
        for sketch in &scene.sketches {
            let color = reference_color(sketch.color, selected == Some(sketch.feature_id));
            for sketch_loop in sketch_loops(sketch) {
                for segment in sketch_segments(sketch_loop) {
                    append_polyline(&mut buffers.topology_vertices, &segment, color);
                }
                for point in sketch_loop {
                    for segment in sketch_point_segments(
                        *point,
                        sketch.x_direction,
                        sketch.y_direction,
                        sketch_point_radius,
                    ) {
                        append_polyline(&mut buffers.topology_vertices, &segment, color);
                    }
                }
            }
            let construction_color = if selected == Some(sketch.feature_id) {
                [1.0, 0.72, 0.12, 1.0]
            } else {
                [0.38, 0.66, 0.72, 1.0]
            };
            for polyline in &sketch.construction {
                for segment in construction_segments(polyline) {
                    append_polyline(&mut buffers.topology_vertices, &segment, construction_color);
                }
            }
        }
        for plane in &scene.datum_planes {
            let color = reference_color(plane.color, selected == Some(plane.feature_id));
            for segment in datum_plane_segments(
                plane.origin,
                plane.x_direction,
                plane.y_direction,
                plane.normal,
                datum_extent,
            ) {
                append_polyline(&mut buffers.topology_vertices, &segment, color);
            }
        }
        for point in &scene.datum_points {
            append_vertex_marker(
                &mut buffers.topology_vertices,
                point.position,
                datum_point_radius,
                reference_color(point.color, selected == Some(point.feature_id)),
            );
        }
        for guide in topology.measurement_guides {
            append_polyline(
                &mut buffers.topology_vertices,
                guide,
                [0.18, 0.82, 0.92, 1.0],
            );
        }
        buffers.revision = buffers.revision.wrapping_add(1);
    }
}

fn reference_color(color: [f32; 4], selected: bool) -> [f32; 4] {
    if selected {
        [1.0, 0.72, 0.12, 1.0]
    } else {
        [color[0], color[1], color[2], 1.0]
    }
}

fn reference_extent(scene: &EvaluatedScene) -> f64 {
    let mut minimum = DVec3::splat(f64::INFINITY);
    let mut maximum = DVec3::splat(f64::NEG_INFINITY);
    let mut has_geometry = false;
    for position in scene
        .parts
        .iter()
        .flat_map(|part| &part.mesh.positions)
        .map(|point| point.map(f64::from))
        .chain(
            scene
                .sketches
                .iter()
                .flat_map(sketch_loops)
                .flatten()
                .copied(),
        )
        .chain(
            scene
                .sketches
                .iter()
                .flat_map(|sketch| &sketch.construction)
                .flatten()
                .copied(),
        )
        .chain(scene.datum_points.iter().map(|point| point.position))
        .chain(scene.datum_planes.iter().map(|plane| plane.origin))
    {
        let point = DVec3::from_array(position);
        minimum = minimum.min(point);
        maximum = maximum.max(point);
        has_geometry = true;
    }
    if !has_geometry {
        return 10.0;
    }
    ((maximum - minimum).length() * 0.15).clamp(2.0, 30.0)
}

fn sketch_segments(profile: &[[f64; 3]]) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
    (0..profile.len()).map(|index| [profile[index], profile[(index + 1) % profile.len()]])
}

fn construction_segments(profile: &[[f64; 3]]) -> impl Iterator<Item = [[f64; 3]; 2]> + '_ {
    profile.windows(2).map(|points| [points[0], points[1]])
}

fn sketch_loops(sketch: &EvaluatedSketch) -> impl Iterator<Item = &[[f64; 3]]> {
    std::iter::once(sketch.profile.as_slice()).chain(sketch.holes.iter().map(Vec::as_slice))
}

fn sketch_point_segments(
    point: [f64; 3],
    x_direction: [f64; 3],
    y_direction: [f64; 3],
    radius: f64,
) -> [[[f64; 3]; 2]; 2] {
    let endpoint = |direction: [f64; 3], sign: f64| {
        std::array::from_fn(|axis| direction[axis].mul_add(radius * sign, point[axis]))
    };
    [
        [endpoint(x_direction, -1.0), endpoint(x_direction, 1.0)],
        [endpoint(y_direction, -1.0), endpoint(y_direction, 1.0)],
    ]
}

fn datum_plane_segments(
    origin: [f64; 3],
    x_direction: [f64; 3],
    y_direction: [f64; 3],
    normal: [f64; 3],
    extent: f64,
) -> [[[f64; 3]; 2]; 5] {
    let origin = DVec3::from_array(origin);
    let normal = DVec3::from_array(normal).normalize_or_zero();
    let first = DVec3::from_array(x_direction).normalize_or_zero() * extent;
    let second = DVec3::from_array(y_direction).normalize_or_zero() * extent;
    let corners = [
        origin - first - second,
        origin + first - second,
        origin + first + second,
        origin - first + second,
    ];
    [
        [corners[0].to_array(), corners[1].to_array()],
        [corners[1].to_array(), corners[2].to_array()],
        [corners[2].to_array(), corners[3].to_array()],
        [corners[3].to_array(), corners[0].to_array()],
        [
            origin.to_array(),
            (origin + normal * extent * 0.45).to_array(),
        ],
    ]
}

fn vertex_marker_size(part: &cadx_core::kernel::EvaluatedPart, reference: &VertexRef) -> f64 {
    reference
        .incident_edges
        .iter()
        .filter_map(|edge| part.edge(edge))
        .map(|edge| edge.geometry.length)
        .fold(f64::INFINITY, f64::min)
        .mul_add(0.08, 0.0)
        .clamp(0.25, 3.0)
}

#[allow(clippy::cast_possible_truncation)]
fn append_polyline(target: &mut Vec<GpuVertex>, points: &[[f64; 3]], color: [f32; 4]) {
    for segment in points.windows(2) {
        for point in segment {
            target.push(GpuVertex {
                position: point.map(|value| value as f32),
                normal: [0.0, 0.0, 1.0],
                color,
            });
        }
    }
}

fn append_vertex_marker(
    target: &mut Vec<GpuVertex>,
    center: [f64; 3],
    radius: f64,
    color: [f32; 4],
) {
    for axis in 0..3 {
        let mut start = center;
        let mut end = center;
        start[axis] -= radius;
        end[axis] += radius;
        append_polyline(target, &[start, end], color);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            yaw: -0.75,
            pitch: -0.55,
            distance: 105.0,
            target: Vec3::new(0.0, 0.0, 10.0),
        }
    }
}

impl OrbitCamera {
    pub fn orbit(&mut self, delta: egui::Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.5, 1.5);
    }

    pub fn pan(&mut self, delta: egui::Vec2) {
        let forward = self.forward();
        let right = forward.cross(Vec3::Z).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let scale = self.distance * 0.0015;
        self.target += right * (-delta.x * scale) + up * (delta.y * scale);
    }

    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance * (-delta * 0.0015).exp()).clamp(2.0, 4_000.0);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Fits the camera to the visible evaluated scene.
    #[allow(clippy::cast_possible_truncation)]
    pub fn frame_scene(&mut self, scene: &EvaluatedScene) {
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        let mut has_geometry = false;
        for part in &scene.parts {
            for position in &part.mesh.positions {
                let point = Vec3::from_array(*position);
                minimum = minimum.min(point);
                maximum = maximum.max(point);
                has_geometry = true;
            }
        }
        for sketch in &scene.sketches {
            for sketch_loop in sketch_loops(sketch) {
                for position in sketch_loop {
                    let point = Vec3::from_array(position.map(|value| value as f32));
                    minimum = minimum.min(point);
                    maximum = maximum.max(point);
                    has_geometry = true;
                }
            }
            for polyline in &sketch.construction {
                for position in polyline {
                    let point = Vec3::from_array(position.map(|value| value as f32));
                    minimum = minimum.min(point);
                    maximum = maximum.max(point);
                    has_geometry = true;
                }
            }
        }
        let datum_extent = reference_extent(scene);
        for plane in &scene.datum_planes {
            for segment in datum_plane_segments(
                plane.origin,
                plane.x_direction,
                plane.y_direction,
                plane.normal,
                datum_extent,
            ) {
                for position in segment {
                    let point = Vec3::from_array(position.map(|value| value as f32));
                    minimum = minimum.min(point);
                    maximum = maximum.max(point);
                    has_geometry = true;
                }
            }
        }
        for point in &scene.datum_points {
            let point = Vec3::from_array(point.position.map(|value| value as f32));
            minimum = minimum.min(point);
            maximum = maximum.max(point);
            has_geometry = true;
        }
        if !has_geometry {
            self.reset();
            return;
        }
        self.target = (minimum + maximum) * 0.5;
        let radius = (maximum - minimum).length() * 0.5;
        self.distance = (radius / 0.382_683_43 * 1.2).clamp(2.0, 4_000.0);
    }

    fn forward(self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
        )
    }

    fn view_projection(self, aspect: f32) -> [[f32; 4]; 4] {
        let eye = self.target - self.forward() * self.distance;
        let view = camera::rh::view::look_at_mat4(eye, self.target, Vec3::Z);
        let projection = camera::rh::proj::directx::perspective(
            45_f32.to_radians(),
            aspect.max(0.01),
            0.1,
            10_000.0,
        );
        (projection * view).to_cols_array_2d()
    }
}

/// Selects the closest visible part under a viewport pointer.
///
/// The renderer owns the same camera projection used to draw the scene, so
/// selection remains accurate after orbit, pan, zoom, and window resizing.
#[must_use]
pub fn pick_feature(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<FeatureId> {
    pick_reference_feature(scene, rect, pointer, camera)
        .or_else(|| pick_hit(scene, rect, pointer, camera).map(|(feature_id, _)| feature_id))
}

fn pick_reference_feature(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<FeatureId> {
    const POINT_PICK_RADIUS: f32 = 9.0;
    const PLANE_PICK_RADIUS: f32 = 7.0;
    const SKETCH_PICK_RADIUS: f32 = 7.0;
    let (view_projection, _, surface_depth) = topology_pick_context(scene, rect, pointer, camera)?;
    let mut closest: Option<(f32, f32, FeatureId)> = None;
    let mut consider = |distance: f32, depth: f32, feature_id: FeatureId, radius: f32| {
        if distance > radius || surface_depth.is_some_and(|surface| depth > surface + 0.002) {
            return;
        }
        if closest
            .as_ref()
            .is_none_or(|(current_distance, current_depth, _)| {
                pick_is_closer(distance, depth, *current_distance, *current_depth)
            })
        {
            closest = Some((distance, depth, feature_id));
        }
    };
    for point in &scene.datum_points {
        if let Some((screen, depth)) = project_topology_point(point.position, rect, view_projection)
        {
            consider(
                screen.distance(pointer),
                depth,
                point.feature_id,
                POINT_PICK_RADIUS,
            );
        }
    }
    for sketch in &scene.sketches {
        for sketch_loop in sketch_loops(sketch) {
            for segment in sketch_segments(sketch_loop) {
                let (Some(first), Some(second)) = (
                    project_topology_point(segment[0], rect, view_projection),
                    project_topology_point(segment[1], rect, view_projection),
                ) else {
                    continue;
                };
                let (distance, factor) = point_segment_distance(pointer, first.0, second.0);
                let depth = (second.1 - first.1).mul_add(factor, first.1);
                consider(distance, depth, sketch.feature_id, SKETCH_PICK_RADIUS);
            }
        }
        for polyline in &sketch.construction {
            for segment in construction_segments(polyline) {
                let (Some(first), Some(second)) = (
                    project_topology_point(segment[0], rect, view_projection),
                    project_topology_point(segment[1], rect, view_projection),
                ) else {
                    continue;
                };
                let (distance, factor) = point_segment_distance(pointer, first.0, second.0);
                let depth = (second.1 - first.1).mul_add(factor, first.1);
                consider(distance, depth, sketch.feature_id, SKETCH_PICK_RADIUS);
            }
        }
    }
    let extent = reference_extent(scene);
    for plane in &scene.datum_planes {
        for segment in datum_plane_segments(
            plane.origin,
            plane.x_direction,
            plane.y_direction,
            plane.normal,
            extent,
        ) {
            let (Some(first), Some(second)) = (
                project_topology_point(segment[0], rect, view_projection),
                project_topology_point(segment[1], rect, view_projection),
            ) else {
                continue;
            };
            let (distance, factor) = point_segment_distance(pointer, first.0, second.0);
            let depth = (second.1 - first.1).mul_add(factor, first.1);
            consider(distance, depth, plane.feature_id, PLANE_PICK_RADIUS);
        }
    }
    closest.map(|(_, _, feature_id)| feature_id)
}

/// Selects the closest persistent topological face under a viewport pointer.
#[must_use]
pub fn pick_face(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<FaceRef> {
    pick_hit(scene, rect, pointer, camera).and_then(|(_, face)| face)
}

/// Selects the nearest visible persistent edge within a screen-space radius.
#[must_use]
pub fn pick_edge(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<EdgeRef> {
    const PICK_RADIUS: f32 = 7.0;
    let (view_projection, surface_feature, surface_depth) =
        topology_pick_context(scene, rect, pointer, camera)?;
    let mut closest: Option<(f32, f32, EdgeRef)> = None;
    for part in &scene.parts {
        if surface_feature.is_some_and(|feature| feature != part.feature_id) {
            continue;
        }
        for edge in &part.edges {
            for segment in edge.geometry.polyline.windows(2) {
                let (Some(first), Some(second)) = (
                    project_topology_point(segment[0], rect, view_projection),
                    project_topology_point(segment[1], rect, view_projection),
                ) else {
                    continue;
                };
                let (distance, factor) = point_segment_distance(pointer, first.0, second.0);
                let depth = (second.1 - first.1).mul_add(factor, first.1);
                if distance > PICK_RADIUS
                    || surface_depth.is_some_and(|surface| depth > surface + 0.002)
                {
                    continue;
                }
                if closest
                    .as_ref()
                    .is_none_or(|(current_distance, current_depth, _)| {
                        pick_is_closer(distance, depth, *current_distance, *current_depth)
                    })
                {
                    closest = Some((distance, depth, edge.reference.clone()));
                }
            }
        }
    }
    closest.map(|(_, _, reference)| reference)
}

/// Selects the nearest visible persistent vertex within a screen-space radius.
#[must_use]
pub fn pick_vertex(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<VertexRef> {
    const PICK_RADIUS: f32 = 8.0;
    let (view_projection, surface_feature, surface_depth) =
        topology_pick_context(scene, rect, pointer, camera)?;
    let mut closest: Option<(f32, f32, VertexRef)> = None;
    for part in &scene.parts {
        if surface_feature.is_some_and(|feature| feature != part.feature_id) {
            continue;
        }
        for vertex in &part.vertices {
            let Some((screen, depth)) =
                project_topology_point(vertex.geometry.position, rect, view_projection)
            else {
                continue;
            };
            let distance = screen.distance(pointer);
            if distance > PICK_RADIUS
                || surface_depth.is_some_and(|surface| depth > surface + 0.002)
            {
                continue;
            }
            if closest
                .as_ref()
                .is_none_or(|(current_distance, current_depth, _)| {
                    pick_is_closer(distance, depth, *current_distance, *current_depth)
                })
            {
                closest = Some((distance, depth, vertex.reference.clone()));
            }
        }
    }
    closest.map(|(_, _, reference)| reference)
}

fn topology_pick_context(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<(glam::Mat4, Option<FeatureId>, Option<f32>)> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || !rect.contains(pointer) {
        return None;
    }
    let view_projection =
        glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
    let surface = pick_surface_hit(scene, rect, pointer, camera);
    let surface_feature = surface.as_ref().map(|(feature, _, _)| *feature);
    let surface_depth = surface.and_then(|(_, distance, _)| {
        let (origin, direction) = viewport_ray(rect, pointer, view_projection)?;
        project_topology_point(
            (origin + direction * distance).to_array().map(f64::from),
            rect,
            view_projection,
        )
        .map(|(_, depth)| depth)
    });
    Some((view_projection, surface_feature, surface_depth))
}

#[allow(clippy::cast_possible_truncation)]
fn project_topology_point(
    point: [f64; 3],
    rect: egui::Rect,
    view_projection: glam::Mat4,
) -> Option<(egui::Pos2, f32)> {
    let clip = view_projection * Vec4::new(point[0] as f32, point[1] as f32, point[2] as f32, 1.0);
    if clip.w <= f32::EPSILON {
        return None;
    }
    let projected = clip.truncate() / clip.w;
    if !(0.0..=1.0).contains(&projected.z) {
        return None;
    }
    Some((
        egui::pos2(
            rect.left() + (projected.x + 1.0) * 0.5 * rect.width(),
            rect.top() + (1.0 - projected.y) * 0.5 * rect.height(),
        ),
        projected.z,
    ))
}

fn point_segment_distance(point: egui::Pos2, start: egui::Pos2, end: egui::Pos2) -> (f32, f32) {
    let segment = end - start;
    let length_squared = segment.length_sq();
    if length_squared <= f32::EPSILON {
        return (point.distance(start), 0.0);
    }
    let factor = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    (point.distance(start + segment * factor), factor)
}

fn pick_is_closer(distance: f32, depth: f32, current_distance: f32, current_depth: f32) -> bool {
    distance
        .total_cmp(&current_distance)
        .then_with(|| depth.total_cmp(&current_depth))
        .is_lt()
}

fn pick_hit(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<(FeatureId, Option<FaceRef>)> {
    pick_surface_hit(scene, rect, pointer, camera).map(|(feature_id, _, face)| (feature_id, face))
}

fn pick_surface_hit(
    scene: &EvaluatedScene,
    rect: egui::Rect,
    pointer: egui::Pos2,
    camera: OrbitCamera,
) -> Option<(FeatureId, f32, Option<FaceRef>)> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || !rect.contains(pointer) {
        return None;
    }
    let view_projection =
        glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
    let (near, direction) = viewport_ray(rect, pointer, view_projection)?;

    let mut closest = None;
    for part in &scene.parts {
        for (triangle_index, triangle) in part.mesh.indices.chunks_exact(3).enumerate() {
            let [ia, ib, ic] = [triangle[0], triangle[1], triangle[2]];
            let (Some(a), Some(b), Some(c)) = (
                part.mesh.positions.get(ia as usize),
                part.mesh.positions.get(ib as usize),
                part.mesh.positions.get(ic as usize),
            ) else {
                continue;
            };
            let Some(distance) = ray_triangle_hit(
                near,
                direction,
                Vec3::from_array(*a),
                Vec3::from_array(*b),
                Vec3::from_array(*c),
            ) else {
                continue;
            };
            if closest
                .as_ref()
                .is_none_or(|(_, current, _)| distance < *current)
            {
                let face = u32::try_from(triangle_index).ok().and_then(|ordinal| {
                    part.faces
                        .iter()
                        .find(|face| face.triangles.contains(&ordinal))
                        .map(|face| face.reference.clone())
                });
                closest = Some((part.feature_id, distance, face));
            }
        }
    }
    closest
}

fn viewport_ray(
    rect: egui::Rect,
    pointer: egui::Pos2,
    view_projection: glam::Mat4,
) -> Option<(Vec3, Vec3)> {
    let ndc = Vec3::new(
        ((pointer.x - rect.left()) / rect.width()) * 2.0 - 1.0,
        1.0 - ((pointer.y - rect.top()) / rect.height()) * 2.0,
        0.0,
    );
    let inverse = view_projection.inverse();
    let near = inverse.project_point3(ndc);
    let far = inverse.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
    let direction = (far - near).normalize_or_zero();
    (direction.length_squared() > f32::EPSILON).then_some((near, direction))
}

fn ray_triangle_hit(
    origin: Vec3,
    direction: Vec3,
    point_a: Vec3,
    point_b: Vec3,
    point_c: Vec3,
) -> Option<f32> {
    let edge_one = point_b - point_a;
    let edge_two = point_c - point_a;
    let perpendicular = direction.cross(edge_two);
    let determinant = edge_one.dot(perpendicular);
    if determinant.abs() < 1.0e-6 {
        return None;
    }
    let inverse = determinant.recip();
    let offset = origin - point_a;
    let barycentric_u = offset.dot(perpendicular) * inverse;
    if !(0.0..=1.0).contains(&barycentric_u) {
        return None;
    }
    let cross = offset.cross(edge_one);
    let barycentric_v = direction.dot(cross) * inverse;
    if barycentric_v < 0.0 || barycentric_u + barycentric_v > 1.0 {
        return None;
    }
    let distance = edge_two.dot(cross) * inverse;
    (distance >= 0.0).then_some(distance)
}

pub fn register_renderer(render_state: &egui_wgpu::RenderState) {
    let renderer = ViewportRenderer::new(&render_state.device, render_state.target_format);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(renderer);
}

pub fn paint_viewport(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    scene: ViewportScene,
    camera: OrbitCamera,
) {
    let aspect = rect.width() / rect.height().max(1.0);
    let callback = ViewportCallback {
        scene,
        view_projection: camera.view_projection(aspect),
    };
    ui.painter()
        .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}

struct ViewportCallback {
    scene: ViewportScene,
    view_projection: [[f32; 4]; 4],
}

impl CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(renderer) = callback_resources.get_mut::<ViewportRenderer>() {
            renderer.prepare(device, queue, &self.scene, self.view_projection);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        if let Some(renderer) = callback_resources.get::<ViewportRenderer>() {
            renderer.paint(render_pass);
        }
    }
}

struct ViewportRenderer {
    solid_pipeline: wgpu::RenderPipeline,
    grid_pipeline: wgpu::RenderPipeline,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    solid_vertex_buffer: wgpu::Buffer,
    solid_index_buffer: wgpu::Buffer,
    grid_vertex_buffer: wgpu::Buffer,
    topology_vertex_buffer: wgpu::Buffer,
    solid_index_count: u32,
    grid_vertex_count: u32,
    topology_vertex_count: u32,
    uploaded_revision: Option<u64>,
}

impl ViewportRenderer {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadx viewport shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/cad.wgsl").into()),
        });
        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cadx camera uniform"),
            contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadx camera bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadx camera bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadx viewport pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let solid_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            target_format,
            wgpu::PrimitiveTopology::TriangleList,
            "fs_solid",
            true,
        );
        let grid_pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            target_format,
            wgpu::PrimitiveTopology::LineList,
            "fs_grid",
            false,
        );
        let solid_vertex_buffer =
            empty_buffer(device, "cadx solid vertices", wgpu::BufferUsages::VERTEX);
        let solid_index_buffer =
            empty_buffer(device, "cadx solid indices", wgpu::BufferUsages::INDEX);
        let grid_vertex_buffer =
            empty_buffer(device, "cadx grid vertices", wgpu::BufferUsages::VERTEX);
        let topology_vertex_buffer =
            empty_buffer(device, "cadx topology vertices", wgpu::BufferUsages::VERTEX);

        Self {
            solid_pipeline,
            grid_pipeline,
            camera_buffer,
            camera_bind_group,
            solid_vertex_buffer,
            solid_index_buffer,
            grid_vertex_buffer,
            topology_vertex_buffer,
            solid_index_count: 0,
            grid_vertex_count: 0,
            topology_vertex_count: 0,
            uploaded_revision: None,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &ViewportScene,
        view_projection: [[f32; 4]; 4],
    ) {
        queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::bytes_of(&CameraUniform { view_projection }),
        );
        let Ok(buffers) = scene.buffers.read() else {
            return;
        };
        if self.uploaded_revision == Some(buffers.revision) {
            return;
        }
        self.solid_vertex_buffer = buffer_with_data(
            device,
            "cadx solid vertices",
            bytemuck::cast_slice(&buffers.solid_vertices),
            wgpu::BufferUsages::VERTEX,
        );
        self.solid_index_buffer = buffer_with_data(
            device,
            "cadx solid indices",
            bytemuck::cast_slice(&buffers.solid_indices),
            wgpu::BufferUsages::INDEX,
        );
        self.grid_vertex_buffer = buffer_with_data(
            device,
            "cadx grid vertices",
            bytemuck::cast_slice(&buffers.grid_vertices),
            wgpu::BufferUsages::VERTEX,
        );
        self.topology_vertex_buffer = buffer_with_data(
            device,
            "cadx topology vertices",
            bytemuck::cast_slice(&buffers.topology_vertices),
            wgpu::BufferUsages::VERTEX,
        );
        self.solid_index_count = u32::try_from(buffers.solid_indices.len()).unwrap_or(u32::MAX);
        self.grid_vertex_count = u32::try_from(buffers.grid_vertices.len()).unwrap_or(u32::MAX);
        self.topology_vertex_count =
            u32::try_from(buffers.topology_vertices.len()).unwrap_or(u32::MAX);
        self.uploaded_revision = Some(buffers.revision);
    }

    fn paint(&self, render_pass: &mut wgpu::RenderPass<'static>) {
        render_pass.set_bind_group(0, &self.camera_bind_group, &[]);

        render_pass.set_pipeline(&self.grid_pipeline);
        render_pass.set_vertex_buffer(0, self.grid_vertex_buffer.slice(..));
        render_pass.draw(0..self.grid_vertex_count, 0..1);

        render_pass.set_pipeline(&self.solid_pipeline);
        render_pass.set_vertex_buffer(0, self.solid_vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.solid_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..self.solid_index_count, 0, 0..1);

        render_pass.set_pipeline(&self.grid_pipeline);
        render_pass.set_vertex_buffer(0, self.topology_vertex_buffer.slice(..));
        render_pass.draw(0..self.topology_vertex_count, 0..1);
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    topology: wgpu::PrimitiveTopology,
    fragment_entry: &str,
    depth_write: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cadx viewport pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[GpuVertex::layout()],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: if topology == wgpu::PrimitiveTopology::TriangleList {
                Some(wgpu::Face::Back)
            } else {
                None
            },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLE_COUNT,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

fn empty_buffer(device: &wgpu::Device, label: &str, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: 4,
        usage,
        mapped_at_creation: false,
    })
}

fn buffer_with_data(
    device: &wgpu::Device,
    label: &str,
    data: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    if data.is_empty() {
        empty_buffer(device, label, usage)
    } else {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: data,
            usage,
        })
    }
}

fn build_grid(extent: f32, spacing: f32) -> Vec<GpuVertex> {
    let mut vertices = Vec::new();
    let mut offset = -extent;
    while offset <= extent + spacing * 0.5 {
        let major = offset.abs() < spacing * 0.25;
        let color = if major {
            [0.32, 0.35, 0.38, 1.0]
        } else {
            [0.20, 0.22, 0.24, 1.0]
        };
        vertices.extend([
            GpuVertex {
                position: [-extent, offset, 0.0],
                normal: [0.0, 0.0, 1.0],
                color,
            },
            GpuVertex {
                position: [extent, offset, 0.0],
                normal: [0.0, 0.0, 1.0],
                color,
            },
            GpuVertex {
                position: [offset, -extent, 0.0],
                normal: [0.0, 0.0, 1.0],
                color,
            },
            GpuVertex {
                position: [offset, extent, 0.0],
                normal: [0.0, 0.0, 1.0],
                color,
            },
        ]);
        offset += spacing;
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_core::kernel::{
        EvaluatedDatumPlane, EvaluatedDatumPoint, EvaluatedPart, EvaluatedSketch, TriangleMesh,
    };
    use cadx_core::topology::{
        CurveKind, EdgeGeometry, EdgeRef, EvaluatedEdge, EvaluatedFace, EvaluatedVertex,
        FaceGeometry, FaceRef, PrimitiveFace, SurfaceKind, VertexGeometry, VertexRef,
    };

    #[test]
    fn picking_returns_the_closest_feature_under_the_cursor() {
        let reference = FaceRef::primitive(7, PrimitiveFace::BoxZMax);
        let edge_reference = EdgeRef::new(7, reference.clone(), reference.clone(), 0);
        let vertex_reference = VertexRef::new(7, vec![edge_reference.clone()], 0);
        let scene = EvaluatedScene {
            parts: vec![EvaluatedPart {
                feature_id: 7,
                name: "plate".into(),
                color: [1.0; 4],
                material: None,
                mesh: TriangleMesh {
                    positions: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [0.0, 10.0, 0.0]],
                    normals: vec![[0.0, 0.0, 1.0]; 3],
                    indices: vec![0, 1, 2],
                },
                faces: vec![EvaluatedFace {
                    reference: reference.clone(),
                    geometry: FaceGeometry {
                        surface: SurfaceKind::Plane,
                        plane: None,
                        area: 200.0,
                        centroid: [0.0, 0.0, 0.0],
                        mean_normal: [0.0, 0.0, 1.0],
                    },
                    triangles: 0..1,
                }],
                edges: vec![EvaluatedEdge {
                    reference: edge_reference.clone(),
                    geometry: EdgeGeometry {
                        curve: CurveKind::Line,
                        endpoints: [[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0]],
                        midpoint: [0.0, -10.0, 0.0],
                        length: 20.0,
                        length_error_estimate: Some(0.0),
                        polyline: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0]],
                    },
                }],
                vertices: vec![EvaluatedVertex {
                    reference: vertex_reference.clone(),
                    geometry: VertexGeometry {
                        position: [-10.0, -10.0, 0.0],
                    },
                }],
            }],
            ..EvaluatedScene::default()
        };
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        assert_eq!(pick_feature(&scene, rect, rect.center(), camera), Some(7));
        assert_eq!(
            pick_face(&scene, rect, rect.center(), camera),
            Some(reference.clone())
        );

        let view_projection =
            glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
        let edge_pointer = project_topology_point([0.0, -10.0, 0.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(
            pick_edge(&scene, rect, edge_pointer, camera),
            Some(edge_reference.clone())
        );
        let vertex_pointer = project_topology_point([-10.0, -10.0, 0.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(
            pick_vertex(&scene, rect, vertex_pointer, camera),
            Some(vertex_reference.clone())
        );

        let viewport = ViewportScene::default();
        let measurement_edges = std::slice::from_ref(&edge_reference);
        let measurement_vertices = std::slice::from_ref(&vertex_reference);
        let measurement_faces = std::slice::from_ref(&reference);
        let measurement_guides = [[[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0]]];
        viewport.update_with_topology(
            &scene,
            Some(7),
            TopologySelection {
                edges: std::slice::from_ref(&edge_reference),
                vertex: Some(&vertex_reference),
                face: None,
                measurement_faces,
                measurement_edges,
                measurement_vertices,
                measurement_guides: &measurement_guides,
            },
        );
        let buffers = viewport.buffers.read().unwrap();
        assert_eq!(buffers.topology_vertices.len(), 18);
        assert!(buffers.solid_vertices.iter().all(|vertex| {
            vertex
                .color
                .iter()
                .zip([0.18, 0.82, 0.92, 1.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        }));
    }

    #[test]
    fn frame_empty_scene_restores_a_stable_camera() {
        let mut camera = OrbitCamera {
            distance: 500.0,
            ..Default::default()
        };
        camera.frame_scene(&EvaluatedScene::default());
        assert!((camera.distance - OrbitCamera::default().distance).abs() < f32::EPSILON);
    }

    #[test]
    fn sketch_overlay_is_framed_highlighted_and_feature_pickable() {
        let profile = vec![
            [-8.0, -4.0, 3.0],
            [8.0, -4.0, 3.0],
            [8.0, 4.0, 3.0],
            [-8.0, 4.0, 3.0],
        ];
        let scene = EvaluatedScene {
            sketches: vec![EvaluatedSketch {
                feature_id: 12,
                name: "mounting profile".into(),
                color: [0.2, 0.7, 0.5, 1.0],
                constraint_annotations: Vec::new(),
                profile: profile.clone(),
                holes: vec![vec![
                    [-2.0, -1.0, 3.0],
                    [2.0, -1.0, 3.0],
                    [2.0, 1.0, 3.0],
                    [-2.0, 1.0, 3.0],
                ]],
                construction: vec![vec![[-12.0, 0.0, 3.0], [12.0, 0.0, 3.0]]],
                origin: [0.0, 0.0, 3.0],
                x_direction: [1.0, 0.0, 0.0],
                y_direction: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            }],
            ..EvaluatedScene::default()
        };
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        assert!((camera.target.z - 3.0).abs() < f32::EPSILON);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 240.0));
        let view_projection =
            glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
        let pointer = project_topology_point([0.0, -4.0, 3.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(pick_feature(&scene, rect, pointer, camera), Some(12));
        let hole_pointer = project_topology_point([0.0, -1.0, 3.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(pick_feature(&scene, rect, hole_pointer, camera), Some(12));
        let construction_pointer = project_topology_point([11.0, 0.0, 3.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(
            pick_feature(&scene, rect, construction_pointer, camera),
            Some(12)
        );

        let viewport = ViewportScene::default();
        viewport.update(&scene, Some(12));
        let buffers = viewport.buffers.read().unwrap();
        assert_eq!(buffers.topology_vertices.len(), 50);
        assert!(buffers.topology_vertices.iter().all(|vertex| {
            vertex
                .color
                .iter()
                .zip([1.0, 0.72, 0.12, 1.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        }));
    }

    #[test]
    fn sampled_arc_extremum_is_framed_and_its_midpoint_is_pickable() {
        let scene = EvaluatedScene {
            sketches: vec![EvaluatedSketch {
                feature_id: 18,
                name: "sampled semicircle".into(),
                color: [0.2, 0.7, 0.5, 1.0],
                constraint_annotations: Vec::new(),
                profile: vec![
                    [-10.0, 0.0, 2.0],
                    [-7.071, 7.071, 2.0],
                    [0.0, 10.0, 2.0],
                    [7.071, 7.071, 2.0],
                    [10.0, 0.0, 2.0],
                ],
                holes: Vec::new(),
                construction: Vec::new(),
                origin: [0.0, 0.0, 2.0],
                x_direction: [1.0, 0.0, 0.0],
                y_direction: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            }],
            ..EvaluatedScene::default()
        };
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        assert!((camera.target.y - 5.0).abs() < 1.0e-5);
        assert!((camera.target.z - 2.0).abs() < 1.0e-5);

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 240.0));
        let view_projection =
            glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
        let arc_midpoint = project_topology_point([0.0, 10.0, 2.0], rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(pick_feature(&scene, rect, arc_midpoint, camera), Some(18));
    }

    #[test]
    fn reference_geometry_is_framed_drawn_and_feature_pickable() {
        let face = FaceRef::primitive(7, PrimitiveFace::BoxZMax);
        let edge = EdgeRef::new(7, face.clone(), face.clone(), 0);
        let vertex = VertexRef::new(7, vec![edge], 0);
        let point_position = [10.0, 2.0, 3.0];
        let scene = EvaluatedScene {
            datum_planes: vec![EvaluatedDatumPlane {
                feature_id: 8,
                name: "datum plane".into(),
                color: [0.2, 0.7, 0.9, 1.0],
                face,
                origin: [0.0; 3],
                x_direction: [1.0, 0.0, 0.0],
                y_direction: [0.0, 1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            }],
            datum_points: vec![EvaluatedDatumPoint {
                feature_id: 9,
                name: "datum point".into(),
                color: [0.9, 0.3, 0.2, 1.0],
                vertex,
                position: point_position,
            }],
            ..EvaluatedScene::default()
        };
        let mut camera = OrbitCamera::default();
        camera.frame_scene(&scene);
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 240.0));
        let view_projection =
            glam::Mat4::from_cols_array_2d(&camera.view_projection(rect.width() / rect.height()));
        let pointer = project_topology_point(point_position, rect, view_projection)
            .unwrap()
            .0;
        assert_eq!(pick_feature(&scene, rect, pointer, camera), Some(9));

        let viewport = ViewportScene::default();
        viewport.update(&scene, Some(9));
        let buffers = viewport.buffers.read().unwrap();
        assert_eq!(buffers.topology_vertices.len(), 16);
        assert!(buffers.topology_vertices[10..].iter().all(|vertex| {
            vertex
                .color
                .iter()
                .zip([1.0, 0.72, 0.12, 1.0])
                .all(|(actual, expected)| (actual - expected).abs() < f32::EPSILON)
        }));
    }
}
