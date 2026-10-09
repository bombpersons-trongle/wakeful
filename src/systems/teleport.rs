//! Teleporters: trigger rects in the scene that load another scene when
//! the player touches one. The jump rides the transition (a fade to
//! black, the scene swap behind the cover, the fade into the new
//! scene) — the swap itself happens at the covered point, when the
//! game state returns to [`crate::game_state::GameState::Scene`] and
//! [`release_warp`] hands the queued warp to `transition_scene`.

use bevy::prelude::*;

use crate::game_state::GameState;
use crate::scene::Scene;
use crate::transition::{Effect, TransitionState};
use crate::{CurrentScene, PendingTeleport, Player, TeleporterArmed};

/// A teleporter the player touched, waiting for the transition's covered
/// point to swap the scene. Distinct from [`PendingTeleport`], which
/// means "swap now": queueing one of those directly would despawn the
/// old scene while the fade was still opening.
#[derive(Resource)]
pub(crate) struct PendingSceneWarp {
    pub(crate) target: String,
    pub(crate) arrival: Vec2,
}

/// `OnEnter(Scene)`: fires at the transition's covered point — hands the
/// queued warp to `transition_scene`, which swaps the scene behind the
/// still-opaque cover.
pub(crate) fn release_warp(
    mut commands: Commands,
    warp: Option<Res<PendingSceneWarp>>,
) {
    let Some(warp) = warp else {
        return;
    };
    commands.insert_resource(PendingTeleport {
        target: warp.target.clone(),
        arrival: warp.arrival,
    });
    commands.remove_resource::<PendingSceneWarp>();
}

