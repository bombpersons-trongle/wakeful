//! The battle framework: staging, turn sequencing, and the script
//! surface that builds battles.
//!
//! The engine owns almost nothing about a battle's *content*: a
//! participant is an identity, a staged transform, a `time_until_act`
//! value, and an opaque data bag (HP/MP are conventions the scripts
//! keep in the bag, not engine fields). Every participant has a
//! **brain** — a script answering "your turn: what do you do?" — and
//! actions are named handlers registered by the battle script. The
//! same machinery drives menu-driven heroes (the shipped player brain)
//! and AI enemies, so the engine never distinguishes sides.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::state::state::NextState;
use rhai::{Dynamic, Engine, Scope};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::game_state::GameState;
use crate::input::InputManager;
use crate::transition::{Effect, TransitionState};
use crate::systems::animation::Locomotion;
use crate::systems::camera::{BattleCamera, SceneCamera};
use crate::world_state::WorldState;

use crate::systems::actor::ActorModel;
use crate::systems::animation::EmoteRequest;
use crate::systems::ui::{UiApi, UiRequest};

// ---------------------------------------------------------------- script state

/// A brain reference: a script file loaded per participant, or a named
/// function in the battle script's own AST (inline brains coordinate
/// bespoke fights).
pub(crate) enum Brain {
    File {
        script: crate::scripts::ActorScript,
    },
    Inline {
        source: crate::scripts::ScriptSource,
        fn_name: String,
    },
}

/// A named action handler: a function in the battle script that
/// registered it; called with the actor's id, returns the recovery
/// time.
#[derive(Clone)]
pub(crate) struct ActionHandler {
    pub(crate) source: crate::scripts::ScriptSource,
    pub(crate) fn_name: String,
}

/// A participant as the battle script defines it.
#[derive(Clone)]
pub(crate) struct ParticipantDef {
    pub(crate) id: String,
    pub(crate) model: String,
    pub(crate) position: Vec3,
    pub(crate) facing_degrees: f32,
    pub(crate) brain: Option<String>,
    pub(crate) time_until_act: f32,
    pub(crate) bag: rhai::Map,
}

/// `start_battle` arguments, verbatim from the script.
#[derive(Clone)]
pub(crate) struct StartBattle {
    pub(crate) arena: String,
    /// Camera position and look-at target, offsets from the arena
    /// origin (the engine adds the stage offset).
    pub(crate) camera_pos: Vec3,
    pub(crate) camera_look: Vec3,
    pub(crate) participants: Vec<ParticipantDef>,
}

/// Everything the scripts queued before the state transition fired:
/// the battle definition plus the actions and brains registered for
/// it. Consumed by `OnEnter(Battle)`.
#[derive(Resource, Default)]
pub(crate) struct PendingBattleStart {
    pub(crate) def: Option<StartBattle>,
    pub(crate) actions: BTreeMap<String, ActionHandler>,
    pub(crate) brains: Vec<(String, Brain)>,
}

/// What a script asked the battle engine to do.
pub(crate) enum BattleRequest {
    Start(StartBattle),
    /// The result string is for the script's own use after return.
    End {
        result: String,
    },
    Action {
        name: String,
        handler: ActionHandler,
    },
    Brain {
        participant: String,
        brain: Brain,
    },
    Emote {
        participant: String,
        clip: String,
    },
    /// Free-camera flight deltas: yaw and pitch in radians, dolly in
    /// world units along the look direction.
    CameraMove {
        yaw: f32,
        pitch: f32,
        dolly: f32,
    },
}

/// A participant as scripts see it: engine fields plus the bag merged
/// in. Brains and the UI read this.
pub(crate) fn participant_map(
    id: &str,
    position: Vec3,
    time_until_act: f32,
    bag: &rhai::Map,
) -> rhai::Map {
    let mut map = rhai::Map::new();
    map.insert("id".into(), Dynamic::from(id.to_string()));
    map.insert("x".into(), Dynamic::from(position.x as f64));
    map.insert("y".into(), Dynamic::from(position.y as f64));
    map.insert("z".into(), Dynamic::from(position.z as f64));
    map.insert(
        "time_until_act".into(),
        Dynamic::from(time_until_act as f64),
    );
    for (key, value) in bag {
        map.insert(key.clone(), value.clone());
    }
    map
}

/// The script-facing half of the battle framework, shared with every
/// compiled script engine through host functions.
#[derive(Clone, Default, Resource)]
pub struct BattleHandle(Arc<Mutex<ScriptState>>);

#[derive(Default)]
struct ScriptState {
    active: bool,
    store: BTreeMap<String, Dynamic>,
    requests: Vec<BattleRequest>,
    camera: Option<CameraSnapshot>,
    /// All live participants, republished every running tick for
    /// `battle_participants()` readers.
    participants: Vec<rhai::Map>,
}

/// The battle camera as scripts see it, published every tick.
#[derive(Clone, Copy)]
pub(crate) struct CameraSnapshot {
    pub(crate) position: Vec3,
    /// Yaw and pitch in radians, bevy convention (look direction from
    /// yaw around Y then pitch).
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) fov_radians: f32,
}

