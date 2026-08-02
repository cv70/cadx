use std::{collections::HashSet, ops::Range};

use bytemuck::{Pod, Zeroable};
use cadx_core::{
    assembly::AssemblyTransform,
    domain::FeatureId,
    kernel::{EvaluatedPart, TriangleMesh},
};
use egui_wgpu::wgpu;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuMeshVertex {
    pub(crate) position: [f32; 3],
    pub(crate) normal: [f32; 3],
}

impl GpuMeshVertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    pub(crate) model: [[f32; 4]; 4],
    pub(crate) color: [f32; 4],
}

impl GpuInstance {
    const ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        2 => Float32x4,
        3 => Float32x4,
        4 => Float32x4,
        5 => Float32x4,
        6 => Float32x4
    ];

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn new(transform: AssemblyTransform, color: [f32; 4]) -> Self {
        let rotation = transform.rotation;
        Self {
            model: [
                [
                    rotation[0][0] as f32,
                    rotation[1][0] as f32,
                    rotation[2][0] as f32,
                    0.0,
                ],
                [
                    rotation[0][1] as f32,
                    rotation[1][1] as f32,
                    rotation[2][1] as f32,
                    0.0,
                ],
                [
                    rotation[0][2] as f32,
                    rotation[1][2] as f32,
                    rotation[2][2] as f32,
                    0.0,
                ],
                [
                    transform.translation[0] as f32,
                    transform.translation[1] as f32,
                    transform.translation[2] as f32,
                    1.0,
                ],
            ],
            color,
        }
    }

    pub(crate) fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SolidDraw {
    pub(crate) indices: Range<u32>,
    pub(crate) instances: Range<u32>,
}

#[derive(Debug, Default)]
pub(crate) struct SolidBuffers {
    pub(crate) vertices: Vec<GpuMeshVertex>,
    pub(crate) indices: Vec<u32>,
    pub(crate) instances: Vec<GpuInstance>,
    pub(crate) draws: Vec<SolidDraw>,
}

impl SolidBuffers {
    pub(crate) fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.instances.clear();
        self.draws.clear();
    }

    pub(crate) fn append(&mut self, mesh: &TriangleMesh, instances: &[GpuInstance]) -> bool {
        if instances.is_empty() || mesh.positions.len() != mesh.normals.len() {
            return false;
        }
        let Ok(base_vertex) = u32::try_from(self.vertices.len()) else {
            return false;
        };
        let Some(indices) = mesh
            .indices
            .iter()
            .map(|index| {
                usize::try_from(*index)
                    .ok()
                    .filter(|index| *index < mesh.positions.len())?;
                base_vertex.checked_add(*index)
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let Ok(first_index) = u32::try_from(self.indices.len()) else {
            return false;
        };
        let Ok(index_count) = u32::try_from(indices.len()) else {
            return false;
        };
        let Some(last_index) = first_index.checked_add(index_count) else {
            return false;
        };
        let Ok(first_instance) = u32::try_from(self.instances.len()) else {
            return false;
        };
        let Ok(instance_count) = u32::try_from(instances.len()) else {
            return false;
        };
        let Some(last_instance) = first_instance.checked_add(instance_count) else {
            return false;
        };

        self.vertices.extend(
            mesh.positions
                .iter()
                .zip(&mesh.normals)
                .map(|(&position, &normal)| GpuMeshVertex { position, normal }),
        );
        self.indices.extend(indices);
        self.instances.extend_from_slice(instances);
        self.draws.push(SolidDraw {
            indices: first_index..last_index,
            instances: first_instance..last_instance,
        });
        true
    }
}

pub(crate) fn triangle_subset(
    mesh: &TriangleMesh,
    triangles: &HashSet<u32>,
) -> Option<TriangleMesh> {
    let mut triangles = triangles.iter().copied().collect::<Vec<_>>();
    triangles.sort_unstable();
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut normals = Vec::with_capacity(triangles.len() * 3);
    for triangle in triangles {
        let start = usize::try_from(triangle).ok()?.checked_mul(3)?;
        for index in mesh.indices.get(start..start.checked_add(3)?)? {
            let index = usize::try_from(*index).ok()?;
            positions.push(*mesh.positions.get(index)?);
            normals.push(*mesh.normals.get(index)?);
        }
    }
    let indices = (0..positions.len())
        .map(u32::try_from)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(TriangleMesh {
        positions,
        normals,
        indices,
    })
}

pub(crate) fn part_display_color(part: &EvaluatedPart, selected: Option<FeatureId>) -> [f32; 4] {
    if selected == Some(part.feature_id) {
        [
            (part.color[0] * 1.25).min(1.0),
            (part.color[1] * 1.25).min(1.0),
            (part.color[2] * 1.25).min(1.0),
            1.0,
        ]
    } else {
        part.color
    }
}
