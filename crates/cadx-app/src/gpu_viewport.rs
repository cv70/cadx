use std::fmt;
use std::mem::size_of;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use cadx_core::EntityId;
use cadx_render::{Bounds3, MechanicalScene, OrbitCamera, ViewportSize};
use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;

pub(crate) const GPU_DEPTH_BITS: u8 = 32;
pub(crate) const GPU_MSAA_SAMPLES: u16 = 4;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_GPU_FACE_INDICES: usize = cadx_render::MAX_MECHANICAL_TRIANGLES * 3;
const MAX_GPU_EDGE_INDICES: usize = cadx_render::MAX_MECHANICAL_VERTICES * 3;

const SHADER: &str = r#"
struct CameraUniform {
    view_projection: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    selected_entity: vec2<u32>,
    selected_valid: u32,
    padding: u32,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) entity_id: vec2<u32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) entity_id: vec2<u32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
    output.world_position = input.position;
    output.color = input.color;
    output.entity_id = input.entity_id;
    return output;
}

fn is_selected(entity_id: vec2<u32>) -> bool {
    return camera.selected_valid != 0u && all(entity_id == camera.selected_entity);
}

@fragment
fn fs_face(input: VertexOutput) -> @location(0) vec4<f32> {
    var normal = normalize(cross(dpdx(input.world_position), dpdy(input.world_position)));
    let view_direction = normalize(camera.camera_position.xyz - input.world_position);
    if dot(normal, view_direction) < 0.0 {
        normal = -normal;
    }
    let light = normalize(camera.light_direction.xyz);
    let intensity = clamp(0.25 + 0.75 * max(dot(normal, light), 0.0), 0.18, 1.0);
    let selection_boost = select(1.0, 1.18, is_selected(input.entity_id));
    return vec4<f32>(
        input.color.rgb * clamp(intensity * selection_boost, 0.18, 1.0),
        input.color.a,
    );
}

@fragment
fn fs_edge(input: VertexOutput) -> @location(0) vec4<f32> {
    let regular = vec4<f32>(0.04, 0.055, 0.063, 0.72);
    let selected = vec4<f32>(0.49, 0.93, 0.83, 1.0);
    return select(regular, selected, is_selected(input.entity_id));
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 3],
    color: [u8; 4],
    entity_id: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    camera_position: [f32; 4],
    light_direction: [f32; 4],
    selected_entity: [u32; 2],
    selected_valid: u32,
    padding: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MechanicalGpuScene {
    vertices: Vec<GpuVertex>,
    face_indices: Vec<u32>,
    edge_indices: Vec<u32>,
    pub(crate) bounds: Option<Bounds3>,
}

impl MechanicalGpuScene {
    pub(crate) fn from_scene(scene: &MechanicalScene) -> Result<Self, GpuSceneError> {
        let mut gpu = Self {
            vertices: Vec::new(),
            face_indices: Vec::new(),
            edge_indices: Vec::new(),
            bounds: scene.bounds,
        };
        for item in &scene.items {
            let vertex_offset =
                u32::try_from(gpu.vertices.len()).map_err(|_| GpuSceneError::LimitExceeded {
                    resource: "vertices",
                    limit: cadx_render::MAX_MECHANICAL_VERTICES,
                })?;
            let entity_id = split_entity_id(item.entity_id);
            for point in &item.mesh.vertices {
                let position = [point.x as f32, point.y as f32, point.z as f32];
                if position.iter().any(|value| !value.is_finite()) {
                    return Err(GpuSceneError::CoordinateOutOfRange {
                        entity: item.entity_id,
                    });
                }
                gpu.vertices.push(GpuVertex {
                    position,
                    color: item.color,
                    entity_id,
                });
            }
            for triangle in &item.mesh.triangles {
                for index in triangle {
                    gpu.face_indices
                        .push(vertex_offset.checked_add(*index).ok_or(
                            GpuSceneError::LimitExceeded {
                                resource: "face indices",
                                limit: MAX_GPU_FACE_INDICES,
                            },
                        )?);
                }
            }
            for edge in &item.mesh.feature_edges {
                for index in edge {
                    gpu.edge_indices
                        .push(vertex_offset.checked_add(*index).ok_or(
                            GpuSceneError::LimitExceeded {
                                resource: "edge indices",
                                limit: MAX_GPU_EDGE_INDICES,
                            },
                        )?);
                }
            }
        }
        if gpu.vertices.len() > cadx_render::MAX_MECHANICAL_VERTICES {
            return Err(GpuSceneError::LimitExceeded {
                resource: "vertices",
                limit: cadx_render::MAX_MECHANICAL_VERTICES,
            });
        }
        if gpu.face_indices.len() > MAX_GPU_FACE_INDICES {
            return Err(GpuSceneError::LimitExceeded {
                resource: "face indices",
                limit: MAX_GPU_FACE_INDICES,
            });
        }
        if gpu.edge_indices.len() > MAX_GPU_EDGE_INDICES {
            return Err(GpuSceneError::LimitExceeded {
                resource: "edge indices",
                limit: MAX_GPU_EDGE_INDICES,
            });
        }
        Ok(gpu)
    }