impl BattleHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ScriptState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn push(&self, request: BattleRequest) {
        self.lock().requests.push(request);
    }

    pub(crate) fn take_requests(&self) -> Vec<BattleRequest> {
        std::mem::take(&mut self.lock().requests)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn pending_requests(&self) -> usize {
        self.lock().requests.len()
    }

    pub(crate) fn set_active(&self, active: bool) {
        self.lock().active = active;
    }

    pub(crate) fn active(&self) -> bool {
        self.lock().active
    }

    pub(crate) fn set_store(&self, key: &str, value: Dynamic) {
        self.lock().store.insert(key.to_owned(), value);
    }

    pub(crate) fn get_store(&self, key: &str) -> Dynamic {
        self.lock().store.get(key).cloned().unwrap_or(Dynamic::UNIT)
    }

    pub(crate) fn clear_store(&self) {
        self.lock().store.clear();
    }

    pub(crate) fn set_camera(&self, camera: CameraSnapshot) {
        self.lock().camera = Some(camera);
    }

    pub(crate) fn camera(&self) -> Option<CameraSnapshot> {
        self.lock().camera
    }

    pub(crate) fn publish_participants(&self, participants: Vec<rhai::Map>) {
        self.lock().participants = participants;
    }

    pub(crate) fn participants(&self) -> Vec<rhai::Map> {
        self.lock().participants.clone()
    }
}

/// Projects a battle-world point into virtual-screen pixels
/// (320x240, origin top-left) using the published battle camera.
/// Returns the on-screen position and whether the point faces the
/// camera at all.
pub(crate) fn project_to_screen(point: Vec3, camera: &CameraSnapshot) -> (Vec2, bool) {
    // View-space transform: translate then rotate by the inverse of
    // the camera's yaw/pitch.
    let d = point - camera.position;
    let (yaw_sin, yaw_cos) = camera.yaw.sin_cos();
    let (pitch_sin, pitch_cos) = camera.pitch.sin_cos();
    // bevy cameras look down -Z: forward = R_y(yaw) * R_x(pitch) * -Z.
    let forward = Vec3::new(-yaw_sin * pitch_cos, pitch_sin, -yaw_cos * pitch_cos);
    let right = Vec3::new(yaw_cos, 0.0, -yaw_sin);
    let up = right.cross(forward);
    let x = d.dot(right);
    let y = d.dot(up);
    let depth = d.dot(forward);
    if depth <= 0.0 {
        return (Vec2::ZERO, false);
    }
    // Perspective: the projection plane sits 1 unit ahead; fov is
    // vertical.
    let half_fov = (camera.fov_radians / 2.0).tan();
    let aspect = 320.0 / 240.0;
    let ndc_x = (x / (depth * half_fov * aspect)).clamp(-1.5, 1.5);
    let ndc_y = (y / (depth * half_fov)).clamp(-1.5, 1.5);
    let screen_x = (ndc_x * 0.5 + 0.5) * 320.0;
    let screen_y = (0.5 - ndc_y * 0.5) * 240.0;
    (Vec2::new(screen_x, screen_y), true)
}

// ---------------------------------------------------------------- engine state

/// Root of every battle-context graphic: the arena and the fighters.
/// Staging spawns under it and unhides it; cleanup despawns its content
/// and hides it — the scene never bleeds into the fight.
#[derive(Resource)]
pub(crate) struct BattleGraphics(pub(crate) Entity);

pub(crate) fn setup_graphics(commands: &mut Commands) {
    let battle = commands
        .spawn((
            Name::new("battle graphics"),
            Visibility::Hidden,
            Transform::IDENTITY,
        ))
        .id();
    commands.insert_resource(BattleGraphics(battle));
}

/// One staged combatant on the engine side.
pub(crate) struct Combatant {
    pub(crate) id: String,
    pub(crate) entity: Entity,
    pub(crate) position: Vec3,
    pub(crate) time_until_act: f32,
    pub(crate) bag: rhai::Map,
    pub(crate) brain: Option<Brain>,
}

/// The engine-side battle, present as a resource only while one runs.
/// The transition choreography (the capture, the curtain, the cover)
/// lives in transition.rs; by the time this resource exists the world
/// is the fight.
#[derive(Resource)]
pub(crate) struct Battle {
    pub(crate) arena: Entity,
    pub(crate) participants: Vec<Combatant>,
    /// Action handlers by name.
    pub(crate) actions: BTreeMap<String, ActionHandler>,
    pub(crate) result: Option<String>,
}

impl Battle {
    /// The participant with the lowest time-until-act, if any.
    pub(crate) fn next_actor(&self) -> Option<usize> {
        self.participants
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.time_until_act.total_cmp(&b.1.time_until_act))
            .map(|(i, _)| i)
    }
}

/// Marks a battle participant entity (model root + brain + bag live in
/// `Battle`).
#[derive(Component)]
pub(crate) struct BattleParticipant;

// ---------------------------------------------------------------- registration

