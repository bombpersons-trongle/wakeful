//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::movement::{
    PLAYER_RUN_SPEED, PLAYER_SPEED, TURN_SPEED, camera_relative_direction, face_direction,
    facing_rotation, move_position,
};
use crate::scene::Scene;
use crate::systems::animation::Locomotion;
use crate::{CurrentScene, Player};

/// The placeholder's geometry: a cone lying on its side, apex (the nose)
/// pointing along the facing direction. The radius also drives
/// walkable-grid collision; shipped-scene tests assert arrivals fit a
/// body of this size.
pub(crate) const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_LENGTH: f32 = 1.2;
/// Resting height of the lying cone: its base rim touches the ground.
/// Also the body height used whenever the scene application repositions
/// the persistent player.
pub(crate) const PLAYER_Y: f32 = PLAYER_RADIUS;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);

/// Spawns the placeholder player at a world XZ position, facing `toward`
/// (a ground-plane direction; usually the scene's camera forward, so the
/// player starts pointing screen-up). Called by scene application, so
/// every scene starts with a fresh player; teleporters pick the position
/// via the scene's arrival data. The cone body is a pitched child — the
/// entity itself stays upright so attached models stand straight.
pub(crate) fn spawn_player(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    at: Vec2,
    toward: Vec2,
) {
    let player = commands
        .spawn((
            Player,
            Locomotion::default(),
            Transform::from_xyz(at.x, PLAYER_Y, at.y).with_rotation(facing_rotation(toward)),
        ))
        .id();
    spawn_placeholder_body(commands, player, meshes, materials);
}

/// Spawns the placeholder cone under `player`, pitched along its +Z
/// front. Shared by player spawn and the party's revert-to-capsule path.
pub(crate) fn spawn_placeholder_body(
    commands: &mut Commands,
    player: Entity,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    // The cone's apex is +Y; pitch it to lie along the body's +Z front.
    // Real models need no such correction and attach upright instead.
    commands.entity(player).with_child((
        PlaceholderBody,
        Mesh3d(meshes.add(Cone {
            radius: PLAYER_RADIUS,
            height: PLAYER_LENGTH,
        })),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::Z)),
    ));
}

/// The placeholder cone body; despawned when a real model attaches.
#[derive(Component)]
pub(crate) struct PlaceholderBody;

pub fn move_player(
    time: Res<Time>,
    input: Res<crate::input::InputManager>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    editor: Option<Res<EditorState>>,
    mut players: Query<(&mut Transform, Option<&mut Locomotion>), With<Player>>,
) {
    // Editing pauses play: the mouse paints cells and the camera pose is
    // whatever the panel says.
    if editor.is_some_and(|editor| editor.open) {
        return;
    }
    let Ok((mut transform, locomotion)) = players.single_mut() else {
        return;
    };
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    let forward = scene.camera_forward();

    // Input actions are camera-relative: up walks away from the camera,
    // right walks to its screen-right, so controls stay intuitive
    // whichever way the scene's camera faces. The dpad and the (gated)
    // left stick sum into the same vector.
    let screen = input.movement();

    let from = transform.translation.xz();
    let direction = camera_relative_direction(screen, forward);
    let running = input.pressed(crate::input::PadButton::R2);
    let speed = if running {
        PLAYER_RUN_SPEED
    } else {
        PLAYER_SPEED
    };
    let moved = move_position(from, direction, speed, time.delta_secs());

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
    // facing. The gait follows actual displacement, so pushing into a
    // wall reads as standing, not speedwalking.
    transform.rotation =
        face_direction(transform.rotation, direction, TURN_SPEED, time.delta_secs());
    if let Some(mut locomotion) = locomotion {
        let travelled = moved.distance_squared(from);
        locomotion.moving = travelled > 1e-9;
        locomotion.running = locomotion.moving && running;
    }
}