    #[cfg(test)]
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        (
            self.vertices.len(),
            self.face_indices.len(),
            self.edge_indices.len(),
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MechanicalGpuCallback {
    scene: Arc<MechanicalGpuScene>,
    scene_revision: u64,
    camera: CameraUniform,
}

impl MechanicalGpuCallback {
    pub(crate) fn new(
        scene: Arc<MechanicalGpuScene>,
        scene_revision: u64,
        camera: OrbitCamera,
        viewport: ViewportSize,
        selected_entity: Option<EntityId>,
    ) -> Result<Self, GpuSceneError> {
        let bounds = scene.bounds.ok_or(GpuSceneError::EmptyScene)?;
        let projection = camera
            .projection(bounds, viewport)
            .ok_or(GpuSceneError::InvalidCamera)?;
        let camera_position = [
            projection.camera_position.x as f32,
            projection.camera_position.y as f32,
            projection.camera_position.z as f32,
            1.0,
        ];
        if camera_position.iter().any(|value| !value.is_finite()) {
            return Err(GpuSceneError::InvalidCamera);
        }
        let (selected_entity, selected_valid) = selected_entity
            .map(|id| (split_entity_id(id), 1))
            .unwrap_or(([0, 0], 0));
        Ok(Self {
            scene,
            scene_revision,
            camera: CameraUniform {
                view_projection: projection.view_projection,
                camera_position,
                light_direction: [0.35, -0.55, 1.0, 0.0],
                selected_entity,
                selected_valid,
                padding: 0,
            },
        })
    }

    pub(crate) fn paint_callback(self, rect: egui::Rect) -> egui::PaintCallback {
        egui_wgpu::Callback::new_paint_callback(rect, self)
    }
}

impl CallbackTrait for MechanicalGpuCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let resources = callback_resources
            .get_mut::<MechanicalGpuResources>()
            .expect("mechanical GPU resources are installed at startup");
        if resources.uploaded_revision != Some(self.scene_revision) {
            resources.upload_scene(device, queue, &self.scene);
            resources.uploaded_revision = Some(self.scene_revision);
        }
        queue.write_buffer(
            &resources.uniform_buffer,
            0,
            bytemuck::bytes_of(&self.camera),
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let resources = callback_resources
            .get::<MechanicalGpuResources>()
            .expect("mechanical GPU resources are installed at startup");
        if resources.face_index_count == 0 {
            return;
        }
        render_pass.set_pipeline(&resources.face_pipeline);
        render_pass.set_bind_group(0, &resources.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, resources.vertex_buffer.slice(..));
        render_pass.set_index_buffer(
            resources.face_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        render_pass.draw_indexed(0..resources.face_index_count, 0, 0..1);
        if resources.edge_index_count > 0 {
            render_pass.set_pipeline(&resources.edge_pipeline);
            render_pass.set_index_buffer(
                resources.edge_index_buffer.slice(..),
                wgpu::IndexFormat::Uint32,
            );
            render_pass.draw_indexed(0..resources.edge_index_count, 0, 0..1);
        }
    }
}

struct MechanicalGpuResources {
    face_pipeline: wgpu::RenderPipeline,
    edge_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    face_index_buffer: wgpu::Buffer,
    edge_index_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    face_index_capacity: u64,
    edge_index_capacity: u64,
    face_index_count: u32,
    edge_index_count: u32,
    uploaded_revision: Option<u64>,
}

impl MechanicalGpuResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadx_mechanical_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadx_mechanical_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadx_mechanical_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadx_mechanical_uniform_buffer"),
            size: size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadx_mechanical_uniform_bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let face_pipeline = create_pipeline(
            device,
            target_format,
            &pipeline_layout,
            &shader,
            PipelineSpec {
                topology: wgpu::PrimitiveTopology::TriangleList,
                fragment_entry: "fs_face",
                label: "cadx_mechanical_face_pipeline",
                depth_write_enabled: true,
            },
        );
        let edge_pipeline = create_pipeline(
            device,
            target_format,
            &pipeline_layout,
            &shader,
            PipelineSpec {
                topology: wgpu::PrimitiveTopology::LineList,
                fragment_entry: "fs_edge",
                label: "cadx_mechanical_edge_pipeline",
                depth_write_enabled: false,
            },
        );
        Self {
            face_pipeline,
            edge_pipeline,
            uniform_buffer,
            uniform_bind_group,
            vertex_buffer: create_buffer(
                device,
                "cadx_mechanical_vertex_buffer",
                4,
                wgpu::BufferUsages::VERTEX,
            ),
            face_index_buffer: create_buffer(
                device,
                "cadx_mechanical_face_index_buffer",
                4,
                wgpu::BufferUsages::INDEX,
            ),
            edge_index_buffer: create_buffer(
                device,
                "cadx_mechanical_edge_index_buffer",
                4,
                wgpu::BufferUsages::INDEX,
            ),
            vertex_capacity: 4,
            face_index_capacity: 4,
            edge_index_capacity: 4,
            face_index_count: 0,
            edge_index_count: 0,
            uploaded_revision: None,
        }
    }