/// Wires battle host functions into a script engine. `battle` carries
/// script-visible state; `engine`/`ast` are the *calling* script's,
/// captured so inline brains and actions can be invoked later.
pub(crate) fn register_battle_api(
    engine: &mut Engine,
    battle: &BattleHandle,
    source: &crate::scripts::ScriptSource,
) {
    use crate::scripts::runtime_error;

    {
        let battle = battle.clone();
        engine.register_fn("in_battle", move || -> bool { battle.active() });
    }

    {
        let battle = battle.clone();
        engine.register_fn(
            "start_battle",
            move |arena: &str,
                  cam: rhai::Map,
                  participants: rhai::Array|
                  -> Result<(), Box<rhai::EvalAltResult>> {
                let camera_pos = vec_from_map(&cam, "pos")?;
                let camera_look = vec_from_map(&cam, "look")?;
                let mut defs = Vec::new();
                for entry in &participants {
                    let map = entry.clone().try_cast::<rhai::Map>().ok_or_else(|| {
                        runtime_error("start_battle: participants must be maps".into())
                    })?;
                    let id = map
                        .get("id")
                        .and_then(|v| v.clone().into_string().ok())
                        .ok_or_else(|| {
                            runtime_error("start_battle: participant needs id".into())
                        })?;
                    let model = map
                        .get("model")
                        .and_then(|v| v.clone().into_string().ok())
                        .unwrap_or_else(|| "models/character.glb".into());
                    let position = vec_from_map(&map, "position")?;
                    let facing = map
                        .get("facing")
                        .and_then(|v| v.as_float().ok())
                        .unwrap_or(0.0) as f32;
                    let brain = map.get("brain").and_then(|v| v.clone().into_string().ok());
                    let time_until_act = map
                        .get("time_until_act")
                        .and_then(|v| v.as_float().ok())
                        .unwrap_or(0.0) as f32;
                    let bag = map
                        .get("bag")
                        .and_then(|v| v.clone().try_cast::<rhai::Map>())
                        .unwrap_or_default();
                    defs.push(ParticipantDef {
                        id,
                        model,
                        position,
                        facing_degrees: facing,
                        brain,
                        time_until_act,
                        bag,
                    });
                }
                battle.push(BattleRequest::Start(StartBattle {
                    arena: arena.to_owned(),
                    camera_pos,
                    camera_look,
                    participants: defs,
                }));
                Ok(())
            },
        );
    }

    {
        let battle = battle.clone();
        engine.register_fn("end_battle", move |result: &str| {
            battle.push(BattleRequest::End {
                result: result.to_owned(),
            });
        });
    }

    {
        let battle = battle.clone();
        let source = source.clone();
        engine.register_fn("battle_action", move |name: &str, fn_name: &str| {
            battle.push(BattleRequest::Action {
                name: name.to_owned(),
                handler: ActionHandler {
                    source: source.clone(),
                    fn_name: fn_name.to_owned(),
                },
            });
        });
    }

    {
        let battle = battle.clone();
        let source = source.clone();
        engine.register_fn("battle_brain", move |participant: &str, fn_name: &str| {
            battle.push(BattleRequest::Brain {
                participant: participant.to_owned(),
                brain: Brain::Inline {
                    source: source.clone(),
                    fn_name: fn_name.to_owned(),
                },
            });
        });
    }

    {
        let battle = battle.clone();
        engine.register_fn("battle_set", move |key: &str, value: Dynamic| {
            battle.set_store(key, value);
        });
    }

    {
        let battle = battle.clone();
        engine.register_fn("battle_get", move |key: &str| -> Dynamic {
            battle.get_store(key)
        });
    }

    {
        let battle = battle.clone();
        engine.register_fn("battle_emote", move |participant: &str, clip: &str| {
            battle.push(BattleRequest::Emote {
                participant: participant.to_owned(),
                clip: clip.to_owned(),
            });
        });
    }

    {
        let battle = battle.clone();
        engine.register_fn(
            "camera_move",
            move |yaw: f64, pitch: f64, dolly: f64| {
                battle.push(BattleRequest::CameraMove {
                    yaw: yaw as f32,
                    pitch: pitch as f32,
                    dolly: dolly as f32,
                });
            },
        );
    }
}

/// Participants as maps for brains and UI: engine fields with the bag
/// merged over them. Registered as `battle_participants()`; needs the
/// live list, so the engine publishes it into the handle each tick.
pub(crate) fn register_battle_readers(engine: &mut Engine, battle: &BattleHandle) {
    use crate::scripts::runtime_error;

    {
        let battle = battle.clone();
        engine.register_fn("battle_participants", move || -> rhai::Array {
            battle
                .participants()
                .into_iter()
                .map(Dynamic::from)
                .collect()
        });
    }

    {
        let battle = battle.clone();
        engine.register_fn(
            "battle_screen_pos",
            move |x: f64, y: f64, z: f64| -> Result<rhai::Map, Box<rhai::EvalAltResult>> {
                let camera = battle
                    .camera()
                    .ok_or_else(|| runtime_error("battle_screen_pos: no battle camera".into()))?;
                let (screen, onscreen) =
                    project_to_screen(Vec3::new(x as f32, y as f32, z as f32), &camera);
                let mut map = rhai::Map::new();
                map.insert("x".into(), Dynamic::from(screen.x as f64));
                map.insert("y".into(), Dynamic::from(screen.y as f64));
                map.insert("onscreen".into(), Dynamic::from(onscreen));
                Ok(map)
            },
        );
    }

    {
        let battle = battle.clone();
        engine.register_fn(
            "battle_bag_set",
            move |participant: &str, key: &str, value: Dynamic| {
                battle
                    .lock()
                    .store
                    .insert(format!("bag:{participant}:{key}"), value);
            },
        );
    }

    {
        let battle = battle.clone();
        engine.register_fn(
            "battle_bag_get",
            move |participant: &str, key: &str| -> Dynamic {
                battle
                    .lock()
                    .store
                    .get(&format!("bag:{participant}:{key}"))
                    .cloned()
                    .unwrap_or(Dynamic::UNIT)
            },
        );
    }
}

