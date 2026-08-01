use std::path::Path;

use cadx_core::kernel::{EvaluatedPart, EvaluatedScene};

use crate::{ExportError, atomic::write_atomic};

const STL_HEADER_SIZE: usize = 80;
const STL_TRIANGLE_SIZE: usize = 50;

/// Encodes a scene as binary STL after validating every indexed triangle.
///
/// # Errors
///
/// Returns [`ExportError`] when the scene is empty, a mesh is malformed, or
/// the triangle count cannot be represented by binary STL.
pub fn encode_binary_stl(scene: &EvaluatedScene) -> Result<Vec<u8>, ExportError> {
    let triangle_count = scene.triangle_count();
    if triangle_count == 0 {
        return Err(ExportError::EmptyScene);
    }
    let triangle_count =
        u32::try_from(triangle_count).map_err(|_| ExportError::TooManyTriangles)?;
    let capacity = STL_HEADER_SIZE
        .checked_add(4)
        .and_then(|size| {
            size.checked_add(
                usize::try_from(triangle_count)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(STL_TRIANGLE_SIZE),
            )
        })
        .ok_or(ExportError::TooManyTriangles)?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut header = [0_u8; STL_HEADER_SIZE];
    let label = b"CADX binary STL";
    header[..label.len()].copy_from_slice(label);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&triangle_count.to_le_bytes());

    for part in &scene.parts {
        validate_mesh(part)?;
        for (triangle, indices) in part.mesh.indices.chunks_exact(3).enumerate() {
            let points = [
                part.mesh.positions[indices[0] as usize],
                part.mesh.positions[indices[1] as usize],
                part.mesh.positions[indices[2] as usize],
            ];
            let normal = face_normal(points).ok_or_else(|| ExportError::InvalidTriangle {
                feature_id: part.feature_id,
                triangle,
                message: "triangle is degenerate".into(),
            })?;
            for value in normal.into_iter().chain(points.into_iter().flatten()) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&0_u16.to_le_bytes());
        }
    }
    Ok(bytes)
}

/// Atomically writes a validated binary STL file.
///
/// # Errors
///
/// Returns [`ExportError`] when validation, encoding, or file output fails.
pub fn write_binary_stl(scene: &EvaluatedScene, path: impl AsRef<Path>) -> Result<(), ExportError> {
    write_atomic(path.as_ref(), &encode_binary_stl(scene)?).map_err(ExportError::from)
}

pub(crate) fn validate_mesh(part: &EvaluatedPart) -> Result<(), ExportError> {
    let mesh = &part.mesh;
    if mesh.indices.is_empty() || !mesh.indices.len().is_multiple_of(3) {
        return Err(ExportError::InvalidMesh {
            feature_id: part.feature_id,
            message: "index count must be a non-zero multiple of three".into(),
        });
    }
    if mesh
        .positions
        .iter()
        .flatten()
        .any(|value| !value.is_finite())
    {
        return Err(ExportError::InvalidMesh {
            feature_id: part.feature_id,
            message: "vertex coordinates must be finite".into(),
        });
    }
    for (triangle, indices) in mesh.indices.chunks_exact(3).enumerate() {
        if indices
            .iter()
            .any(|index| *index as usize >= mesh.positions.len())
        {
            return Err(ExportError::InvalidTriangle {
                feature_id: part.feature_id,
                triangle,
                message: "vertex index is out of bounds".into(),
            });
        }
        let points = [
            mesh.positions[indices[0] as usize],
            mesh.positions[indices[1] as usize],
            mesh.positions[indices[2] as usize],
        ];
        if face_normal(points).is_none() {
            return Err(ExportError::InvalidTriangle {
                feature_id: part.feature_id,
                triangle,
                message: "triangle is degenerate".into(),
            });
        }
    }
    Ok(())
}

fn face_normal([a, b, c]: [[f32; 3]; 3]) -> Option<[f32; 3]> {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let length = cross.iter().map(|value| value * value).sum::<f32>().sqrt();
    (length.is_finite() && length > f32::EPSILON)
        .then(|| [cross[0] / length, cross[1] / length, cross[2] / length])
}

#[cfg(test)]
mod tests {
    use cadx_core::kernel::{EvaluatedPart, TriangleMesh};

    use super::*;

    fn triangle_scene() -> EvaluatedScene {
        EvaluatedScene {
            parts: vec![EvaluatedPart {
                feature_id: 7,
                name: "triangle".into(),
                color: [0.8, 0.2, 0.1, 1.0],
                material: None,
                mesh: TriangleMesh {
                    positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    normals: Vec::new(),
                    indices: vec![0, 1, 2],
                },
                faces: Vec::new(),
                edges: Vec::new(),
                vertices: Vec::new(),
            }],
            ..EvaluatedScene::default()
        }
    }

    #[test]
    fn encodes_binary_stl_layout() {
        let bytes = encode_binary_stl(&triangle_scene()).unwrap();
        assert_eq!(bytes.len(), 84 + STL_TRIANGLE_SIZE);
        assert_eq!(u32::from_le_bytes(bytes[80..84].try_into().unwrap()), 1);
    }

    #[test]
    fn rejects_out_of_bounds_indices() {
        let mut scene = triangle_scene();
        scene.parts[0].mesh.indices[2] = 3;
        assert!(matches!(
            encode_binary_stl(&scene),
            Err(ExportError::InvalidTriangle { .. })
        ));
    }
}