    fn upload_scene(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &MechanicalGpuScene,
    ) {
        let vertex_bytes = bytemuck::cast_slice(&scene.vertices);
        let face_bytes = bytemuck::cast_slice(&scene.face_indices);
        let edge_bytes = bytemuck::cast_slice(&scene.edge_indices);
        ensure_buffer(
            device,
            &mut self.vertex_buffer,
            &mut self.vertex_capacity,
            vertex_bytes.len() as u64,
            "cadx_mechanical_vertex_buffer",
            wgpu::BufferUsages::VERTEX,
        );
        ensure_buffer(
            device,
            &mut self.face_index_buffer,
            &mut self.face_index_capacity,
            face_bytes.len() as u64,
            "cadx_mechanical_face_index_buffer",
            wgpu::BufferUsages::INDEX,
        );
        ensure_buffer(
            device,
            &mut self.edge_index_buffer,
            &mut self.edge_index_capacity,
            edge_bytes.len() as u64,
            "cadx_mechanical_edge_index_buffer",
            wgpu::BufferUsages::INDEX,
        );
        if !vertex_bytes.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, vertex_bytes);
        }
        if !face_bytes.is_empty() {
            queue.write_buffer(&self.face_index_buffer, 0, face_bytes);
        }
        if !edge_bytes.is_empty() {
            queue.write_buffer(&self.edge_index_buffer, 0, edge_bytes);
        }
        self.face_index_count = scene.face_indices.len() as u32;
        self.edge_index_count = scene.edge_indices.len() as u32;
    }
}

pub(crate) fn install_gpu_resources(render_state: &egui_wgpu::RenderState) -> String {
    let resources = MechanicalGpuResources::new(&render_state.device, render_state.target_format);
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
    let info = render_state.adapter.get_info();
    format!("{:?}: {}", info.backend, info.name)
}

fn create_pipeline(
    device: &wgpu::Device,
    target_format: wgpu::TextureFormat,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    spec: PipelineSpec,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(spec.label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: size_of::<GpuVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3,
                    1 => Unorm8x4,
                    2 => Uint32x2
                ],
            }],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: spec.topology,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: spec.depth_write_enabled,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: if spec.depth_write_enabled {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.0,
                    clamp: 0.0,
                }
            } else {
                wgpu::DepthBiasState::default()
            },
        }),
        multisample: wgpu::MultisampleState {
            count: u32::from(GPU_MSAA_SAMPLES),
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(spec.fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview: None,
        cache: None,
    })
}

