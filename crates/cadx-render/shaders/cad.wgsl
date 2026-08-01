struct Camera {
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
    output.normal = input.normal;
    output.color = input.color;
    return output;
}

@fragment
fn fs_solid(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal);
    let key_light = normalize(vec3<f32>(0.35, -0.45, 0.82));
    let fill_light = normalize(vec3<f32>(-0.65, 0.25, 0.45));
    let key = max(dot(normal, key_light), 0.0);
    let fill = max(dot(normal, fill_light), 0.0) * 0.22;
    let light = 0.28 + key * 0.68 + fill;
    return vec4<f32>(input.color.rgb * light, input.color.a);
}

@fragment
fn fs_grid(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