fn vec_from_map(map: &rhai::Map, key: &str) -> Result<Vec3, Box<rhai::EvalAltResult>> {
    let value = map
        .get(key)
        .ok_or_else(|| crate::scripts::runtime_error(format!("missing '{key}'")))?;
    let array = value
        .clone()
        .try_cast::<rhai::Array>()
        .ok_or_else(|| crate::scripts::runtime_error(format!("'{key}' must be [x, y, z]")))?;
    let mut out = [0.0f64; 3];
    for (i, entry) in array.iter().take(3).enumerate() {
        out[i] = entry.as_float().unwrap_or(0.0);
    }
    Ok(Vec3::new(out[0] as f32, out[1] as f32, out[2] as f32))
}

/// The camera transform (position + yaw/pitch) derived from a position
/// and a look-at target.
pub(crate) fn camera_from_look(position: Vec3, look: Vec3) -> (Vec3, Vec2) {
    let dir = (look - position).normalize_or_zero();
    let yaw = (-dir.x).atan2(-dir.z);
    let pitch = dir.y.asin().clamp(-1.5, 1.5);
    (position, Vec2::new(yaw, pitch))
}

// ---------------------------------------------------------------- engine systems

/// Everything the battle systems need, gathered once.
#[allow(clippy::type_complexity)]
#[derive(SystemParam)]
pub(crate) struct BattleTurnParams<'w, 's> {
    pub(crate) battle: ResMut<'w, BattleHandle>,
    pub(crate) state: Option<ResMut<'w, Battle>>,
    pub(crate) camera: Query<'w, 's, (&'static Transform, &'static Projection), With<BattleCamera>>,
}

/// Drains script requests, runs phases, and sequences turns. Sits at
/// the head of FixedUpdate so `in_battle()` is fresh for every other
/// system and script this tick.
/// Drains script requests in BOTH states: before a battle they
/// accumulate into [`PendingBattleStart`] (whose `OnEnter` staging
/// consumes them); during one they apply to the live battle. Start and
/// End begin the transition that carries the game in or out.
#[allow(clippy::too_many_arguments)]
pub(crate) fn battle_requests(
    battle: ResMut<BattleHandle>,
    mut battle_state: Option<ResMut<Battle>>,
    mut next_state: ResMut<NextState<GameState>>,
    mut transition: ResMut<TransitionState>,
    mut pending: ResMut<PendingBattleStart>,
    mut emote_targets: Query<&mut EmoteRequest>,
    mut cameras: Query<(&mut Transform, &Camera), Or<(With<SceneCamera>, With<BattleCamera>)>>,
) {
    let mut end: Option<String> = None;
    for request in battle.take_requests() {
        match request {
            BattleRequest::Start(def) => {
                // Into the transition state: the swirl freezes the scene,
                // and staging fires behind the cover (OnEnter(Battle)).
                if transition.begin(GameState::Battle, Effect::Swirl) {
                    pending.def = Some(def);
                    next_state.set(GameState::Transition);
                } else {
                    warn!("start_battle ignored: a transition is already running");
                }
            }
            BattleRequest::End { result } => end = Some(result),
            BattleRequest::Action { name, handler } => {
                if let Some(b) = battle_state.as_mut() {
                    b.actions.insert(name, handler);
                } else {
                    pending.actions.insert(name, handler);
                }
            }
            BattleRequest::Brain { participant, brain } => {
                if let Some(b) = battle_state.as_mut()
                    && let Some(c) = b.participants.iter_mut().find(|c| c.id == participant)
                {
                    c.brain = Some(brain);
                } else {
                    pending.brains.push((participant, brain));
                }
            }
            BattleRequest::Emote { participant, clip } => {
                if let Some(b) = battle_state.as_mut()
                    && let Some(c) = b.participants.iter().find(|c| c.id == participant)
                    && let Some(mut request) = emote_targets.get_mut(c.entity).ok()
                {
                    request.emote = Some(clip);
                }
            }
            BattleRequest::CameraMove {
                yaw: dyaw,
                pitch: dpitch,
                dolly,
            } => {
                // The free-camera flight: rotate, then slide along the
                // new look direction — of whichever view is active,
                // scene or battle alike. The sequencer republishes the
                // camera snapshot from this transform every tick, so
                // script projections stay truthful while flying.
                for (mut transform, camera) in cameras.iter_mut() {
                    if !camera.is_active {
                        continue;
                    }
                    let (yaw_now, pitch_now, _) =
                        transform.rotation.to_euler(EulerRot::YXZ);
                    let yaw = yaw_now + dyaw;
                    let pitch = (pitch_now + dpitch).clamp(-1.5, 1.5);
                    transform.rotation =
                        Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
                    let slide = transform.forward() * dolly;
                    transform.translation += slide;
                }
            }
        }
    }
        if let Some(result) = end {
            if let Some(b) = battle_state.as_mut() {
                b.result = Some(result);
            } else {
                bevy::log::warn!("battle: end_battle with no battle running");
            }
            // Back to the scene behind a plain fade; the teardown fires
            // at the covered point (OnEnter(Scene)).
            if transition.begin(GameState::Scene, Effect::Fade) {
                next_state.set(GameState::Transition);
            } else {
                warn!("end_battle ignored: a transition is already running");
            }
        }
}

