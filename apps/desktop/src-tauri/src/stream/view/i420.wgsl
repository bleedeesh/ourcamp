// One picture of a stream: three planes of I420 in, RGB out (ADR-0053).

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// One triangle bigger than the viewport: no vertex buffer, and no seam between two.
@vertex
fn vs(@builtin(vertex_index) index: u32) -> VertexOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var y_plane: texture_2d<f32>;
@group(0) @binding(1) var u_plane: texture_2d<f32>;
@group(0) @binding(2) var v_plane: texture_2d<f32>;
@group(0) @binding(3) var linear: sampler;

// BT.601 in limited range: what `argb_to_i420` of libyuv writes on the streamer's side.
@fragment
fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let y = (textureSample(y_plane, linear, in.uv).r - 16.0 / 255.0) * 255.0 / 219.0;
    let u = (textureSample(u_plane, linear, in.uv).r - 128.0 / 255.0) * 255.0 / 224.0;
    let v = (textureSample(v_plane, linear, in.uv).r - 128.0 / 255.0) * 255.0 / 224.0;
    let rgb = vec3<f32>(
        y + 1.402 * v,
        y - 0.344136 * u - 0.714136 * v,
        y + 1.772 * u,
    );
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
