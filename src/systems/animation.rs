//! Character animation: glTF clips wired to the standard chibi rig
//! convention and driven by gameplay state.
//!
//! Character models ship a standard clip set (`idle`, `walk`, `run`,
//! plus one-shot emotes) under standard joint node names — see
//! `tools/generate_character.py`, the reference implementation. The
//! glTF loader spawns an `AnimationPlayer` inside a loaded model's
//! scene; this module finds it, builds the `AnimationGraph`, and runs
//! one driver: one-shot emotes win over locomotion, locomotion
//! (`walk`/`run`) wins over `idle`, crossfaded. Models without the
//! standard clips stay static.

use std::time::Duration;

use bevy::animation::prelude::*;
use bevy::gltf::Gltf;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// The standard clip names, in the order the graph is built. Locomotion
/// clips loop; emotes are one-shots.
const CLIPS: [&str; 6] = ["idle", "walk", "run", "pick_up", "shrug", "wave"];

/// How long a gait switch crossfades.
const CROSSFADE_MS: u64 = 200;

/// The gait a character is in this tick, written by the movement
/// systems and read by the driver.
#[derive(Component, Debug, Default, PartialEq)]
pub(crate) struct Locomotion {
    pub(crate) moving: bool,
    pub(crate) running: bool,
}

/// Queued on a character when its model attaches; resolved once the
/// loaded glTF scene spawns its AnimationPlayer.
#[derive(Component)]
pub(crate) struct PendingAnimations(pub(crate) Handle<Gltf>);

/// A character's standard clips, resolved against its model.
#[derive(Component)]
pub(crate) struct CharacterAnimations {
    clips: HashMap<&'static str, AnimationNodeIndex>,
}

/// Per-character driver state: the AnimationPlayer inside the model
/// plus the one-shot emote currently selected.
#[derive(Component, Default)]
pub(crate) struct CharacterAnimator {
    player: Option<Entity>,
    emote: Option<AnimationNodeIndex>,
}

/// A one-shot emote an actor script asked for this tick, taken by the
/// driver.
#[derive(Component, Debug, Default)]
pub(crate) struct EmoteRequest(pub(crate) Option<String>);

/// A driver decision for one tick, before touching the ECS.
#[derive(Debug, PartialEq)]
enum Step {
    /// Start (or restart) this emote instantly.
    StartEmote(AnimationNodeIndex),
    /// Keep the current emote running.
    HoldEmote,
    /// Crossfade to this locomotion clip if not already playing it.
    Locomotion(AnimationNodeIndex),
    /// Nothing to play (no suitable clip).
    None,
}

/// The clip for the base gait, with graceful fallbacks: run falls back
/// to walk, walk to idle.
fn base_clip(
    clips: &HashMap<&'static str, AnimationNodeIndex>,
    moving: bool,
    running: bool,
) -> Option<AnimationNodeIndex> {
    if !moving {
        return clips.get("idle").copied();
    }
    let gait = if running { "run" } else { "walk" };
    clips
        .get(gait)
        .or_else(|| clips.get("walk"))
        .or_else(|| clips.get("idle"))
        .copied()
}

/// The full state-machine decision: emote requests select a one-shot
/// that owns the character until it finishes, then locomotion resumes.
fn next_step(
    emote: &mut Option<AnimationNodeIndex>,
    request: Option<AnimationNodeIndex>,
    moving: bool,
    running: bool,
    clips: &HashMap<&'static str, AnimationNodeIndex>,
    emote_status: Option<bool>,
) -> Step {
    // A request re-selects the one-shot; the same, still-running emote
    // is held rather than restarted (scripts may ask every tick).
    if let Some(request) = request {
        if *emote == Some(request) && emote_status == Some(false) {
            return Step::HoldEmote;
        }
        *emote = Some(request);
        return Step::StartEmote(request);
    }
    if let Some(_current) = *emote {
        match emote_status {
            Some(false) => return Step::HoldEmote,
            _ => *emote = None,
        }
    }
    base_clip(clips, moving, running).map_or(Step::None, Step::Locomotion)
}