/// Runs after `move_player`: when the player's position lands inside a
/// teleporter's trigger rect, queues a scene transition. Editing pauses
/// the trigger like movement, and a pending transition absorbs
/// re-triggers until `transition_scene` consumes it.
///
/// Triggers re-arm: a teleporter fires only while armed, disarms when it
/// fires, and rearms once the player leaves it — so arriving inside a
/// region (its spawn point) doesn't chain-teleport until the player
/// walks out and back in.
#[allow(clippy::too_many_arguments)]
pub fn check_teleporters(
    mut commands: Commands,
    mut armed: Option<ResMut<TeleporterArmed>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    pending: Option<Res<PendingTeleport>>,
    warp: Option<Res<PendingSceneWarp>>,
    mut transition: ResMut<TransitionState>,
    mut next_state: ResMut<NextState<GameState>>,
    players: Query<&Transform, With<Player>>,
) {
    if pending.is_some() || warp.is_some() {
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
        // A teleporter with no flag yet arrived after the scene loaded
        // (an edited file, a hot reload); it starts armed.
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
    // The jump rides the transition: the fade closes over the street,
    // the covered flip returns the game to Scene, and release_warp
    // hands the swap to transition_scene behind the cover.
    if transition.begin(GameState::Scene, Effect::Fade) {
        commands.insert_resource(PendingSceneWarp {
            target: teleporter.target.clone(),
            arrival: teleporter.arrival.into(),
        });
        next_state.set(GameState::Transition);
    } else {
        warn!("teleporter ignored: a transition is already running");
    }
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
            walk_mesh: None,
            teleporters: vec![Teleporter {
                position: [2.0, 0.0],
                size: [2.0, 2.0],
                target: "scenes/room2.scene".into(),
                arrival: [1.0, 2.0],
            }],
            depth_map: None,
            depth_range: 32.0,
            pan: None,
            script: None,
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
        world.init_resource::<TransitionState>();
        world.init_resource::<NextState<GameState>>();
        let player =
            player_at.map(|at| world.spawn((Player, Transform::from_translation(at))).id());
        (world, player)
    }

    fn set_player_at(world: &mut World, player: Entity, at: Vec2) {
        world.get_mut::<Transform>(player).unwrap().translation = Vec3::new(at.x, 0.9, at.y);
    }

    #[test]
    fn touching_a_teleporter_begins_a_warp() {
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();

        // The warp queues for the covered point; the swap signal does
        // not exist yet.
        let warp = world.resource::<PendingSceneWarp>();
        assert_eq!(warp.target, "scenes/room2.scene");
        assert_eq!(warp.arrival, Vec2::new(1.0, 2.0));
        assert!(world.get_resource::<PendingTeleport>().is_none());

        // And the game rides the transition state (the fade is the
        // call site's choice; transition.rs's own tests cover it).
        assert!(matches!(
            *world.resource::<NextState<GameState>>(),
            NextState::Pending(GameState::Transition)
        ));
    }

    #[test]
    fn standing_clear_does_not_teleport() {
        let (mut world, _) = world_with(Some(Vec3::new(0.0, 0.9, 0.0)));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_none());
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn without_a_player_nothing_triggers() {
        let (mut world, player) = world_with(None);
        assert!(player.is_none());
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_none());
    }

    #[test]
    fn a_pending_transition_absorbs_re_triggers() {
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        world.insert_resource(PendingTeleport {
            target: "scenes/already-going.scene".into(),
            arrival: Vec2::ZERO,
        });
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_none());
        assert_eq!(
            world.resource::<PendingTeleport>().target,
            "scenes/already-going.scene"
        );
    }

    #[test]
    fn a_running_transition_absorbs_re_triggers() {
        let (mut world, _) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        // A transition is mid-flight (say, the battle swirl): the
        // teleporter must not derail it.
        world
            .resource_mut::<TransitionState>()
            .begin(GameState::Battle, Effect::Swirl)
            .then_some(())
            .expect("the machine starts idle");
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_none());
    }

    #[test]
    fn release_warp_hands_the_swap_to_transition_scene() {
        let mut world = World::new();
        world.insert_resource(PendingSceneWarp {
            target: "scenes/room2.scene".into(),
            arrival: Vec2::new(1.0, 2.0),
        });

        world.run_system_once(release_warp).unwrap();
        world.flush();

        assert!(world.get_resource::<PendingSceneWarp>().is_none());
        let pending = world.resource::<PendingTeleport>();
        assert_eq!(pending.target, "scenes/room2.scene");
        assert_eq!(pending.arrival, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn release_warp_without_a_warp_is_quiet() {
        let mut world = World::new();
        world.run_system_once(release_warp).unwrap();
        world.flush();
        assert!(world.get_resource::<PendingTeleport>().is_none());
    }

    #[test]
    fn every_shipped_arrival_fits_the_player() {
        // An arrival is where the player is placed with no further
        // correction, and constrain rejects whole moves rather than
        // stepping out, so an arrival whose body overhangs the walk mesh
        // would leave the player stuck there for good.
        for name in crate::scene::SHIPPED_SCENES {
            let scene = crate::scene::read_shipped(name);
            let Some(mesh) = &scene.walk_mesh else {
                continue;
            };
            for (index, teleporter) in scene.teleporters.iter().enumerate() {
                assert!(
                    mesh.contains_circle(
                        teleporter.arrival[0],
                        teleporter.arrival[1],
                        crate::systems::player::PLAYER_RADIUS
                    ),
                    "{name}: teleporter {index} arrives off the walk mesh"
                );
            }
        }
    }

    #[test]
    fn firing_disarms_until_the_player_leaves_and_reenters() {
        let (mut world, player) = world_with(Some(Vec3::new(2.0, 0.9, 0.0)));
        let player = player.unwrap();
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_some());
        assert!(!world.resource::<TeleporterArmed>().0[0]);

        // The covered-point hand-off consumes the warp; the player is
        // still inside the region, so it stays quiet.
        world.remove_resource::<PendingSceneWarp>();
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_none());

        // Leaving rearms the trigger without firing it.
        set_player_at(&mut world, player, Vec2::new(0.0, 0.0));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.resource::<TeleporterArmed>().0[0]);
        assert!(world.get_resource::<PendingTeleport>().is_none());

        // Re-entering fires it again. In the game the first transition
        // has long completed (the machine rests after its reveal); the
        // bare world skips the driver, so reset the machine to idle to
        // stand in for that.
        *world.resource_mut::<TransitionState>() = TransitionState::default();
        set_player_at(&mut world, player, Vec2::new(2.0, 0.0));
        world.run_system_once(check_teleporters).unwrap();
        assert!(world.get_resource::<PendingSceneWarp>().is_some());
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
        world.init_resource::<TransitionState>();
        world.init_resource::<NextState<GameState>>();
        world.spawn((
            Player,
            Transform::from_translation(Vec3::new(0.0, 0.9, 0.0)),
        ));
        world.run_system_once(check_teleporters).unwrap();
        // The first (disarmed) one is skipped; the second fires.
        assert_eq!(
            world.resource::<PendingSceneWarp>().target,
            "scenes/second.scene"
        );
    }
}
