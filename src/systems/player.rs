//! The player actor: spawning and movement.

use bevy::prelude::*;

use crate::movement::{
    PLAYER_RUN_SPEED, PLAYER_SPEED, TURN_SPEED, camera_relative_direction, face_direction,
    facing_rotation, move_position,
};
use crate::scene::Scene;
use crate::systems::animation::Locomotion;
use crate::walkmesh;
use crate::{CurrentScene, Player};

/// The placeholder's geometry: a cone lying on its side, apex (the nose)
/// pointing along the facing direction. The radius also drives walk-mesh
/// collision; shipped-scene tests assert arrivals fit a body of this size.
pub(crate) const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_LENGTH: f32 = 1.2;
/// Resting height of the lying cone above the ground: its base rim
/// touches it. Added to the walk mesh's height whenever the scene
/// application repositions the persistent player.
pub(crate) const PLAYER_Y: f32 = PLAYER_RADIUS;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);

/// Spawns the placeholder player at a world XZ position, standing on
/// `ground`, facing `toward` (a ground-plane direction; usually the
/// scene's camera forward, so the player starts pointing screen-up).
/// Called by scene application, so every scene starts with a fresh
/// player; teleporters pick the position via the scene's arrival data.
/// The cone body is a pitched child — the entity itself stays upright so
/// attached models stand straight.
pub(crate) fn spawn_player(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    at: Vec2,
    ground: f32,
    toward: Vec2,
) -> Entity {
    let player = commands
        .spawn((
            Player,
            Locomotion::default(),
            Visibility::default(),
            Transform::from_xyz(at.x, ground + PLAYER_Y, at.y).with_rotation(facing_rotation(toward)),
        ))
        .id();
    spawn_placeholder_body(commands, player, meshes, materials);
    player
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

#[allow(clippy::too_many_arguments)]
pub fn move_player(
    time: Res<Time>,
    input: Res<crate::input::InputManager>,
    captures: Res<crate::input::InputCaptures>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    pause: Res<crate::systems::ui::UiPause>,
    mut players: Query<(&mut Transform, Option<&mut Locomotion>), With<Player>>,
) {
    // Script UI can pause the world — FF7 menus freeze the field.
    // Battles freeze it by state instead.
    if pause.0 {
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
    // left stick sum into the same vector — minus whatever a script
    // captured (the free camera flying on the d-pad).
    let screen = input.movement(&captures);

    let from = transform.translation.xz();
    let direction = camera_relative_direction(screen, forward);
    let running = !captures.blocks(crate::input::PadButton::R2)
        && input.pressed(crate::input::PadButton::R2);
    let speed = if running {
        PLAYER_RUN_SPEED
    } else {
        PLAYER_SPEED
    };
    let moved = move_position(from, direction, speed, time.delta_secs());

    // The scene's walk mesh bounds where the player may go and says how
    // high the ground is there: the body (not just the center point) stays
    // on it, and sliding along its edge keeps movement feeling responsive.
    let mesh = scene.walk_mesh.as_ref();
    let moved = mesh
        .map(|mesh| mesh.constrain(from, moved, PLAYER_RADIUS))
        .unwrap_or(moved);
    let ground = walkmesh::ground_height(mesh, moved.x, moved.y, from);

    transform.translation = Vec3::new(moved.x, ground + PLAYER_Y, moved.y);
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