#[derive(Clone, Copy)]
struct PipelineSpec {
    topology: wgpu::PrimitiveTopology,
    fragment_entry: &'static str,
    label: &'static str,
    depth_write_enabled: bool,
}

fn create_buffer(
    device: &wgpu::Device,
    label: &'static str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn ensure_buffer(
    device: &wgpu::Device,
    buffer: &mut wgpu::Buffer,
    capacity: &mut u64,
    required: u64,
    label: &'static str,
    usage: wgpu::BufferUsages,
) {
    if required <= *capacity {
        return;
    }
    let next_capacity = required.next_power_of_two().max(4);
    *buffer = create_buffer(device, label, next_capacity, usage);
    *capacity = next_capacity;
}

fn split_entity_id(entity_id: EntityId) -> [u32; 2] {
    [entity_id as u32, (entity_id >> 32) as u32]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GpuSceneError {
    EmptyScene,
    InvalidCamera,
    CoordinateOutOfRange {
        entity: EntityId,
    },
    LimitExceeded {
        resource: &'static str,
        limit: usize,
    },
}

impl fmt::Display for GpuSceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScene => formatter.write_str("the mechanical scene has no visible solids"),
            Self::InvalidCamera => formatter.write_str("the mechanical camera is invalid"),
            Self::CoordinateOutOfRange { entity } => {
                write!(formatter, "entity {entity} exceeds GPU coordinate range")
            }
            Self::LimitExceeded { resource, limit } => {
                write!(formatter, "GPU {resource} exceeds the limit of {limit}")
            }
        }
    }
}

impl std::error::Error for GpuSceneError {}

#[cfg(test)]
mod tests {
    use cadx_core::Point2;
    use cadx_render::{MechanicalItem, Point3, SolidMesh};

    use super::*;

    fn gpu_test_scene(entity_id: EntityId) -> MechanicalScene {
        let mesh = SolidMesh::extrude(
            &[
                Point2::new(0.0, 0.0),
                Point2::new(10.0, 0.0),
                Point2::new(10.0, 6.0),
                Point2::new(0.0, 6.0),
            ],
            4.0,
        )
        .unwrap();
        MechanicalScene {
            bounds: Some(mesh.bounds),
            items: vec![MechanicalItem {
                entity_id,
                layer_id: 1,
                color: [73, 184, 165, 255],
                locked: false,
                mesh,
            }],
        }
    }

    #[test]
    fn gpu_scene_preserves_indexed_geometry_color_and_full_entity_id() {
        let entity_id = (u64::from(u32::MAX) << 32) | 17;
        let gpu = MechanicalGpuScene::from_scene(&gpu_test_scene(entity_id)).unwrap();

        assert_eq!(gpu.counts(), (8, 36, 24));
        assert_eq!(gpu.vertices[0].color, [73, 184, 165, 255]);
        assert_eq!(gpu.vertices[0].entity_id, [17, u32::MAX]);
        assert!(gpu.face_indices.iter().all(|index| *index < 8));
        assert!(gpu.edge_indices.iter().all(|index| *index < 8));
    }

    #[test]
    fn gpu_scene_rejects_coordinates_outside_f32_range() {
        let mut scene = gpu_test_scene(9);
        scene.items[0].mesh.vertices[0] = Point3::new(f64::MAX, 0.0, 0.0);

        assert_eq!(
            MechanicalGpuScene::from_scene(&scene).unwrap_err(),
            GpuSceneError::CoordinateOutOfRange { entity: 9 }
        );
    }

    #[test]
    fn gpu_callback_rejects_invalid_camera_or_viewport() {
        let scene = Arc::new(MechanicalGpuScene::from_scene(&gpu_test_scene(2)).unwrap());
        assert_eq!(
            MechanicalGpuCallback::new(
                Arc::clone(&scene),
                1,
                OrbitCamera::default(),
                ViewportSize::new(0.0, 600.0),
                None,
            )
            .unwrap_err(),
            GpuSceneError::InvalidCamera
        );
        let invalid_camera = OrbitCamera {
            distance: f64::NAN,
            ..OrbitCamera::default()
        };
        assert_eq!(
            MechanicalGpuCallback::new(
                scene,
                1,
                invalid_camera,
                ViewportSize::new(800.0, 600.0),
                None,
            )
            .unwrap_err(),
            GpuSceneError::InvalidCamera
        );
    }
}
