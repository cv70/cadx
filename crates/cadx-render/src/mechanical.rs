use std::fmt;

use cadx_core::{CadDocument, EntityId, EntityKind, LayerId, Point2};

use crate::{ScreenPoint, ViewportSize};

pub const MAX_PROFILE_VERTICES: usize = 250_000;
pub const MAX_MECHANICAL_VERTICES: usize = 1_000_000;
pub const MAX_MECHANICAL_TRIANGLES: usize = 2_000_000;

const PROFILE_EPSILON: f64 = 1.0e-12;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Point3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn subtract(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    fn scale(self, factor: f64) -> Self {
        Self::new(self.x * factor, self.y * factor, self.z * factor)
    }

    fn dot(self, other: Self) -> f64 {
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }

    fn cross(self, other: Self) -> Self {
        Self::new(
            self.y.mul_add(other.z, -self.z * other.y),
            self.z.mul_add(other.x, -self.x * other.z),
            self.x.mul_add(other.y, -self.y * other.x),
        )
    }

    fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length.is_finite() && length > f64::EPSILON).then(|| self.scale(1.0 / length))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds3 {
    pub min: Point3,
    pub max: Point3,
}

impl Bounds3 {
    pub const fn from_point(point: Point3) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    pub fn include_point(&mut self, point: Point3) {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.min.z = self.min.z.min(point.z);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
        self.max.z = self.max.z.max(point.z);
    }

    pub fn include_bounds(&mut self, other: Self) {
        self.include_point(other.min);
        self.include_point(other.max);
    }

