//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::movement::{PLAYER_SPEED, move_position};
use crate::scene::Scene;
use crate::{CurrentScene, Player};

/// Capsule geometry; `PLAYER_Y` keeps it resting on the ground plane.
const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_HALF_HEIGHT: f32 = 0.5;
const PLAYER_Y: f32 = PLAYER_RADIUS + PLAYER_HALF_HEIGHT;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);

pub fn spawn_player(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Player,
        Mesh3d(meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_xyz(0.0, PLAYER_Y, 0.0),
    ));
}

pub fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    let Ok(mut transform) = players.single_mut() else {
        return;
    };

    // Arrows map onto the ground plane as seen by the fixed camera:
    // up walks away from the camera, down walks toward it.
    let mut direction = Vec2::ZERO;
    if keys.pressed(KeyCode::ArrowUp) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    let from = transform.translation.xz();
    let moved = move_position(from, direction, PLAYER_SPEED, time.delta_secs());

    // The scene's walkable grid bounds where the player may go; sliding
    // along blocked cells keeps movement feeling responsive.
    let moved = current
        .as_ref()
        .and_then(|c| scenes.get(&c.0))
        .and_then(|scene| scene.walkable.as_ref())
        .map(|grid| grid.constrain(from, moved))
        .unwrap_or(moved);

    transform.translation = Vec3::new(moved.x, PLAYER_Y, moved.y);
}
