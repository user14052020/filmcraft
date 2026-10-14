// Original FilmCraft HDR presentation shader (MIT OR Apache-2.0).
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn vs(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs(in: VertexOutput) -> @location(0) vec4<f32> {
    // Premultiplied working pixels over black. Do not clamp HDR or wide-gamut channels.
    return vec4<f32>(textureSample(image, image_sampler, in.uv).rgb, 1.0);
}
