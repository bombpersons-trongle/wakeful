//! PSX-style ordered dithering as a fullscreen post-process.
//!
//! The material is attached to the post-process camera — the last
//! camera that draws into the game image — so the pass runs over the
//! background, 3D content, and UI, dithering the finished frame in
//! place.
//!
//! This module has no in-crate dependencies so the `snapshot_mac_os`
//! binary can include it via `#[path]`.

use bevy::core_pipeline::fullscreen_material::FullscreenMaterial;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core2d, Core2dSystems};
use bevy::ecs::schedule::{IntoScheduleConfigs, ScheduleConfigs, ScheduleLabel};
use bevy::ecs::system::BoxedSystem;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;

/// Ordered-dither + color-quantize settings for the game image's
/// final post-process pass.
///
/// Tuned defaults: full-strength dithering on the virtual texel grid at
/// 4-bit color, matching a PS1-era look.
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default)]
pub struct DitherPostProcess {
    /// Bayer threshold offset strength; 0 disables dithering.
    pub dither_strength: f32,
    /// Color quantization levels per channel.
    pub color_steps: f32,
}

impl FullscreenMaterial for DitherPostProcess {
    fn fragment_shader() -> ShaderRef {
        "shaders/dither_post_process.wgsl".into()
    }

    // The bubble camera is a Camera2d, so the pass must run in the 2d
    // graph: the default Core3d schedule never touches 2d views, which
    // silently dropped the dither.
    fn schedule() -> impl ScheduleLabel + Clone {
        Core2d
    }

    fn schedule_configs(system: ScheduleConfigs<BoxedSystem>) -> ScheduleConfigs<BoxedSystem> {
        system
            .in_set(Core2dSystems::PostProcess)
            .before(tonemapping)
    }
}

/// Tuned default settings, shared by the game and the snapshot tool.
pub fn tuned() -> DitherPostProcess {
    DitherPostProcess {
        dither_strength: 1.0,
        color_steps: 15.0,
    }
}