/// The battle-internal machine: phases, the turn sequencer, the fade.
/// One running-battle tick: find who acts, poll their brain, dispatch
/// the action. The transition choreography lives in transition.rs; by
/// the time this runs the world is the fight.
pub(crate) fn battle_turns(mut params: BattleTurnParams) {
    if let Some(state) = params.state.as_mut() {
        sequence_turn(&params.battle, &params.camera, state);
    }
}

/// `OnEnter(Battle)` staging: builds the arena and the participants
/// from the pending start — the hero is a fresh entity, exactly like
/// the enemies; nothing carries over from the scene — wires brains
/// (files + the inline ones the battle script registered), and poses
/// the camera. Everything stages under [`BattleGraphics`], behind the
/// opaque transition cover.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stage_battle(
    mut commands: Commands,
    mut pending: ResMut<PendingBattleStart>,
    handle: ResMut<BattleHandle>,
    assets: Res<AssetServer>,
    graphics: Res<BattleGraphics>,
    mut camera: Query<&'static mut Transform, With<BattleCamera>>,
    input: Res<InputManager>,
    ui: Res<UiApi>,
    world_state: Res<WorldState>,
) {
    let Some(def) = pending.def.take() else {
        warn!("entered Battle without a pending start");
        return;
    };
    // The room's floor sits at the model's local origin
    // (tools/generate_arena.py), so the arena spawns unlifted and the
    // fighters stand on it.
    let arena = commands
        .spawn((
            ActorModel(assets.load(&def.arena)),
            Visibility::default(),
            Transform::IDENTITY,
        ))
        .id();
    commands.entity(graphics.0).add_child(arena);

    // Brain files compile with the full script environment: a menu
    // brain needs the UI, a coordinated pack needs the battle store.
    let env = crate::scripts::ScriptEnv::new(
        input.handle(),
        ui.clone(),
        world_state.clone(),
        handle.clone(),
    );
    let mut participants = Vec::new();
    for def in &def.participants {
        let rotation = Quat::from_rotation_y(def.facing_degrees.to_radians());
        // The hero stages like any other participant: a fresh model at
        // the formation slot, spawned visible — staging fires behind
        // the opaque transition cover, and the resolver's reveal owns
        // the model root's visibility from there.
        let entity = commands
            .spawn((
                BattleParticipant,
                Locomotion::default(),
                ActorModel(assets.load(&def.model)),
                Visibility::default(),
                Transform::from_translation(def.position).with_rotation(rotation),
            ))
            .id();
        commands.entity(graphics.0).add_child(entity);
        let mut brain = None;
        if let Some(path) = &def.brain {
            brain = crate::scripts::ActorScript::load(path, env.clone().with_store(path))
                .map(|script| Brain::File { script });
        }
        participants.push(Combatant {
            id: def.id.clone(),
            entity,
            position: def.position,
            time_until_act: def.time_until_act,
            bag: def.bag.clone(),
            brain,
        });
    }
    // The battle camera takes the script's pose; the scene camera is
    // never touched, so there is nothing to restore on the way out.
    // glam's YXZ angles arrive in axis order: yaw (Y) first, pitch (X)
    // second — swapping them points the camera up and sideways.
    if let Ok(mut transform) = camera.single_mut() {
        let (position, yaw_pitch) = camera_from_look(def.camera_pos, def.camera_look);
        transform.translation = position;
        let (yaw, pitch) = (yaw_pitch.x, yaw_pitch.y);
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }

    // The store starts clean, then is seeded with the starting bags:
    // handler scripts read and write bag values through it, and
    // sequence_turn folds the store back into the participants before
    // publishing.
    handle.set_active(true);
    handle.clear_store();
    for participant in &participants {
        for (key, value) in &participant.bag {
            let store_key = format!("bag:{}:{key}", participant.id);
            handle.set_store(&store_key, value.clone());
        }
    }
    commands.insert_resource(Battle {
        arena,
        participants,
        actions: std::mem::take(&mut pending.actions),
        result: None,
    });
    commands.entity(graphics.0).insert(Visibility::Visible);
}
/// `OnEnter(Scene)`: fires at the transition's covered point — the
/// arena and the fighters go, and the scene modules restore themselves
/// in their own hooks.
pub(crate) fn cleanup_battle(
    mut commands: Commands,
    battle: Option<Res<Battle>>,
    handle: ResMut<BattleHandle>,
    ui: Res<UiApi>,
    // Fires once at boot (before Startup) with no root to fold.
    graphics: Option<Res<BattleGraphics>>,
) {
    let Some(battle) = battle else {
        return;
    };
    // A battle script that errored can't run its own exit cleanup, so
    // the engine sweeps every battle window regardless.
    ui.push(UiRequest::Close {
        name: "battle".to_owned(),
    });
    ui.push(UiRequest::Close {
        name: "battle-status".to_owned(),
    });
    for n in 0..8 {
        ui.push(UiRequest::Close {
            name: format!("float{n}"),
        });
    }
    for combatant in &battle.participants {
        commands.entity(combatant.entity).despawn();
    }
    commands.entity(battle.arena).despawn();
    commands.remove_resource::<Battle>();
    handle.set_active(false);
    handle.clear_store();
    if let Some(graphics) = graphics {
        commands.entity(graphics.0).insert(Visibility::Hidden);
    }
}

