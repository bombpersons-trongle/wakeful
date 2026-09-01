//! Teleporters: trigger rects in the scene that load another scene when
//! the player touches one.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::scene::Scene;
use crate::{CurrentScene, PendingTeleport, Player};

/// Runs after `move_player`: when the player's position lands inside a
/// teleporter's trigger rect, queues a scene transition. Editing pauses
/// the trigger like movement, and a pending transition absorbs
/// re-triggers until `transition_scene` consumes it.
pub fn check_teleporters(
    mut commands: Commands,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    editor: Option<Res<EditorState>>,
    pending: Option<Res<PendingTeleport>>,
    players: Query<&Transform, With<Player>>,
) {
    if editor.is_some_and(|editor| editor.open) {
        return;
    }
    if pending.is_some() {
        return;
    }
    let Ok(transform) = players.single() else {
        return;
    };
    let Some(scene) = current.as_ref().and_then(|c| scenes.get(&c.handle)) else {
        return;
    };
    let at = transform.translation.xz();
    let Some(teleporter) = scene.teleporter_at(at.x, at.y) else {
        return;
    };
    commands.insert_resource(PendingTeleport {
        target: teleporter.target.clone(),
        arrival: teleporter.arrival.into(),
    });
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use crate::scene::{CameraPose, Teleporter};

    use super::*;

    fn scene_with_teleporter() -> Scene {
        Scene {
            background: None,
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walkable: None,
            character_model: None,
            teleporters: vec![Teleporter {
                position: [2.0, 0.0],
                size: [2.0, 2.0],
                target: "scenes/room2.scene".into(),
                arrival: [1.0, 2.0],
            }],
        }
    }

    fn world_with(player_at: Option<Vec3>) -> World {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let handle = assets.add(scene_with_teleporter());
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        if let Some(at) = player_at {
            world.spawn((Player, Transform::from_translation(at)));
        }
        world
    }

    #[test]
    fn touching_a_teleporter_queues_a_transition() {
        let mut world = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();
        let pending = world.resource::<PendingTeleport>();
        assert_eq!(pending.target, "scenes/room2.scene");
        assert_eq!(pending.arrival, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn standing_clear_does_not_teleport() {
        let mut world = world_with(Some(Vec3::new(0.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn without_a_player_nothing_triggers() {
        let mut world = world_with(None);
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn a_pending_transition_absorbs_re_triggers() {
        let mut world = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        world.insert_resource(PendingTeleport {
            target: "scenes/already-going.scene".into(),
            arrival: Vec2::ZERO,
        });
        world.run_system_once(check_teleporters).unwrap();
        assert_eq!(
            world.resource::<PendingTeleport>().target,
            "scenes/already-going.scene"
        );
    }

    #[test]
    fn editing_pauses_teleports() {
        let mut world = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        let mut editor = EditorState::default();
        editor.open = true;
        world.insert_resource(editor);
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }
}
