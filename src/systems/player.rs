//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::movement::{PLAYER_SPEED, move_position};
use crate::scene::Scene;
use crate::{CurrentScene, Player};

/// Capsule geometry; `PLAYER_Y` keeps it resting on the ground plane.
/// The radius also drives walkable-grid collision; shipped-scene tests
/// assert arrivals fit a body of this size.
pub(crate) const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_HALF_HEIGHT: f32 = 0.5;
const PLAYER_Y: f32 = PLAYER_RADIUS + PLAYER_HALF_HEIGHT;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);

/// Spawns the player capsule at a world XZ position. Called by scene
/// application, so every scene starts with a fresh player; teleporters
/// pick the position via the scene's arrival data.
pub(crate) fn spawn_player(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    at: Vec2,
) {
    commands.spawn((
        Player,
        Mesh3d(meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_xyz(at.x, PLAYER_Y, at.y),
    ));
}

pub fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    editor: Option<Res<EditorState>>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    // Editing pauses play: the mouse paints cells and the camera pose is
    // whatever the panel says.
    if editor.is_some_and(|editor| editor.open) {
        return;
    }
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

    // The scene's walkable grid bounds where the player may go; the body
    // (not just the center point) stays inside, and sliding along blocked
    // cells keeps movement feeling responsive.
    let moved = current
        .as_ref()
        .and_then(|c| scenes.get(&c.handle))
        .and_then(|scene| scene.walkable.as_ref())
        .map(|grid| grid.constrain(from, moved, PLAYER_RADIUS))
        .unwrap_or(moved);

    transform.translation = Vec3::new(moved.x, PLAYER_Y, moved.y);
}
