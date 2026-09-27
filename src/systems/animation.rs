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

use crate::systems::actor::ScriptTicked;

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
    clips: HashMap<Box<str>, AnimationNodeIndex>,
}

/// Per-character driver state: the AnimationPlayer inside the model
/// plus the one-shot emote and held pose currently selected. `root` is
/// the spawned model's top entity, kept hidden until the character's
/// first driven step so a re-entered scene never flashes its bind pose
/// (`appeared` latches the unhide).
#[derive(Component, Default)]
pub(crate) struct CharacterAnimator {
    player: Option<Entity>,
    emote: Option<AnimationNodeIndex>,
    pose: Option<AnimationNodeIndex>,
    root: Option<Entity>,
    appeared: bool,
}

#[cfg(debug_assertions)]
impl CharacterAnimator {
    pub(crate) fn debug(&self) -> String {
        format!(
            "player={:?} emote={:?} pose={:?}",
            self.player.map(|e| e.index()),
            self.emote.map(|i| i.index()),
            self.pose.map(|i| i.index())
        )
    }

    pub(crate) fn debug_player(&self) -> Option<Entity> {
        self.player
    }
}

#[cfg(debug_assertions)]
impl CharacterAnimations {
    pub(crate) fn debug_clips(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .clips
            .iter()
            .map(|(name, idx)| format!("{}#{}", name, idx.index()))
            .collect();
        names.sort();
        names
    }
}

/// A one-shot emote an actor script asked for this tick, taken by the
/// driver.
/// What an actor's script asked the animation driver to show this
/// tick: `emote` plays a clip; `pose` snaps to its final frame and
/// holds (how props stay open without re-animating).
#[derive(Component, Debug, Default)]
pub(crate) struct EmoteRequest {
    pub(crate) emote: Option<String>,
    pub(crate) pose: Option<String>,
}

/// A driver decision for one tick, before touching the ECS.
#[derive(Debug, PartialEq)]
enum Step {
    /// Start (or restart) this emote instantly.
    StartEmote(AnimationNodeIndex),
    /// Keep the current emote running.
    HoldEmote,
    /// Keep holding the applied pose (no work per tick unless re-asserted).
    HoldPose,
    /// Snap to this clip's final frame and hold it (no animation).
    Pose(AnimationNodeIndex),
    /// Crossfade to this locomotion clip if not already playing it.
    Locomotion(AnimationNodeIndex),
    /// Nothing to play (no suitable clip).
    None,
}

