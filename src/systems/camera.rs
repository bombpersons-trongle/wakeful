//! Gameplay camera setup.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use crate::GameCamera;
use crate::screen::GameImage;

/// FF7-style fixed view. The pose is overwritten by the scene once it
/// loads; runs behind the background camera so the background shows
/// through.
pub fn setup_game_camera(mut commands: Commands, game_image: Res<GameImage>) {
    // MSAA stays off: the game image is single-sampled, and hard edges
    // match the pixelated pipeline.
    commands.spawn((
        GameCamera,
        Camera3d::default(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            ..default()
        },
        Msaa::Off,
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(0),
        Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}