    pub fn center(self) -> Point3 {
        Point3::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    pub fn diagonal(self) -> f64 {
        self.max.subtract(self.min).length()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SolidMesh {
    pub vertices: Vec<Point3>,
    pub triangles: Vec<[u32; 3]>,
    pub feature_edges: Vec<[u32; 2]>,
    pub bounds: Bounds3,
}

impl SolidMesh {
    pub fn extrude(profile: &[Point2], distance: f64) -> Result<Self, MechanicalSceneError> {
        if !distance.is_finite() || distance <= 0.0 {
            return Err(MechanicalSceneError::InvalidProfile(
                "extrude distance must be finite and positive".into(),
            ));
        }
        if profile.len() > MAX_PROFILE_VERTICES {
            return Err(MechanicalSceneError::LimitExceeded {
                resource: "profile vertices",
                limit: MAX_PROFILE_VERTICES,
            });
        }
        let mut points = sanitize_profile(profile)?;
        if signed_twice_area(&points) < 0.0 {
            points.reverse();
        }
        let flat = points
            .iter()
            .flat_map(|point| [point.x, point.y])
            .collect::<Vec<_>>();
        let cap_indices = earcutr::earcut(&flat, &[], 2).map_err(|error| {
            MechanicalSceneError::Triangulation(format!("profile triangulation failed: {error}"))
        })?;
        let expected_cap_indices = points.len().saturating_sub(2) * 3;
        if cap_indices.len() != expected_cap_indices
            || cap_indices.iter().any(|index| *index >= points.len())
        {
            return Err(MechanicalSceneError::Triangulation(
                "profile did not produce a complete simple-polygon triangulation".into(),
            ));
        }
        let deviation = earcutr::deviation(&flat, &[], 2, &cap_indices);
        if !deviation.is_finite() || deviation > 1.0e-9 {
            return Err(MechanicalSceneError::Triangulation(format!(
                "profile triangulation area deviation is {deviation}"
            )));
        }

        let count = points.len();
        let mut vertices = Vec::with_capacity(count * 2);
        vertices.extend(
            points
                .iter()
                .map(|point| Point3::new(point.x, point.y, 0.0)),
        );
        vertices.extend(
            points
                .iter()
                .map(|point| Point3::new(point.x, point.y, distance)),
        );
        let mut triangles = Vec::with_capacity(cap_indices.len() / 3 * 2 + count * 2);
        for triangle in cap_indices.chunks_exact(3) {
            let mut a = triangle[0];
            let mut b = triangle[1];
            let c = triangle[2];
            if triangle_cross(points[a], points[b], points[c]) < 0.0 {
                std::mem::swap(&mut a, &mut b);
            }
            triangles.push([
                to_index(a + count)?,
                to_index(b + count)?,
                to_index(c + count)?,
            ]);
            triangles.push([to_index(c)?, to_index(b)?, to_index(a)?]);
        }
        for index in 0..count {
            let next = (index + 1) % count;
            let bottom = to_index(index)?;
            let bottom_next = to_index(next)?;
            let top = to_index(index + count)?;
            let top_next = to_index(next + count)?;
            triangles.push([bottom, bottom_next, top_next]);
            triangles.push([bottom, top_next, top]);
        }
        let mut feature_edges = Vec::with_capacity(count * 3);
        for index in 0..count {
            let next = (index + 1) % count;
            feature_edges.push(canonical_edge(to_index(index)?, to_index(next)?));
            feature_edges.push(canonical_edge(
                to_index(index + count)?,
                to_index(next + count)?,
            ));
            feature_edges.push(canonical_edge(to_index(index)?, to_index(index + count)?));
        }
        feature_edges.sort_unstable();
        feature_edges.dedup();
        if triangles.len() > MAX_MECHANICAL_TRIANGLES {
            return Err(MechanicalSceneError::LimitExceeded {
                resource: "solid triangles",
                limit: MAX_MECHANICAL_TRIANGLES,
            });
        }
        let mut bounds = Bounds3::from_point(vertices[0]);
        for vertex in vertices.iter().copied().skip(1) {
            bounds.include_point(vertex);
        }
        Ok(Self {
            vertices,
            triangles,
            feature_edges,
            bounds,
        })
    }
}

fn sanitize_profile(profile: &[Point2]) -> Result<Vec<Point2>, MechanicalSceneError> {
    let finite = profile
        .iter()
        .all(|point| point.x.is_finite() && point.y.is_finite());
    if !finite {
        return Err(MechanicalSceneError::InvalidProfile(
            "profile coordinates must be finite".into(),
        ));
    }
    let scale = profile.iter().fold(1.0_f64, |scale, point| {
        scale.max(point.x.abs()).max(point.y.abs())
    });
    let epsilon = scale * PROFILE_EPSILON;
    let mut points = Vec::with_capacity(profile.len());
    for point in profile.iter().copied() {
        if points
            .last()
            .is_none_or(|previous| point_distance(*previous, point) > epsilon)
        {
            points.push(point);
        }
    }
    if points.len() > 1 && point_distance(points[0], points[points.len() - 1]) <= epsilon {
        points.pop();
    }
    if points.len() < 3 {
        return Err(MechanicalSceneError::InvalidProfile(
            "a solid profile needs at least three distinct vertices".into(),
        ));
    }
    let area = signed_twice_area(&points);
    if !area.is_finite() || area.abs() <= epsilon * epsilon {
        return Err(MechanicalSceneError::InvalidProfile(
            "a solid profile must enclose nonzero planar area".into(),
        ));
    }
    Ok(points)
}

fn point_distance(left: Point2, right: Point2) -> f64 {
    (left.x - right.x).hypot(left.y - right.y)
}

fn signed_twice_area(points: &[Point2]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(left, right)| left.x.mul_add(right.y, -left.y * right.x))
        .sum()
}

fn triangle_cross(a: Point2, b: Point2, c: Point2) -> f64 {
    (b.x - a.x).mul_add(c.y - a.y, -(b.y - a.y) * (c.x - a.x))
}

fn to_index(index: usize) -> Result<u32, MechanicalSceneError> {
    u32::try_from(index).map_err(|_| MechanicalSceneError::LimitExceeded {
        resource: "mesh index",
        limit: u32::MAX as usize,
    })
}

fn canonical_edge(left: u32, right: u32) -> [u32; 2] {
    [left.min(right), left.max(right)]
}

#[derive(Clone, Debug, PartialEq)]
pub struct MechanicalItem {
    pub entity_id: EntityId,
    pub layer_id: LayerId,
    pub color: [u8; 4],
    pub locked: bool,
    pub mesh: SolidMesh,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MechanicalScene {
    pub items: Vec<MechanicalItem>,
    pub bounds: Option<Bounds3>,
}

impl MechanicalScene {
    pub fn from_document(document: &CadDocument) -> Result<Self, MechanicalSceneError> {
        let mut scene = Self::default();
        let mut total_vertices = 0_usize;
        let mut total_triangles = 0_usize;
        for entity in document.entities.values() {
            let EntityKind::Extrude { profile, distance } = entity.kind else {
                continue;
            };
            let Some(layer) = document.layers.get(&entity.layer) else {
                continue;
            };
            if !entity.visible || !layer.visible {
                continue;
            }
            let profile_entity =
                document
                    .entities
                    .get(&profile)
                    .ok_or(MechanicalSceneError::ProfileMissing {
                        extrude: entity.id,
                        profile,
                    })?;
            let EntityKind::SketchProfile {
                points,
                closed: true,
            } = &profile_entity.kind
            else {
                return Err(MechanicalSceneError::ProfileInvalid {
                    extrude: entity.id,
                    profile,
                });
            };
            let mesh = SolidMesh::extrude(points, distance).map_err(|error| {
                MechanicalSceneError::EntityMesh {
                    entity: entity.id,
                    detail: error.to_string(),
                }
            })?;
            total_vertices = total_vertices.checked_add(mesh.vertices.len()).ok_or(
                MechanicalSceneError::LimitExceeded {
                    resource: "scene vertices",
                    limit: MAX_MECHANICAL_VERTICES,
                },
            )?;
            total_triangles = total_triangles.checked_add(mesh.triangles.len()).ok_or(
                MechanicalSceneError::LimitExceeded {
                    resource: "scene triangles",
                    limit: MAX_MECHANICAL_TRIANGLES,
                },
            )?;
            if total_vertices > MAX_MECHANICAL_VERTICES {
                return Err(MechanicalSceneError::LimitExceeded {
                    resource: "scene vertices",
                    limit: MAX_MECHANICAL_VERTICES,
                });
            }
            if total_triangles > MAX_MECHANICAL_TRIANGLES {
                return Err(MechanicalSceneError::LimitExceeded {
                    resource: "scene triangles",
                    limit: MAX_MECHANICAL_TRIANGLES,
                });
            }
            match &mut scene.bounds {
                Some(bounds) => bounds.include_bounds(mesh.bounds),
                None => scene.bounds = Some(mesh.bounds),
            }
            scene.items.push(MechanicalItem {
                entity_id: entity.id,
                layer_id: layer.id,
                color: layer.color,
                locked: layer.locked,
                mesh,
            });
        }
        Ok(scene)
    }

    pub fn projected_triangles(
        &self,
        camera: OrbitCamera,
        viewport: ViewportSize,
    ) -> Vec<ProjectedTriangle> {
        if !viewport.width.is_finite()
            || !viewport.height.is_finite()
            || viewport.width <= 0.0
            || viewport.height <= 0.0
        {
            return Vec::new();
        }
        let Some(basis) = camera.basis() else {
            return Vec::new();
        };
        let mut projected = Vec::new();
        for item in &self.items {
            for triangle in &item.mesh.triangles {
                let vertices = triangle.map(|index| item.mesh.vertices[index as usize]);
                let camera_vertices = vertices.map(|vertex| basis.camera_point(vertex));
                if camera_vertices.iter().any(|vertex| vertex.z <= basis.near) {
                    continue;
                }
                let points = camera_vertices.map(|vertex| basis.project(vertex, viewport));
                let Some(normal) = vertices[1]
                    .subtract(vertices[0])
                    .cross(vertices[2].subtract(vertices[0]))
                    .normalized()
                else {
                    continue;
                };
                if normal.dot(basis.position.subtract(vertices[0])) <= 0.0 {
                    continue;
                }
                let light = Point3::new(0.35, -0.55, 1.0)
                    .normalized()
                    .expect("fixed light direction is nonzero");
                let intensity = (0.25 + 0.75 * normal.dot(light).max(0.0)).clamp(0.0, 1.0) as f32;
                projected.push(ProjectedTriangle {
                    entity_id: item.entity_id,
                    color: item.color,
                    locked: item.locked,
                    points,
                    edges: [
                        item.mesh
                            .feature_edges
                            .binary_search(&canonical_edge(triangle[0], triangle[1]))
                            .is_ok(),
                        item.mesh
                            .feature_edges
                            .binary_search(&canonical_edge(triangle[1], triangle[2]))
                            .is_ok(),
                        item.mesh
                            .feature_edges
                            .binary_search(&canonical_edge(triangle[2], triangle[0]))
                            .is_ok(),
                    ],
                    depth: (camera_vertices[0].z + camera_vertices[1].z + camera_vertices[2].z)
                        / 3.0,
                    intensity,
                });
            }
        }
        projected.sort_by(|left, right| {
            right
                .depth
                .total_cmp(&left.depth)
                .then_with(|| left.entity_id.cmp(&right.entity_id))
        });
        projected
    }

    pub fn pick(
        &self,
        camera: OrbitCamera,
        viewport: ViewportSize,
        point: ScreenPoint,
    ) -> Option<MechanicalPickHit> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return None;
        }
        self.projected_triangles(camera, viewport)
            .into_iter()
            .filter(|triangle| !triangle.locked && point_in_triangle(point, triangle.points))
            .map(|triangle| MechanicalPickHit {
                entity_id: triangle.entity_id,
                depth: triangle.depth,
            })
            .min_by(|left, right| {
                left.depth
                    .total_cmp(&right.depth)
                    .then_with(|| left.entity_id.cmp(&right.entity_id))
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Point3,
    pub yaw: f64,
    pub pitch: f64,
    pub distance: f64,
    pub vertical_fov_radians: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraProjection3 {
    pub view_projection: [[f32; 4]; 4],
    pub camera_position: Point3,
    pub near: f64,
    pub far: f64,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Point3::default(),
            yaw: -0.75,
            pitch: 0.55,
            distance: 100.0,
            vertical_fov_radians: 45.0_f64.to_radians(),
        }
    }
}

impl OrbitCamera {
    pub fn fit_bounds(&mut self, bounds: Bounds3, viewport: ViewportSize, padding: f64) {
        let radius = (bounds.diagonal() * 0.5).max(1.0e-3);
        let aspect = (viewport.width / viewport.height.max(1.0)).max(1.0e-3);
        let vertical_half = self.vertical_fov_radians * 0.5;
        let horizontal_half = (vertical_half.tan() * aspect).atan();
        let limiting_half = vertical_half.min(horizontal_half).max(0.05);
        self.target = bounds.center();
        self.distance = (radius / limiting_half.sin()) * (1.0 + padding.max(0.0));
    }

    pub fn orbit_pixels(&mut self, delta_x: f64, delta_y: f64) {
        if delta_x.is_finite() && delta_y.is_finite() {
            self.yaw = (self.yaw - delta_x * 0.008).rem_euclid(std::f64::consts::TAU);
            self.pitch = (self.pitch + delta_y * 0.008).clamp(-1.45, 1.45);
        }
    }

    pub fn zoom(&mut self, factor: f64) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance / factor).clamp(1.0e-6, 1.0e18);
        }
    }

