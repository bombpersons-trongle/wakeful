//! Teleporters: trigger rects in the scene that load another scene when
//! the player touches one.

use bevy::prelude::*;

use crate::editor::EditorState;
use crate::scene::Scene;
use crate::{CurrentScene, PendingTeleport, Player, TeleporterArmed};

/// Runs after `move_player`: when the player's position lands inside a
/// teleporter's trigger rect, queues a scene transition. Editing pauses
/// the trigger like movement, and a pending transition absorbs
/// re-triggers until `transition_scene` consumes it.
///
/// Triggers re-arm: a teleporter fires only while armed, disarms when it
/// fires, and rearms once the player leaves it — so arriving inside a
/// region (its spawn point) doesn't chain-teleport until the player
/// walks out and back in.
pub fn check_teleporters(
    mut commands: Commands,
    mut armed: Option<ResMut<TeleporterArmed>>,
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
    let Some(armed) = armed.as_deref_mut() else {
        return;
    };
    let at = transform.translation.xz();

    // Everything the player has left rearms; the first armed trigger
    // under the player fires and disarms until they leave and re-enter.
    let mut fired = None;
    for (index, teleporter) in scene.teleporters.iter().enumerate() {
        // The editor can add teleporters mid-session; flags for them
        // start armed.
        if index >= armed.0.len() {
            armed.0.push(true);
        }
        if teleporter.contains(at.x, at.y) {
            if armed.0[index] && fired.is_none() {
                fired = Some(teleporter);
                armed.0[index] = false;
            }
        } else {
            armed.0[index] = true;
        }
    }
    let Some(teleporter) = fired else {
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
            actors: Vec::new(),
        }
    }

    fn world_with(player_at: Option<Vec3>) -> (World, Option<Entity>) {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let handle = assets.add(scene_with_teleporter());
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        world.insert_resource(TeleporterArmed(vec![true]));
        let player =
            player_at.map(|at| world.spawn((Player, Transform::from_translation(at))).id());
        (world, player)
    }

    fn set_player_at(world: &mut World, player: Entity, at: Vec2) {
        world.get_mut::<Transform>(player).unwrap().translation = Vec3::new(at.x, 0.9, at.y);
    }

    #[test]
    fn touching_a_teleporter_queues_a_transition() {
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();
        let pending = world.resource::<PendingTeleport>();
        assert_eq!(pending.target, "scenes/room2.scene");
        assert_eq!(pending.arrival, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn standing_clear_does_not_teleport() {
        let (mut world, _) = world_with(Some(Vec3::new(0.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn without_a_player_nothing_triggers() {
        let (mut world, player) = world_with(None);
        assert!(player.is_none());
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn a_pending_transition_absorbs_re_triggers() {
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
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
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        let mut editor = EditorState::default();
        editor.open = true;
        world.insert_resource(editor);
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn firing_disarms_until_the_player_leaves_and_reenters() {
        let (mut world, player) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        let player = player.unwrap();
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_some());
        assert!(!world.resource::<TeleporterArmed>().0[0]);

        // The transition consumes the pending teleport; the player is
        // still inside the region, so it stays quiet.
        world.remove_resource::<PendingTeleport>();
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_none());

        // Leaving rearms the trigger without firing it.
        set_player_at(&mut world, player, Vec2::new(0.0, 0.0));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.resource::<TeleporterArmed>().0[0]);
        assert!(world.get_resource::<PendingTeleport>().is_none());

        // Re-entering fires it again.
        set_player_at(&mut world, player, Vec2::new(2.0, 0.0));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingTeleport>().is_some());
    }

    #[test]
    fn overlapping_triggers_fire_the_first_armed_one() {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let scene = Scene {
            teleporters: vec![
                Teleporter {
                    position: [0.0, 0.0],
                    size: [2.0, 2.0],
                    target: "scenes/first.scene".into(),
                    arrival: [0.0, 0.0],
                },
                Teleporter {
                    position: [0.0, 0.0],
                    size: [4.0, 4.0],
                    target: "scenes/second.scene".into(),
                    arrival: [0.0, 0.0],
                },
            ],
            ..scene_with_teleporter()
        };
        let handle = assets.add(scene);
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        world.insert_resource(TeleporterArmed(vec![false, true]));
        world.spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, 0.9, 0.0)),
        ));
        world.run_system_once(check_teleporters).unwrap();
        // The first (disarmed) one is skipped; the second fires.
        assert_eq!(
            world.resource::<PendingTeleport>().target,
            "scenes/second.scene"
        );
    }
}
