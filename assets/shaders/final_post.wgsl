//! The final post-processing pass: PSX ordered dithering + color
//! quantization, then CRT-style display simulation, over the presented
//! frame.
//!
//! Runs on the present camera, after the finished game image has been
//! blitted to the window, so the effects cover the upscaled picture
//! exactly like a tube showing a 240p console signal. The dither runs
//! first (quantizing the signal the way a 240p console would emit it),
//! then the CRT effects layer the display simulation on top.
//!
//! Every effect is periodic in virtual-pixel space — one scanline per
//! game row (240 rows, like the signal itself) and one RGB stripe
//! triple per game pixel column — so the pattern stays stable at any
//! window size and the shader needs no window-size uniform. Barrel
//! curvature would be the exception to that; deliberately not
//! implemented.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var texture_sampler: sampler;

struct FinalPostMaterial {
    bleed: f32,
    scanline: f32,
    mask: f32,
    vignette: f32,
    dither_strength: f32,
    color_steps: f32,
}

@group(0) @binding(2) var<uniform> settings: FinalPostMaterial;

const GAME_W: f32 = 320.0;
const GAME_H: f32 = 240.0;

/// Signal bleed: the cable's limited bandwidth smears sharp color
/// edges horizontally, one and two game pixels out to either side.
/// The five taps carry gaussian-like weights (1, 4, 6, 4, 1) and
/// conserve total energy, so no brightness correction is needed.
fn signal_bleed(center: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    let px = 1.0 / GAME_W;
    let near =
        textureSampleLevel(screen_texture, texture_sampler, uv - vec2<f32>(px, 0.0), 0.0).rgb
            + textureSampleLevel(screen_texture, texture_sampler, uv + vec2<f32>(px, 0.0), 0.0).rgb;
    let far =
        textureSampleLevel(screen_texture, texture_sampler, uv - vec2<f32>(2.0 * px, 0.0), 0.0).rgb
            + textureSampleLevel(screen_texture, texture_sampler, uv + vec2<f32>(2.0 * px, 0.0), 0.0).rgb;
    return (center * 6.0 + near * 4.0 + far) / 16.0;
}

/// 4x4 Bayer threshold matrix, tiled across the screen in virtual
/// texels.
const BAYER_4X4: array<f32, 16> = array<f32, 16>(
    0.0,  8.0,  2.0, 10.0,
   12.0,  4.0, 14.0,  6.0,
    3.0, 11.0,  1.0,  9.0,
   15.0,  7.0, 13.0,  5.0
);

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    var color = textureSampleLevel(screen_texture, texture_sampler, in.uv, 0.0).rgb;

    // Ordered dither + color quantize first: the frame is pushed
    // toward a 240p console's 4-bit-ish palette before the tube
    // simulation layers on top. A strength of 0 leaves raw colors.
    if (settings.dither_strength > 0.0) {
        let texel = floor(in.uv * vec2<f32>(320.0, 240.0));
        let bayer = BAYER_4X4[u32(texel.x + texel.y * 4.0) % 16u] / 16.0 - 0.5;
        color += bayer * settings.dither_strength / settings.color_steps;
        color = floor(color * settings.color_steps) / settings.color_steps;
    }

    // Bleed first: the cable smears the signal before the tube shows
    // it. A bleed of 0 keeps the frame perfectly sharp.
    var out = mix(color, signal_bleed(color, in.uv), settings.bleed);

    // Scanline: fade toward each game row's edge. `line` is 0 at the
    // row center and 1 at the row boundary.
    let line = abs(fract(in.uv.y * GAME_H) - 0.5) * 2.0;
    out *= 1.0 - settings.scanline * line;

    // Aperture mask: each game pixel column gets one RGB phosphor
    // stripe; the two non-matching channels are dimmed.
    let stripe = u32(floor(in.uv.x * GAME_W)) % 3u;
    var dim = vec3<f32>(1.0 - settings.mask);
    if (stripe == 0u) {
        dim.r = 1.0;
    } else if (stripe == 1u) {
        dim.g = 1.0;
    } else {
        dim.b = 1.0;
    }
    out *= dim;

    // Vignette: darken toward the screen corners.
    let d = distance(in.uv, vec2<f32>(0.5, 0.5));
    let vig = 1.0 - settings.vignette * smoothstep(0.4, 0.9, d);
    out *= vig;

    // Keep mean brightness near the source: scanlines average
    // 1 - scanline/2, the mask 1 - 2*mask/3.
    let gain = 1.0
        / ((1.0 - settings.scanline * 0.5) * (1.0 - settings.mask * 2.0 / 3.0));
    out *= gain;

    return vec4<f32>(out, 1.0);
}