    pub fn project_point(self, point: Point3, viewport: ViewportSize) -> Option<ScreenPoint> {
        let basis = self.basis()?;
        let point = basis.camera_point(point);
        (point.z > basis.near).then(|| basis.project(point, viewport))
    }

    pub fn projection(self, bounds: Bounds3, viewport: ViewportSize) -> Option<CameraProjection3> {
        if !viewport.width.is_finite()
            || !viewport.height.is_finite()
            || viewport.width <= 0.0
            || viewport.height <= 0.0
        {
            return None;
        }
        let basis = self.basis()?;
        let mut far = basis.near * 2.0;
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    far = far.max(basis.camera_point(Point3::new(x, y, z)).z);
                }
            }
        }
        far = (far + bounds.diagonal().max(1.0) * 0.1).max(basis.near * 2.0);
        if !far.is_finite() {
            return None;
        }

        let aspect = viewport.width / viewport.height;
        let focal = 1.0 / (self.vertical_fov_radians * 0.5).tan();
        let depth_scale = far / (far - basis.near);
        let depth_offset = -basis.near * far / (far - basis.near);
        let rows = [
            [
                basis.right.x * focal / aspect,
                basis.right.y * focal / aspect,
                basis.right.z * focal / aspect,
                -basis.position.dot(basis.right) * focal / aspect,
            ],
            [
                basis.up.x * focal,
                basis.up.y * focal,
                basis.up.z * focal,
                -basis.position.dot(basis.up) * focal,
            ],
            [
                basis.forward.x * depth_scale,
                basis.forward.y * depth_scale,
                basis.forward.z * depth_scale,
                -basis.position.dot(basis.forward) * depth_scale + depth_offset,
            ],
            [
                basis.forward.x,
                basis.forward.y,
                basis.forward.z,
                -basis.position.dot(basis.forward),
            ],
        ];
        let mut view_projection = [[0.0_f32; 4]; 4];
        for (row, values) in rows.into_iter().enumerate() {
            for (column, value) in values.into_iter().enumerate() {
                let value = value as f32;
                if !value.is_finite() {
                    return None;
                }
                view_projection[column][row] = value;
            }
        }
        Some(CameraProjection3 {
            view_projection,
            camera_position: basis.position,
            near: basis.near,
            far,
        })
    }

    fn basis(self) -> Option<CameraBasis> {
        if !self.target.x.is_finite()
            || !self.target.y.is_finite()
            || !self.target.z.is_finite()
            || !self.yaw.is_finite()
            || !self.pitch.is_finite()
            || !self.distance.is_finite()
            || self.distance <= 0.0
            || !self.vertical_fov_radians.is_finite()
            || !(0.0..std::f64::consts::PI).contains(&self.vertical_fov_radians)
        {
            return None;
        }
        let horizontal = self.pitch.cos() * self.distance;
        let position = self.target.add(Point3::new(
            self.yaw.cos() * horizontal,
            self.yaw.sin() * horizontal,
            self.pitch.sin() * self.distance,
        ));
        let forward = self.target.subtract(position).normalized()?;
        let world_up = Point3::new(0.0, 0.0, 1.0);
        let right = forward.cross(world_up).normalized()?;
        let up = right.cross(forward);
        Some(CameraBasis {
            position,
            right,
            up,
            forward,
            vertical_fov_radians: self.vertical_fov_radians,
            near: (self.distance * 1.0e-5).max(1.0e-9),
        })
    }
}

