// The transition cover. Mode 1 (the swirl) samples the frozen frame,
// magnifying about the screen center while the black cover closes over
// it; mode 0 is the plain cover — black at the progress opacity.
// The bindings mirror `TransitionMaterial` in src/transition.rs.

#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(2) @binding(0) var<uniform> effect: Effect;
@group(2) @binding(1) var frozen_frame: texture_2d<f32>;
@group(2) @binding(2) var frozen_frame_sampler: sampler;

struct Effect {
    mode: u32,
    progress: f32,
}

@fragment
fn fragment(
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
    let covered = clamp(effect.progress, 0.0, 1.0);
    if (effect.mode == 1u) {
        // The swirl: the frozen picture magnifies about the screen
        // center while the black cover rises over it.
        let zoom = 1.0 - 0.15 * covered;
        let uv = (mesh.uv - vec2<f32>(0.5, 0.5)) * zoom + vec2<f32>(0.5, 0.5);
        let frame = textureSampleLevel(frozen_frame, frozen_frame_sampler, uv, 0.0).rgb;
        return vec4<f32>(mix(frame, vec3<f32>(0.0), covered), 1.0);
    }
    return vec4<f32>(vec3<f32>(0.0), covered);
}
