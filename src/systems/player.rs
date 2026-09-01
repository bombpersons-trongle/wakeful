//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::movement::{PLAYER_SPEED, camera_relative_direction, move_position};
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
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    let position = Vec2::new(scene.camera.position[0], scene.camera.position[2]);
    let target = Vec2::new(scene.camera.target[0], scene.camera.target[2]);
    let forward = (target - position).normalize_or_zero();

    // Arrows are camera-relative: up walks away from the camera, right
    // walks to its screen-right, so controls stay intuitive whichever
    // way the scene's camera faces.
    let mut screen = Vec2::ZERO;
    if keys.pressed(KeyCode::ArrowUp) {
        screen.y += 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        screen.y -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        screen.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        screen.x += 1.0;
    }

    let from = transform.translation.xz();
    let moved = move_position(
        from,
        camera_relative_direction(screen, forward),
        PLAYER_SPEED,
        time.delta_secs(),
    );

    // The scene's walkable grid bounds where the player may go; the body
    // (not just the center point) stays inside, and sliding along blocked
    // cells keeps movement feeling responsive.
    let moved = scene
        .walkable
        .as_ref()
        .map(|grid| grid.constrain(from, moved, PLAYER_RADIUS))
        .unwrap_or(moved);

    transform.translation = Vec3::new(moved.x, PLAYER_Y, moved.y);
}