#[derive(Clone, Copy)]
struct CameraBasis {
    position: Point3,
    right: Point3,
    up: Point3,
    forward: Point3,
    vertical_fov_radians: f64,
    near: f64,
}

impl CameraBasis {
    fn camera_point(self, point: Point3) -> Point3 {
        let relative = point.subtract(self.position);
        Point3::new(
            relative.dot(self.right),
            relative.dot(self.up),
            relative.dot(self.forward),
        )
    }

    fn project(self, point: Point3, viewport: ViewportSize) -> ScreenPoint {
        let focal = viewport.height / (2.0 * (self.vertical_fov_radians * 0.5).tan());
        ScreenPoint::new(
            viewport.width * 0.5 + point.x * focal / point.z,
            viewport.height * 0.5 - point.y * focal / point.z,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedTriangle {
    pub entity_id: EntityId,
    pub color: [u8; 4],
    pub locked: bool,
    pub points: [ScreenPoint; 3],
    pub edges: [bool; 3],
    pub depth: f64,
    pub intensity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MechanicalPickHit {
    pub entity_id: EntityId,
    pub depth: f64,
}

fn point_in_triangle(point: ScreenPoint, triangle: [ScreenPoint; 3]) -> bool {
    let first = screen_cross(triangle[0], triangle[1], point);
    let second = screen_cross(triangle[1], triangle[2], point);
    let third = screen_cross(triangle[2], triangle[0], point);
    let has_negative = first < -1.0e-9 || second < -1.0e-9 || third < -1.0e-9;
    let has_positive = first > 1.0e-9 || second > 1.0e-9 || third > 1.0e-9;
    !(has_negative && has_positive)
}

fn screen_cross(start: ScreenPoint, end: ScreenPoint, point: ScreenPoint) -> f64 {
    (end.x - start.x).mul_add(point.y - start.y, -(end.y - start.y) * (point.x - start.x))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MechanicalSceneError {
    ProfileMissing {
        extrude: EntityId,
        profile: EntityId,
    },
    ProfileInvalid {
        extrude: EntityId,
        profile: EntityId,
    },
    InvalidProfile(String),
    Triangulation(String),
    EntityMesh {
        entity: EntityId,
        detail: String,
    },
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
}

impl fmt::Display for MechanicalSceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileMissing { extrude, profile } => {
                write!(
                    formatter,
                    "extrude {extrude} references missing profile {profile}"
                )
            }
            Self::ProfileInvalid { extrude, profile } => write!(
                formatter,
                "extrude {extrude} requires closed sketch profile {profile}"
            ),
            Self::InvalidProfile(message) | Self::Triangulation(message) => {
                formatter.write_str(message)
            }
            Self::EntityMesh { entity, detail } => {
                write!(
                    formatter,
                    "cannot build solid mesh for entity {entity}: {detail}"
                )
            }
            Self::LimitExceeded { resource, limit } => {
                write!(
                    formatter,
                    "mechanical {resource} exceeds the limit of {limit}"
                )
            }
        }
    }
}

impl std::error::Error for MechanicalSceneError {}
