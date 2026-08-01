use std::{io::Cursor, path::Path};

use cadx_core::kernel::EvaluatedScene;
use lib3mf::{BaseMaterial, BaseMaterialGroup, BuildItem, Mesh, Model, Object, Triangle, Vertex};

use crate::{ExportError, atomic::write_atomic, stl::validate_mesh};

/// Encodes the visible evaluated bodies as a validated 3MF package.
///
/// Each body remains a separate named object and retains its display color.
///
/// # Errors
///
/// Returns [`ExportError`] when the scene or a mesh is invalid, or when the
/// structured 3MF writer rejects the model.
pub fn encode_3mf(scene: &EvaluatedScene) -> Result<Vec<u8>, ExportError> {
    if scene.parts.is_empty() || scene.triangle_count() == 0 {
        return Err(ExportError::EmptyScene);
    }

    let mut model = Model::new();
    let object_id_base = scene.parts.len() + 1;
    for (index, part) in scene.parts.iter().enumerate() {
        validate_mesh(part)?;
        let color = rgba(part.feature_id, part.color)?;
        let material_id = index + 1;
        let object_id = object_id_base + index;

        let mut materials = BaseMaterialGroup::new(material_id);
        materials
            .materials
            .push(BaseMaterial::new(part.name.clone(), color));
        model.resources.base_material_groups.push(materials);

        let mut mesh = Mesh::with_capacity(part.mesh.positions.len(), part.mesh.triangle_count());
        mesh.vertices
            .extend(part.mesh.positions.iter().map(|point| {
                Vertex::new(
                    f64::from(point[0]),
                    f64::from(point[1]),
                    f64::from(point[2]),
                )
            }));
        mesh.triangles
            .extend(part.mesh.indices.chunks_exact(3).map(|indices| {
                Triangle::new(
                    indices[0] as usize,
                    indices[1] as usize,
                    indices[2] as usize,
                )
            }));

        let mut object = Object::new(object_id);
        object.name = Some(part.name.clone());
        object.mesh = Some(mesh);
        object.basematerialid = Some(material_id);
        object.pindex = Some(0);
        model.resources.objects.push(object);
        model.build.items.push(BuildItem::new(object_id));
    }
    lib3mf::validator::validate_model(&model)
        .map_err(|error| ExportError::InvalidThreeMf(error.to_string()))?;
    let writer = model
        .to_writer(Cursor::new(Vec::new()))
        .map_err(|error| ExportError::InvalidThreeMf(error.to_string()))?;
    let bytes = writer.into_inner();
    validate_3mf(&bytes)?;
    Ok(bytes)
}

/// Parses and validates a complete 3MF OPC package.
///
/// # Errors
///
/// Returns [`ExportError::InvalidThreeMf`] when the package is malformed or
/// contains no build items.
pub fn validate_3mf(bytes: &[u8]) -> Result<(), ExportError> {
    let model = Model::from_reader(Cursor::new(bytes))
        .map_err(|error| ExportError::InvalidThreeMf(error.to_string()))?;
    lib3mf::validator::validate_model(&model)
        .map_err(|error| ExportError::InvalidThreeMf(error.to_string()))?;
    if model.build.items.is_empty() {
        return Err(ExportError::InvalidThreeMf(
            "3MF package contains no build items".into(),
        ));
    }
    Ok(())
}

/// Atomically writes a validated 3MF package.
///
/// # Errors
///
/// Returns [`ExportError`] when validation, encoding, or file output fails.
pub fn write_3mf(scene: &EvaluatedScene, path: impl AsRef<Path>) -> Result<(), ExportError> {
    write_atomic(path.as_ref(), &encode_3mf(scene)?).map_err(ExportError::from)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgba(feature_id: u64, color: [f32; 4]) -> Result<(u8, u8, u8, u8), ExportError> {
    if color
        .iter()
        .any(|channel| !channel.is_finite() || !(0.0..=1.0).contains(channel))
    {
        return Err(ExportError::InvalidColor {
            feature_id,
            message: "RGBA channels must be finite and between zero and one".into(),
        });
    }
    let [red, green, blue, alpha] = color.map(|channel| (channel * 255.0).round() as u8);
    Ok((red, green, blue, alpha))
}

#[cfg(test)]
mod tests {
    use cadx_core::kernel::{EvaluatedPart, TriangleMesh};

    use super::*;

    #[test]
    fn package_round_trip_preserves_named_object() {
        let scene = EvaluatedScene {
            parts: vec![EvaluatedPart {
                feature_id: 4,
                name: "fixture".into(),
                color: [0.2, 0.4, 0.8, 1.0],
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
        };
        let bytes = encode_3mf(&scene).unwrap();
        validate_3mf(&bytes).unwrap();
        let model = Model::from_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(model.resources.objects[0].name.as_deref(), Some("fixture"));
    }
}