/// Builds each character's AnimationGraph once its model's scene is
/// live: finds the descendant AnimationPlayer the glTF loader spawned,
/// wires the graph + transitions onto it, and publishes the clip table
/// on the character. Models without standard clips stay static.
pub(crate) fn resolve_pending_animations(
    mut commands: Commands,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    children: Query<&Children>,
    anim_players: Query<Entity, With<AnimationPlayer>>,
    pending: Query<(Entity, &PendingAnimations)>,
) {
    for (entity, pending) in &pending {
        let Some(gltf) = gltfs.get(&pending.0) else {
            continue;
        };
        let Some(player) = find_animation_player(entity, &children, &anim_players) else {
            continue;
        };
        // Standard clips in graph order; models without any stay static.
        let handles: Vec<_> = CLIPS
            .iter()
            .filter_map(|name| gltf.named_animations.get(*name).cloned())
            .collect();
        if handles.is_empty() {
            commands.entity(entity).remove::<PendingAnimations>();
            continue;
        }
        let (graph, nodes) = AnimationGraph::from_clips(handles);
        let mut remaining = nodes.into_iter();
        let clips: HashMap<&'static str, AnimationNodeIndex> = CLIPS
            .iter()
            .filter(|name| gltf.named_animations.contains_key(**name))
            .map(|name| (*name, remaining.next().expect("one node per clip")))
            .collect();
        let graph = graphs.add(graph);
        commands.entity(entity).insert((
            CharacterAnimations { clips },
            CharacterAnimator {
                player: Some(player),
                emote: None,
            },
        ));
        commands
            .entity(player)
            .insert((AnimationGraphHandle(graph), AnimationTransitions::new()));
        commands.entity(entity).remove::<PendingAnimations>();
    }
}

fn find_animation_player(
    entity: Entity,
    children: &Query<&Children>,
    players: &Query<Entity, With<AnimationPlayer>>,
) -> Option<Entity> {
    if players.contains(entity) {
        return Some(entity);
    }
    children
        .get(entity)
        .ok()?
        .iter()
        .find_map(|child| find_animation_player(child, children, players))
}