/// The clip for the base gait, with graceful fallbacks: run falls back
/// to walk, walk to idle.
fn base_clip(
    clips: &HashMap<Box<str>, AnimationNodeIndex>,
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
#[allow(clippy::too_many_arguments)]
fn next_step(
    emote: &mut Option<AnimationNodeIndex>,
    pose: &mut Option<AnimationNodeIndex>,
    emote_request: Option<AnimationNodeIndex>,
    pose_request: Option<AnimationNodeIndex>,
    moving: bool,
    running: bool,
    clips: &HashMap<Box<str>, AnimationNodeIndex>,
    emote_status: Option<bool>,
) -> Step {
    // A request re-selects the one-shot; the same emote is held rather
    // than restarted, even after finishing — scripts are level-
    // triggered, so a repeated request means "keep this", and a prop
    // model re-announcing its emote every tick stays open instead of
    // re-animating forever. To replay, stop requesting for a tick.
    // A playing emote owns the character; pose requests wait until it
    // finishes. A repeated emote request is held, not restarted —
    // scripts are level-triggered; pulse the request to replay.
    if let Some(request) = emote_request {
        if *emote == Some(request) {
            return Step::HoldEmote;
        }
        *emote = Some(request);
        *pose = None;
        return Step::StartEmote(request);
    }
    if let Some(_current) = *emote {
        match emote_status {
            Some(false) => return Step::HoldEmote,
            // A finished emote on a prop-like model (no locomotion
            // clips and nothing posed or requested) holds its final
            // pose instead of snapping back to bind.
            _ if base_clip(clips, moving, running).is_none() && pose_request.is_none() => {
                return Step::HoldEmote;
            }
            _ => *emote = None,
        }
    }
    if let Some(request) = pose_request {
        if *pose != Some(request) {
            *pose = Some(request);
            return Step::Pose(request);
        }
        return Step::HoldPose;
    }
    // An applied pose keeps holding statically.
    if pose.is_some() {
        return Step::HoldPose;
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
        let Some((player, model_root)) =
            find_animation_player_with_root(entity, &children, &anim_players)
        else {
            continue;
        };
        // Every named clip the model ships — standard names first so
        // the locomotion fallbacks keep stable order, extras (a
        // chest's `open`) ride along for emotes. Models without any
        // animations stay static.
        let mut names: Vec<&str> = CLIPS
            .iter()
            .copied()
            .filter(|name| gltf.named_animations.contains_key(*name))
            .collect();
        for name in gltf.named_animations.keys() {
            if !names.contains(&name.as_ref()) {
                names.push(name.as_ref());
            }
        }
        let handles: Vec<_> = names
            .iter()
            .filter_map(|name| gltf.named_animations.get(*name).cloned())
            .collect();
        if handles.is_empty() {
            commands.entity(entity).remove::<PendingAnimations>();
            continue;
        }
        let (graph, nodes) = AnimationGraph::from_clips(handles);
        let clips: HashMap<Box<str>, AnimationNodeIndex> = names
            .iter()
            .zip(nodes)
            .map(|(name, node)| (Box::<str>::from(*name), node))
            .collect();
        let graph = graphs.add(graph);
        // The model spawns hidden and is revealed by the driver's
        // first driven step (or the actor's first script tick), so a
        // returning chest is seen open rather than flashing closed.
        commands.entity(entity).insert((
            CharacterAnimations { clips },
            CharacterAnimator {
                player: Some(player),
                emote: None,
                pose: None,
                root: model_root,
                appeared: false,
            },
        ));
        if let Some(root) = model_root {
            commands.entity(root).insert(Visibility::Hidden);
        }
        commands
            .entity(player)
            .insert((AnimationGraphHandle(graph), AnimationTransitions::new()));
        commands.entity(entity).remove::<PendingAnimations>();
    }
}

/// Finds the animation player inside a spawned model and the model's
/// top entity (the direct child the scene was attached under).
fn find_animation_player_with_root(
    entity: Entity,
    children: &Query<&Children>,
    players: &Query<Entity, With<AnimationPlayer>>,
) -> Option<(Entity, Option<Entity>)> {
    if players.contains(entity) {
        return Some((entity, None));
    }
    for child in children.get(entity).ok()?.iter() {
        if let Some((player, _)) = find_animation_player_with_root(child, children, players) {
            return Some((player, Some(child)));
        }
    }
    None
}

/// Picks each character's clip from its locomotion and emote state and
/// crossfades to it. An unknown emote name warns once and is dropped —
/// the engine can't know a model's clip names ahead of time.
#[allow(clippy::type_complexity)]
pub(crate) fn run_character_animations(
    mut commands: Commands,
    mut characters: Query<(
        Entity,
        &Locomotion,
        Option<&mut EmoteRequest>,
        &CharacterAnimations,
        &mut CharacterAnimator,
        Option<&ScriptTicked>,
    )>,
    mut transitions: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    for (_entity, locomotion, request, anims, mut animator, ticked) in &mut characters {
        let (emote_name, pose_name) = match request {
            Some(mut request) => {
                let emote = request.emote.take();
                let pose = request.pose.take();
                (emote, pose)
            }
            None => (None, None),
        };
        let resolve = |name: Option<String>| -> Option<AnimationNodeIndex> {
            name.and_then(|name| match anims.clips.get(name.as_str()).copied() {
                Some(idx) => Some(idx),
                None => {
                    warn!("character has no clip '{name}'; ignoring request");
                    None
                }
            })
        };
        let emote_request = resolve(emote_name);
        let pose_request = resolve(pose_name);
        let Some(player_entity) = animator.player else {
            continue;
        };
        let Ok((mut player, mut transitions)) = transitions.get_mut(player_entity) else {
            continue;
        };
        let status = animator
            .emote
            .map(|emote| player.animation(emote).is_none_or(|a| a.is_finished()));
        let mut emote = animator.emote;
        let mut pose = animator.pose;
        let step = next_step(
            &mut emote,
            &mut pose,
            emote_request,
            pose_request,
            locomotion.moving,
            locomotion.running,
            &anims.clips,
            status,
        );
        animator.emote = emote;
        animator.pose = pose;
        match step {
            Step::StartEmote(emote) => {
                transitions.play(&mut player, emote, Duration::ZERO);
            }
            Step::HoldEmote => {}
            Step::Pose(clip) => {
                // Snap to the clip's final frame and hold it: no
                // swing, the prop just is open. Raw player start, NOT
                // transitions: managed clips get stopped when they
                // finish (snap back to bind), a paused raw one just
                // keeps its pose. next_step only emits Pose on change,
                // so this runs once per state change.
                let animation = player.start(clip);
                animation.set_seek_time(f32::MAX).pause();
            }
            Step::HoldPose => {
                // Paused animations hold their pose; nothing to do
                // while the same clip is already snapped and paused.
            }
            Step::Locomotion(clip) => {
                if !player.is_playing_animation(clip) {
                    transitions
                        .play(&mut player, clip, Duration::from_millis(CROSSFADE_MS))
                        .repeat();
                }
            }
            Step::None => {}
        }
        // Reveal the model once it has something true to show: the
        // first driven step applied a pose or gait, or the actor's
        // script has ticked (a closed chest has nothing to animate but
        // must be seen). Until then the model stays hidden so a
        // re-entered scene never flashes its bind pose.
        if !animator.appeared
            && let Some(root) = animator.root
            && (ticked.is_some() || !matches!(step, Step::None | Step::HoldEmote))
        {
            animator.appeared = true;
            commands.entity(root).insert(Visibility::Visible);
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::animation::AnimationClip;
    use bevy::asset::Handle;
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn clips_with(names: &[&str]) -> HashMap<Box<str>, AnimationNodeIndex> {
        let mut clips = HashMap::default();
        // Standard names keep their resolve order; non-standard names
        // (a chest's `open`) get indices after the standard set.
        for (i, name) in CLIPS.iter().enumerate() {
            if names.contains(name) {
                clips.insert((*name).into(), AnimationNodeIndex::new(i));
            }
        }
        for (offset, name) in names.iter().filter(|n| !CLIPS.contains(n)).enumerate() {
            clips.insert(
                (*name).into(),
                AnimationNodeIndex::new(CLIPS.len() + offset),
            );
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
        let mut pose = None;

        // Requested: starts instantly.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(shrug),
                None,
                false,
                false,
                &clips,
                None
            ),
            Step::StartEmote(shrug)
        );
        // Re-requested mid-play every tick: held, not restarted.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(shrug),
                None,
                false,
                false,
                &clips,
                Some(false)
            ),
            Step::HoldEmote
        );
        // Still running with no new request: held.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                None,
                true,
                true,
                &clips,
                Some(false)
            ),
            Step::HoldEmote
        );
        // Finished: released into the current gait.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                None,
                true,
                false,
                &clips,
                Some(true)
            ),
            Step::Locomotion(clips["walk"])
        );
        assert_eq!(emote, None);
        // Re-requested after the release: it starts fresh (the state
        // is gone), so a repeat performance just works.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(shrug),
                None,
                false,
                false,
                &clips,
                Some(true)
            ),
            Step::StartEmote(shrug)
        );
    }

    #[test]
    fn a_prop_without_locomotion_clips_holds_its_finished_emote() {
        // A chest ships only `open`: when the emote finishes there is
        // no gait and no pose to fall back to, so the final pose is
        // held instead of snapping back to bind.
        let clips = clips_with(&["open"]);
        let open = clips["open"];
        let mut emote = None;
        let mut pose = None;

        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(open),
                None,
                false,
                false,
                &clips,
                None
            ),
            Step::StartEmote(open)
        );
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                None,
                false,
                false,
                &clips,
                Some(false)
            ),
            Step::HoldEmote
        );
        // Finished: still held without a new request.
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                None,
                false,
                false,
                &clips,
                Some(true)
            ),
            Step::HoldEmote
        );
        // A repeated request is held (level-triggered scripts emote or
        // pose every tick; nobody wants a re-swing loop).
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(open),
                None,
                false,
                false,
                &clips,
                Some(true)
            ),
            Step::HoldEmote
        );
        assert_eq!(emote, Some(open));
    }

    #[test]
    fn a_pose_snaps_open_once_the_emote_finishes() {
        // The chest flow: the emote swings the lid; a simultaneous
        // pose request waits for it; once it finishes the pose takes
        // over and keeps holding.
        let clips = clips_with(&["open"]);
        let open = clips["open"];
        let mut emote = None;
        let mut pose = None;

        // The emote starts first (the chest's opening tick).
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(open),
                None,
                false,
                false,
                &clips,
                None
            ),
            Step::StartEmote(open)
        );

        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                Some(open),
                Some(open),
                false,
                false,
                &clips,
                Some(false)
            ),
            Step::HoldEmote
        );
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                Some(open),
                false,
                false,
                &clips,
                Some(true)
            ),
            Step::Pose(open)
        );
        assert_eq!(pose, Some(open));
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                Some(open),
                false,
                false,
                &clips,
                None
            ),
            Step::HoldPose
        );
    }

    #[test]
    fn a_pose_on_reload_is_instant_from_a_fresh_state() {
        // Scene reload: fresh driver state, but the script poses on the
        // very first tick — no swing, just open.
        let clips = clips_with(&["open"]);
        let open = clips["open"];
        let mut emote = None;
        let mut pose = None;
        assert_eq!(
            next_step(
                &mut emote,
                &mut pose,
                None,
                Some(open),
                false,
                false,
                &clips,
                None
            ),
            Step::Pose(open)
        );
    }

    #[test]
    fn an_unknown_emote_request_is_dropped() {
        // The system resolves names before the state machine; a miss
        // reaches it as no request, and locomotion carries on.
        let clips = clips_with(&["idle"]);
        let mut emote = None;
        let mut pose = None;
        assert_eq!(
            next_step(
                &mut emote, &mut pose, None, None, false, false, &clips, None
            ),
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
    fn a_model_without_any_clips_stays_static() {
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&[]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        assert!(world.get::<CharacterAnimations>(root).is_none());
        assert!(world.get::<PendingAnimations>(root).is_none());
    }

    #[test]
    fn a_prop_with_only_nonstandard_clips_gets_wired() {
        // A chest ships just `open`: non-standard, but it must land in
        // the clip table so `emote("open")` resolves.
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&["open"]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        let clips = world.get::<CharacterAnimations>(root).unwrap();
        assert!(
            clips.clips.contains_key(&Box::from("open")),
            "open is wired"
        );
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
        world.entity_mut(root).insert(EmoteRequest {
            emote: Some("shrug".into()),
            pose: None,
        });
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

    #[test]
    fn a_fresh_model_hides_until_its_first_driven_step() {
        // The closed-chest flash: a returning scene spawns the model in
        // bind pose and the script's pose lands a tick later. The model
        // stays hidden until something true is shown, so the chest is
        // revealed already open.
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&["open"]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        let animator = world.get::<CharacterAnimator>(root).unwrap();
        let model_root = animator.root.unwrap();
        let player = animator.player.unwrap();
        assert_eq!(
            world.get::<Visibility>(model_root).copied(),
            Some(Visibility::Hidden),
            "a fresh model hides between spawn and its first driven step"
        );

        // First script tick: an open pose request lands. (Locomotion
        // comes with every spawned character in the game; the driver's
        // query reads it.)
        let open = world.get::<CharacterAnimations>(root).unwrap().clips["open"];
        world
            .entity_mut(root)
            .insert((ScriptTicked, Locomotion::default()));
        world.entity_mut(root).insert(EmoteRequest {
            emote: None,
            pose: Some("open".into()),
        });
        world.run_system_once(run_character_animations).unwrap();

        let animator = world.get::<CharacterAnimator>(root).unwrap();
        assert!(animator.appeared, "the pose revealed the model");
        assert_eq!(
            world.get::<Visibility>(model_root).copied(),
            Some(Visibility::Visible),
        );
        let animation = world
            .get::<AnimationPlayer>(player)
            .unwrap()
            .animation(open);
        assert!(animation.is_some_and(|a| a.is_paused()), "held at its end");
    }

    #[test]
    fn a_model_without_clips_is_never_hidden() {
        let mut world = World::new();
        world.init_resource::<Assets<Gltf>>();
        world.init_resource::<Assets<AnimationGraph>>();
        let (_, gltf) = gltf_with(&[]);
        let mut gltfs = world.resource_mut::<Assets<Gltf>>();
        let handle = gltfs.add(gltf);
        let root = character_with_model(&mut world, handle);

        world.run_system_once(resolve_pending_animations).unwrap();
        world.flush();

        // Nothing was wired and nothing was hidden.
        assert!(world.get::<CharacterAnimations>(root).is_none());
        assert!(world.get::<CharacterAnimator>(root).is_none());
    }
}
