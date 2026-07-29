//! Immutable 2D scene extraction, camera transforms, picking, and snapping.
//!
//! This crate deliberately has no renderer backend or mutable document access.
//! Desktop and future GPU renderers consume [`RenderScene`] snapshots produced
//! from validated core data, while input tools use the same transform and
//! picking contracts to turn screen gestures into typed CAD commands elsewhere.

mod bounds;
mod camera;
mod geometry;
mod mechanical;
mod scene;

#[cfg(test)]
mod tests;

pub use bounds::Bounds2;
pub use camera::{
    MAX_PIXELS_PER_UNIT, MIN_PIXELS_PER_UNIT, ScreenPoint, ViewTransform, ViewportSize,
};
pub use geometry::{
    AlignedDimensionGeometry, PickHit, SnapHit, SnapKind, SnapSettings, aligned_dimension_geometry,
    aligned_dimension_offset, format_dimension_text,
};
pub use mechanical::{
    Bounds3, CameraProjection3, MAX_MECHANICAL_TRIANGLES, MAX_MECHANICAL_VERTICES,
    MAX_PROFILE_VERTICES, MechanicalItem, MechanicalPickHit, MechanicalScene, MechanicalSceneError,
    OrbitCamera, Point3, ProjectedTriangle, SolidMesh,
};
pub use scene::{RenderItem, RenderPrimitive, RenderScene};
