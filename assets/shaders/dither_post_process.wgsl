//! PSX-style ordered dithering applied to the game image.
//!
//! Runs as a fullscreen pass on the game camera, after the background and
//! 3D content have been drawn into the game image. The pass re-samples at
//! virtual texel centers, then offsets the color by a 4x4 Bayer threshold
//! before quantizing — trading bit depth for perceived shading like a
//! PS1 game.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;

struct DitherPostProcess {
    dither_strength: f32,
    color_steps: f32,
}

@group(0) @binding(2) var<uniform> settings: DitherPostProcess;

const GAME_W: f32 = 320.0;
const GAME_H: f32 = 240.0;

const BAYER_4X4: array<f32, 16> = array<f32, 16>(
    0.0,  8.0,  2.0, 10.0,
   12.0,  4.0, 14.0,  6.0,
    3.0, 11.0,  1.0,  9.0,
   15.0,  7.0, 13.0,  5.0
);

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = floor(in.uv * vec2<f32>(GAME_W, GAME_H));
    let uv_snapped = (texel + vec2<f32>(0.5)) / vec2<f32>(GAME_W, GAME_H);
    let color = textureSampleLevel(screen_texture, texture_sampler, uv_snapped, 0.0).rgb;

    let dither_p = vec2<u32>(texel);
    let threshold = BAYER_4X4[(dither_p.y % 4u) * 4u + (dither_p.x % 4u)] / 15.0 - 0.5;
    let dithered = color * settings.color_steps + threshold * settings.dither_strength;

    return vec4<f32>(clamp(floor(dithered) / settings.color_steps, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
