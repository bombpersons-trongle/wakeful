//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::movement::{
    PLAYER_SPEED, TURN_SPEED, camera_relative_direction, face_direction, facing_rotation,
    move_position,
};
use crate::scene::Scene;
use crate::{CurrentScene, Player};

/// The placeholder's geometry: a cone lying on its side, apex (the nose)
/// pointing along the facing direction. The radius also drives
/// walkable-grid collision; shipped-scene tests assert arrivals fit a
/// body of this size.
pub(crate) const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_LENGTH: f32 = 1.2;
/// Resting height of the lying cone: its base rim touches the ground.
const PLAYER_Y: f32 = PLAYER_RADIUS;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);

/// Spawns the placeholder player at a world XZ position, facing `toward`
/// (a ground-plane direction; usually the scene's camera forward, so the
/// player starts pointing screen-up). Called by scene application, so
/// every scene starts with a fresh player; teleporters pick the position
/// via the scene's arrival data.
pub(crate) fn spawn_player(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    at: Vec2,
    toward: Vec2,
) {
    commands.spawn((
        Player,
        Mesh3d(meshes.add(Cone {
            radius: PLAYER_RADIUS,
            height: PLAYER_LENGTH,
        })),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_xyz(at.x, PLAYER_Y, at.y).with_rotation(facing_rotation(toward)),
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
    let forward = scene.camera_forward();

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
    let direction = camera_relative_direction(screen, forward);
    let moved = move_position(from, direction, PLAYER_SPEED, time.delta_secs());

    // The scene's walkable grid bounds where the player may go; the body
    // (not just the center point) stays inside, and sliding along blocked
    // cells keeps movement feeling responsive.
    let moved = scene
        .walkable
        .as_ref()
        .map(|grid| grid.constrain(from, moved, PLAYER_RADIUS))
        .unwrap_or(moved);

    transform.translation = Vec3::new(moved.x, PLAYER_Y, moved.y);
    // Ease the nose toward the movement direction; idling keeps the last
    // facing.
    transform.rotation =
        face_direction(transform.rotation, direction, TURN_SPEED, time.delta_secs());
}