/// One running-battle tick: publish state for scripts, find who acts,
/// ask their brain, dispatch the chosen action.
fn sequence_turn(
    battle: &BattleHandle,
    camera: &Query<(&Transform, &Projection), With<BattleCamera>>,
    state: &mut Battle,
) {
    // Script-side bag mutations land in the store ("bag:<id>:<key>");
    // fold them into the participant bags before publishing, so the
    // HUD sees the damage the actions dealt.
    {
        let store = battle.lock();
        for participant in &mut state.participants {
            let prefix = format!("bag:{}:", participant.id);
            for (key, value) in store.store.range(prefix.clone()..) {
                if !key.starts_with(&prefix) {
                    break;
                }
                participant
                    .bag
                    .insert(key[prefix.len()..].into(), value.clone());
            }
        }
    }

    // Publish participants for `battle_participants()` readers.
    let maps: Vec<rhai::Map> = state
        .participants
        .iter()
        .map(|c| participant_map(&c.id, c.position, c.time_until_act, &c.bag))
        .collect();
    battle.publish_participants(maps);

    // Publish the camera snapshot for `battle_screen_pos`.
    if let Ok((transform, Projection::Perspective(perspective))) = camera.single() {
        let fov = perspective.fov;
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        battle.set_camera(CameraSnapshot {
            position: transform.translation,
            yaw,
            pitch,
            fov_radians: fov,
        });
    }

    // Give every participant a little time back each tick? No: only
    // the active one consumes time — everyone else is frozen while the
    // current actor acts, which is what turn-based means here.
    let Some(index) = state.next_actor() else {
        return;
    };
    let (id, position, time_until_act, bag) = {
        let c = &state.participants[index];
        (c.id.clone(), c.position, c.time_until_act, c.bag.clone())
    };
    let Some(brain) = state.participants[index].brain.as_ref() else {
        warn!("battle: participant {id} has no brain; idling");
        state.participants[index].time_until_act += 1.0;
        return;
    };

    let participant = participant_map(&id, position, time_until_act, &bag);
    let decision = decide_with_brain(brain, participant);
    match decision {
        Ok(choice) => {
            let Some(action_name) = choice else {
                // Still deciding (a menu waiting for input): try again
                // next tick.
                return;
            };
            // A unit means the brain (usually the menu) is still
            // deciding; anything non-string is a script bug.
            if action_name.is_unit() {
                return;
            }
            let Ok(action_name) = action_name.into_string() else {
                warn!("battle: brain for {id} returned a non-string action");
                state.participants[index].time_until_act += 1.0;
                return;
            };
            let Some(handler) = state.actions.get(&action_name).cloned() else {
                warn!("battle: no action '{action_name}' registered");
                state.participants[index].time_until_act += 1.0;
                return;
            };
            let recovery = run_action(&handler, &id);
            state.participants[index].time_until_act += recovery;
        }
        Err(e) => {
            warn!("battle: brain for {id} errored: {e}");
            state.participants[index].time_until_act += 1.0;
        }
    }
}

fn decide_with_brain(brain: &Brain, participant: rhai::Map) -> Result<Option<Dynamic>, String> {
    match brain {
        Brain::File { script, .. } => {
            let mut scope = Scope::new();
            script
                .call_dynamic(&mut scope, "decide", participant.into())
                .map(Some)
                .map_err(|e| e.to_string())
        }
        Brain::Inline { source, fn_name } => {
            let inner = source.lock().unwrap_or_else(PoisonError::into_inner);
            let inner = inner.as_ref().ok_or("brain source went away")?;
            let mut scope = Scope::new();
            inner
                .engine
                .call_fn(&mut scope, &inner.ast, fn_name, (participant,))
                .map(Some)
                .map_err(|e| e.to_string())
        }
    }
}