/// Picks each character's clip from its locomotion and emote state and
/// crossfades to it. An unknown emote name warns once and is dropped —
/// the engine can't know a model's clip names ahead of time.
#[allow(clippy::type_complexity)]
pub(crate) fn run_character_animations(
    mut characters: Query<(
        Entity,
        &Locomotion,
        Option<&mut EmoteRequest>,
        &CharacterAnimations,
        &mut CharacterAnimator,
    )>,
    mut transitions: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    for (_entity, locomotion, request, anims, mut animator) in &mut characters {
        let request = request.and_then(|mut request| request.0.take());
        let request = request.map(|name| match anims.clips.get(name.as_str()).copied() {
            Some(idx) => Some(idx),
            None => {
                warn!("character has no clip '{name}'; ignoring emote");
                None
            }
        });
        let Some(player_entity) = animator.player else {
            continue;
        };
        let Ok((mut player, mut transitions)) = transitions.get_mut(player_entity) else {
            continue;
        };
        let status = animator
            .emote
            .map(|emote| player.animation(emote).is_none_or(|a| a.is_finished()));
        match next_step(
            &mut animator.emote,
            request.flatten(),
            locomotion.moving,
            locomotion.running,
            &anims.clips,
            status,
        ) {
            Step::StartEmote(emote) => {
                transitions.play(&mut player, emote, Duration::ZERO);
            }
            Step::HoldEmote => {}
            Step::Locomotion(clip) => {
                if !player.is_playing_animation(clip) {
                    transitions
                        .play(&mut player, clip, Duration::from_millis(CROSSFADE_MS))
                        .repeat();
                }
            }
            Step::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::animation::AnimationClip;
    use bevy::asset::Handle;
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn clips_with(names: &[&str]) -> HashMap<&'static str, AnimationNodeIndex> {
        let mut clips = HashMap::default();
        for (i, name) in CLIPS.iter().enumerate() {
            if names.contains(name) {
                clips.insert(*name, AnimationNodeIndex::new(i));
            }
        }
        clips
    }

    #[test]
    fn the_base_gait_walks_and_falls_back() {
        let clips = clips_with(&["idle", "walk", "run"]);
        let idle = clips["idle"];
        let walk = clips["walk"];
        let run = clips["run"];
        assert_eq!(base_clip(&clips, false, false), Some(idle));
        assert_eq!(base_clip(&clips, true, false), Some(walk));
        assert_eq!(base_clip(&clips, true, true), Some(run));

        // Without run, a running gait degrades to walk; without any
        // locomotion clip at all, nothing plays.
        let idle_walk = clips_with(&["idle", "walk"]);
        assert_eq!(base_clip(&idle_walk, true, true), Some(idle_walk["walk"]));
        let empty = clips_with(&[]);
        assert_eq!(base_clip(&empty, true, true), None);
    }

    #[test]
    fn an_emote_owns_the_character_until_it_finishes() {
        let clips = clips_with(&["idle", "walk", "shrug"]);
        let shrug = clips["shrug"];
        let mut emote = None;

        // Requested: starts instantly.
        assert_eq!(
            next_step(&mut emote, Some(shrug), false, false, &clips, None),
            Step::StartEmote(shrug)
        );
        // Re-requested mid-play every tick: held, not restarted.
        assert_eq!(
            next_step(&mut emote, Some(shrug), false, false, &clips, Some(false)),
            Step::HoldEmote
        );
        // Still running with no new request: held.
        assert_eq!(
            next_step(&mut emote, None, true, true, &clips, Some(false)),
            Step::HoldEmote
        );
        // Finished: released into the current gait.
        assert_eq!(
            next_step(&mut emote, None, true, false, &clips, Some(true)),
            Step::Locomotion(clips["walk"])
        );
        assert_eq!(emote, None);
        // Re-requested after finishing: plays again.
        assert_eq!(
            next_step(&mut emote, Some(shrug), false, false, &clips, Some(true)),
            Step::StartEmote(shrug)
        );
    }

    #[test]
    fn an_unknown_emote_request_is_dropped() {
        // The system resolves names before the state machine; a miss
        // reaches it as no request, and locomotion carries on.
        let clips = clips_with(&["idle"]);
        let mut emote = None;
        assert_eq!(
            next_step(&mut emote, None, false, false, &clips, None),
            Step::Locomotion(clips["idle"])
        );
    }

    fn gltf_with(named: &[&str]) -> (Assets<AnimationClip>, Gltf) {
        let mut clips = Assets::default();
        let named_animations: bevy::platform::collections::HashMap<
            Box<str>,
            Handle<AnimationClip>,
        > = named
            .iter()
            .map(|name| ((*name).into(), clips.add(AnimationClip::default())))
            .collect();
        (
            clips,
            Gltf {
                scenes: vec![Handle::default()],
                named_scenes: HashMap::default(),
                meshes: vec![],
                named_meshes: HashMap::default(),
                materials: vec![],
                named_materials: HashMap::default(),
                nodes: vec![],
                named_nodes: HashMap::default(),
                skins: vec![],
                named_skins: HashMap::default(),
                default_scene: Some(Handle::default()),
                animations: vec![],
                named_animations,
                source: None,
            },
        )
    }

    fn character_with_model(world: &mut World, gltf_handle: Handle<Gltf>) -> Entity {
        let root = world.spawn_empty().id();
        let inner = world.spawn_empty().id();
        let player = world.spawn(AnimationPlayer::default()).id();
        world.entity_mut(root).add_child(inner);
        world.entity_mut(inner).add_child(player);
        world
            .entity_mut(root)
            .insert(PendingAnimations(gltf_handle));
        root
    }

    #[test]
    fn a_model_with_standard_clips_gets_a_wired_player() {
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (clip_assets, gltf) = gltf_with(&["idle", "walk", "run", "shrug"]);
        world.insert_resource(clip_assets);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        let anims = world.get::<CharacterAnimations>(root).unwrap();
        assert_eq!(anims.clips.len(), 4);
        let animator = world.get::<CharacterAnimator>(root).unwrap();
        let player_entity = animator.player.unwrap();
        assert!(world.get::<AnimationGraphHandle>(player_entity).is_some());
        assert!(world.get::<AnimationTransitions>(player_entity).is_some());
        // Already resolved: nothing re-runs.
        assert!(world.get::<PendingAnimations>(root).is_none());
    }

    #[test]
    fn a_model_without_standard_clips_stays_static() {
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&["dance"]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        assert!(world.get::<CharacterAnimations>(root).is_none());
        assert!(world.get::<PendingAnimations>(root).is_none());
    }

    #[test]
    fn the_driver_plays_the_gait_and_starts_requested_emotes() {
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&["idle", "walk", "run", "shrug"]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        let anims = world.get::<CharacterAnimations>(root).unwrap();
        let walk = anims.clips["walk"];
        let shrug = anims.clips["shrug"];
        world.entity_mut(root).insert(Locomotion {
            moving: true,
            running: false,
        });

        // Walking plays the walk clip.
        world.run_system_once(run_character_animations).unwrap();
        let player = world
            .get::<CharacterAnimator>(root)
            .unwrap()
            .player
            .unwrap();
        assert!(
            world
                .get::<AnimationPlayer>(player)
                .unwrap()
                .is_playing_animation(walk)
        );

        // Then an emote request wins over walking.
        world
            .entity_mut(root)
            .insert(EmoteRequest(Some("shrug".into())));
        world.run_system_once(run_character_animations).unwrap();
        let animator = world.get::<CharacterAnimator>(root).unwrap();
        assert_eq!(animator.emote, Some(shrug));
        assert!(
            world
                .get::<AnimationPlayer>(player)
                .unwrap()
                .is_playing_animation(shrug)
        );

        // Without a finish, walking never interrupts the emote; the
        // request is gone but the one-shot still owns the character
        // (time doesn't advance under run_system_once, so the full
        // release cycle is covered by the next_step tests).
        world.entity_mut(root).remove::<EmoteRequest>();
        world.run_system_once(run_character_animations).unwrap();
        assert!(
            world
                .get::<AnimationPlayer>(player)
                .unwrap()
                .is_playing_animation(shrug)
        );
    }
}
