//! PSX-style ordered dithering as a fullscreen post-process.
//!
//! The material is attached to the game camera, so the pass runs after
//! the background and 3D content have been drawn into the game image and
//! dithers the finished frame in place — backgrounds and 3D alike.
//!
//! This module has no in-crate dependencies so the `snapshot` binary can
//! include it via `#[path]`.

use bevy::core_pipeline::fullscreen_material::FullscreenMaterial;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::render_resource::ShaderType;
use bevy::shader::ShaderRef;

/// Ordered-dither + color-quantize settings for the game camera's
/// post-process pass.
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
}

/// Tuned default settings, shared by the game and the snapshot tool.
pub fn tuned() -> DitherPostProcess {
    DitherPostProcess {
        dither_strength: 1.0,
        color_steps: 15.0,
    }
}