fn run_action(handler: &ActionHandler, actor_id: &str) -> f32 {
    let inner = handler
        .source
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(inner) = inner.as_ref() else {
        warn!("battle: action '{}' lost its source", handler.fn_name);
        return 1.0;
    };
    let mut scope = Scope::new();
    match inner.engine.call_fn::<f64>(
        &mut scope,
        &inner.ast,
        &handler.fn_name,
        (actor_id.to_string(),),
    ) {
        Ok(recovery) => recovery as f32,
        Err(e) => {
            warn!("battle: action '{}' errored: {e}", handler.fn_name);
            1.0
        }
    }
}

// ---------------------------------------------------------------- fade overlay

/// Spawns the fullscreen fade quad once at startup; the battle engine
/// only re-alphas its material.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lowest_time_until_act_acts_first() {
        let battle = Battle {
            arena: Entity::PLACEHOLDER,
            participants: vec![
                Combatant {
                    id: "hero".into(),
                    entity: Entity::PLACEHOLDER,
                    position: Vec3::ZERO,
                    time_until_act: 1.2,
                    bag: rhai::Map::new(),
                    brain: None,
                },
                Combatant {
                    id: "goblin".into(),
                    entity: Entity::PLACEHOLDER,
                    position: Vec3::ZERO,
                    time_until_act: 0.3,
                    bag: rhai::Map::new(),
                    brain: None,
                },
            ],
            actions: BTreeMap::new(),
            result: None,
        };
        assert_eq!(
            battle
                .next_actor()
                .map(|i| battle.participants[i].id.as_str()),
            Some("goblin")
        );
    }

    #[test]
    fn screen_positions_project_in_front_and_fail_behind() {
        // A camera at the stage's south edge looking north, the usual
        // battle shot: a point ahead projects near the screen middle;
        // a point behind the camera is reported off-screen.
        let camera = CameraSnapshot {
            position: Vec3::new(0.0, 3.0, 8.5),
            yaw: 0.0,
            pitch: -0.2,
            fov_radians: 45.0_f32.to_radians(),
        };
        let (front, onscreen) = project_to_screen(Vec3::new(0.0, 1.0, 0.0), &camera);
        assert!(onscreen);
        assert!((front.x - 160.0).abs() < 2.0, "centered: {front:?}");
        assert!(
            front.y > 100.0 && front.y < 140.0,
            "near the middle: {front:?}"
        );

        let (_, behind) = project_to_screen(Vec3::new(0.0, 1.0, 20.0), &camera);
        assert!(!behind);
    }

    #[test]
    fn the_shipped_player_brain_menus_then_commits() {
        use crate::input::InputState;
        use crate::scripts::{ActorScript, ScriptEnv};
        use crate::systems::ui::UiApi;

        let ui = UiApi::new();
        let env = ScriptEnv::new(
            crate::input::detached(),
            ui.clone(),
            crate::world_state::WorldState::default(),
            BattleHandle::new(),
        );
        let brain = ActorScript::compile_with_handle(
            include_str!("../assets/scripts/battle/player_brain.rhai"),
            env,
        )
        .expect("the shipped player brain must compile");

        let participant: rhai::Map = [
            ("id", Dynamic::from("hero".to_owned())),
            ("hp", Dynamic::from(87.0)),
            ("max_hp", Dynamic::from(100.0)),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect();

        // No menu pick yet: still deciding.
        let mut scope = Scope::new();
        let choice = brain
            .call_dynamic(&mut scope, "decide", participant.clone().into())
            .unwrap();
        assert!(choice.is_unit());

        // The menu declared two options; pressing cross picks Attack.
        let nav = ui.nav().lock().unwrap_or_else(PoisonError::into_inner);
        let mut nav = nav;
        nav.declare("battle", 2);
        let mut input = InputState::default();
        input.inject(&[], &[crate::input::PadButton::Cross], &[]);
        nav.navigate(&input);
        drop(nav);

        let mut scope = Scope::new();
        let choice = brain
            .call_dynamic(&mut scope, "decide", participant.into())
            .unwrap();
        assert_eq!(choice.into_string().unwrap(), "attack");
    }

    #[test]
    fn an_inline_brain_answers_through_its_source_cell() {
        let engine = Engine::new();
        let ast = engine
            .compile("fn goblin_decide(participant) { \"attack\" }")
            .unwrap();
        let source: crate::scripts::ScriptSource =
            Arc::new(Mutex::new(Some(Arc::new(crate::scripts::ScriptInner {
                engine,
                ast,
            }))));
        let brain = Brain::Inline {
            source,
            fn_name: "goblin_decide".into(),
        };
        let participant = participant_map("goblin", Vec3::ZERO, 0.0, &rhai::Map::new());
        let choice = decide_with_brain(&brain, participant).unwrap().unwrap();
        assert_eq!(choice.into_string().unwrap(), "attack");
    }

    #[test]
    fn the_camera_flight_moves_the_active_camera() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(BattleHandle::new());
        world.init_resource::<crate::transition::TransitionState>();
        world.init_resource::<NextState<GameState>>();
        world.init_resource::<PendingBattleStart>();
        world.spawn((
            BattleCamera,
            Camera {
                is_active: true,
                ..default()
            },
            Transform::from_xyz(10.0, 9.0, 10.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
            Projection::default(),
        ));
        // The scene camera shares the sequencer's query; inactive, so the
        // flight must leave it alone.
        world.spawn((
            crate::systems::camera::SceneCamera,
            Camera {
                is_active: false,
                ..default()
            },
            Transform::from_xyz(0.0, 6.0, 9.0),
            Projection::default(),
        ));
        let pose = |world: &mut World| {
            *world
                .query::<(&BattleCamera, &Transform)>()
                .single(world)
                .expect("the harness spawns the battle camera")
                .1
        };
        let scene_pose = |world: &mut World| {
            *world
                .query::<(&crate::systems::camera::SceneCamera, &Transform)>()
                .single(world)
                .expect("the harness spawns the scene camera")
                .1
        };
        let original = pose(&mut world);

        // No battle: the flight still applies — it is the free camera,
        // not battle-scoped. It turns the camera half a radian and
        // slides it exactly one unit along the (new) look direction.
        world
            .resource_mut::<BattleHandle>()
            .push(BattleRequest::CameraMove {
                yaw: 0.5,
                pitch: 0.0,
                dolly: 1.0,
            });
        world.run_system_once(battle_requests).unwrap();
        let idle = pose(&mut world);
        assert_ne!(idle, original);
        let scene_original = scene_pose(&mut world);

        // A live battle: same treatment for the (still active) camera.
        world.insert_resource(Battle {
            arena: Entity::PLACEHOLDER,
            participants: vec![],
            actions: BTreeMap::new(),
            result: None,
        });
        world
            .resource_mut::<BattleHandle>()
            .push(BattleRequest::CameraMove {
                yaw: 0.5,
                pitch: 0.0,
                dolly: 1.0,
            });
        world.run_system_once(battle_requests).unwrap();
        let flown = pose(&mut world);

        let (yaw, _, _) = flown.rotation.to_euler(EulerRot::YXZ);
        let (yaw_before, _, _) = idle.rotation.to_euler(EulerRot::YXZ);
        assert!((yaw - yaw_before - 0.5).abs() < 1e-4);
        assert!((flown.translation - idle.translation).length() - 1.0 < 1e-4);
        // The inactive scene camera never flew.
        assert_eq!(scene_pose(&mut world), scene_original);
    }

    #[test]
    fn staging_leaves_the_fighters_visible_and_on_the_floor() {
        use bevy::asset::AssetPlugin;
        use bevy::ecs::system::RunSystemOnce;

        // Staging fires at the transition's covered point, behind the
        // opaque cover: nothing in staging may hide the fighters, and
        // the arena floor sits at world zero (the generator's floor is
        // its local origin), so the fighters stand on it. The hero
        // stages as a fresh entity, exactly like the enemies.
        let mut app = App::new();
        bevy::tasks::IoTaskPool::get_or_init(bevy::tasks::TaskPool::default);
        app.add_plugins(AssetPlugin::default());
        app.init_asset::<bevy::gltf::Gltf>();
        let world = app.world_mut();
        world.init_resource::<BattleHandle>();
        crate::battle::setup_graphics(&mut world.commands());
        world.flush();
        let graphics = world.resource::<BattleGraphics>().0;
        world.insert_resource(PendingBattleStart {
            def: Some(StartBattle {
                arena: "models/arena.glb".into(),
                camera_pos: Vec3::new(10.0, 9.0, 10.0),
                camera_look: Vec3::new(0.0, 0.5, 0.0),
                participants: vec![
                    ParticipantDef {
                        id: "hero".into(),
                        model: "models/character.glb".into(),
                        position: Vec3::new(0.0, 0.0, 3.0),
                        facing_degrees: 180.0,
                        brain: None,
                        time_until_act: 0.0,
                        bag: rhai::Map::new(),
                    },
                    ParticipantDef {
                        id: "goblin".into(),
                        model: "models/goblin.glb".into(),
                        position: Vec3::new(0.0, 0.0, -3.0),
                        facing_degrees: 0.0,
                        brain: None,
                        time_until_act: 0.0,
                        bag: rhai::Map::new(),
                    },
                ],
            }),
            ..default()
        });
        world.spawn((BattleCamera, Transform::IDENTITY, Projection::default()));
        world.insert_resource(InputManager::standard());
        world.insert_resource(UiApi::new());
        world.init_resource::<WorldState>();

        world.run_system_once(stage_battle).unwrap();
        world.flush();

        let mut fighters = world.query_filtered::<&Visibility, With<BattleParticipant>>();
        assert_eq!(fighters.iter(world).count(), 2, "the hero stages fresh");
        for visibility in fighters.iter(world) {
            assert_ne!(*visibility, Visibility::Hidden);
        }
        let mut arenas = world.query_filtered::<&Transform, (With<ActorModel>, Without<BattleParticipant>)>();
        let arena = arenas
            .single(world)
            .expect("the arena staged");
        assert_eq!(*arena, Transform::IDENTITY, "the floor is at zero");
        let mut children = world.query::<&ChildOf>();
        assert!(
            children.iter(world).all(|child| child.parent() == graphics),
            "everything staged under the battle graphics root"
        );
    }
}
