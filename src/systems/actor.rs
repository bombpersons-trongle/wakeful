//! Scene actors: a positioned model per actor, optionally driven by a
//! Rhai script (see [`crate::scripts`] for the contract).

use bevy::ecs::system::SystemParam;
use bevy::gltf::Gltf;
use bevy::prelude::*;
use rhai::Scope;

use crate::CurrentScene;
use crate::GameCamera;
use crate::Player;
use crate::movement::{TURN_SPEED, face_direction, facing_rotation};
use crate::scene::Scene;
use crate::scripts::{ActorScript, Said, ScriptBroken};
use crate::systems::animation::{EmoteRequest, Locomotion, PendingAnimations};
use crate::systems::bubble::{self, BubbleTheme};
use crate::systems::party::Party;
use crate::systems::scene::{SceneGraphics, gltf_asset_path};
use crate::text::TextAssets;
use crate::walkmesh;

/// How long a scripted line stays up before closing itself.
const SAY_TTL_SECS: f32 = 3.0;
/// Screen pixels between the actor's projected ground point and the
/// bubble's center, lifting the bubble above the model.
const SAY_HEADROOM_PX: f32 = 24.0;

/// How wide a constrained actor's body is, for walk-mesh collision.
/// Actors are models of unknown size; this matches the player's.
const ACTOR_RADIUS: f32 = crate::systems::player::PLAYER_RADIUS;

/// Marks an actor whose script has run at least once. The animation
/// driver keeps a freshly loaded model hidden until this lands (or a
/// clip is applied), so a revisited scene never flashes the bind pose.
#[derive(Component, Default)]
pub(crate) struct ScriptTicked;

/// Marks a scene actor. The optional script runtime is disabled once it
/// errors, so a broken file can't spam warnings every tick.
#[derive(Component)]
pub struct Actor {
    script: Option<ScriptRuntime>,
    /// The bubble showing the actor's last line, replaced per `say`.
    bubble: Option<Entity>,
    /// The line (and its options) currently in that bubble, to dedupe
    /// per-tick repeats.
    said: Option<Said>,
    /// Whether the scene's walk mesh bounds this actor and carries its
    /// height. False for actors a script places off the mesh.
    constrained: bool,
}

/// A compiled script and its persistent variable scope.
struct ScriptRuntime {
    script: ActorScript,
    scope: Scope<'static>,
}

/// glTF model queued for the actor; removed once attached as a child.
#[derive(Component)]
pub(crate) struct ActorModel(pub(crate) Handle<Gltf>);

/// Actors with runnable scripts and their transform; broken scripts are
/// filtered out at the query level.
type ActorQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Transform,
        &'static mut Actor,
        Option<&'static mut Locomotion>,
    ),
    (Without<Player>, Without<ScriptBroken>),
>;

/// Spawns one entity per scene actor at its ground position, facing
/// `toward` (usually the camera forward, matching the player's spawn).
/// The model attaches when its glTF finishes loading; a missing or
/// broken script leaves the actor standing (with a warning).
pub(crate) fn spawn_actors(
    commands: &mut Commands,
    assets: &AssetServer,
    scene: &Scene,
    toward: Vec2,
    scene_path: &str,
    env: &crate::scripts::ScriptEnv,
    graphics: &crate::systems::scene::SceneGraphics,
) {
    for actor in &scene.actors {
        // Storage identity: game-wide id when set (shared across
        // scenes on request), else the script path.
        let script = actor
            .script
            .as_deref()
            .and_then(|path| {
                let key = match actor.id.as_deref() {
                    Some(id) if actor.shared => format!("actor::{id}"),
                    Some(id) => format!("{scene_path}::{id}"),
                    None => path.to_owned(),
                };
                ActorScript::load(
                    path,
                    env.clone()
                        .with_store(&key)
                        .with_params(actor.params.clone()),
                )
            })
            .map(|script| ScriptRuntime {
                script,
                scope: Scope::new(),
            });
        let at = Vec2::new(actor.position[0], actor.position[1]);
        // The walk mesh decides how high the ground is under the actor, so
        // a slope carries its characters instead of leaving them floating.
        let ground = walkmesh::ground_height(scene.walk_mesh.as_ref(), at.x, at.y, at);
        let entity = commands
            .spawn((
                Actor {
                    script,
                    bubble: None,
                    said: None,
                    constrained: actor.constrained,
                },
                Visibility::default(),
                Locomotion::default(),
                // No script to tick: the driver's reveal marker would
                // never land, so show script-less models from the start.
                ScriptTicked,
                ActorModel(assets.load(gltf_asset_path(&actor.model))),
                // A pinned facing turns the model to a world yaw; otherwise
                // the actor looks the way the scene camera looks.
                Transform::from_xyz(at.x, ground, at.y).with_rotation(
                    match actor.facing {
                        Some(degrees) => Quat::from_rotation_y(degrees.to_radians()),
                        None => facing_rotation(toward),
                    },
                ),
            ))
            .id();
        commands.entity(graphics.0).add_child(entity);
    }
}

/// Attaches each actor's model once its glTF file has loaded.
pub(crate) fn attach_actor_models(
    mut commands: Commands,
    gltfs: Res<Assets<Gltf>>,
    actors: Query<(Entity, &ActorModel)>,
) {
    for (entity, model) in &actors {
        let Some(gltf) = gltfs.get(&model.0) else {
            continue;
        };
        let Some(scene) = gltf
            .default_scene
            .clone()
            .or_else(|| gltf.scenes.first().cloned())
        else {
            continue;
        };
        commands
            .entity(entity)
            .insert(PendingAnimations(model.0.clone()))
            .with_child((
                WorldAssetRoot(scene),
                Visibility::default(),
                Transform::default(),
            ));
        commands.entity(entity).remove::<ActorModel>();
    }
}

/// The optional UI assets speech needs: the resources are absent until
/// their `setup` systems have run.
#[derive(SystemParam)]
pub(crate) struct UiAssets<'w> {
    bubbles: Option<Res<'w, bubble::BubbleAssets>>,
    text: Option<Res<'w, TextAssets>>,
    theme: Option<Res<'w, BubbleTheme>>,
}

/// Runs each actor's `on_update`, applies the returned position, and
/// shows whatever the script `say`-ed as a speech bubble above it.
/// A constrained actor (the default) is kept on the scene's walk mesh and
/// rides its height; one with `constrained: false` is trusted content and
/// goes wherever its script says.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_actor_scripts(
    mut commands: Commands,
    time: Res<Time>,
    mut party: ResMut<Party>,
    players: Query<&Transform, With<Player>>,
    cameras: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut actors: ActorQuery,
    mut bubbles: Query<&mut bubble::SpeechBubble>,
    ui_assets: UiAssets,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let (player_x, player_z) = (player.translation.x, player.translation.z);
    let dt = time.delta_secs();
    let camera = cameras.single().ok();
    let walk_mesh = current
        .as_ref()
        .and_then(|c| scenes.get(&c.handle))
        .and_then(|scene| scene.walk_mesh.as_ref());
    for (entity, mut transform, mut actor, locomotion) in &mut actors {
        let position = &transform.translation;
        // The script polls waiting() to hold its place while the player
        // hasn't confirmed its wait-mode bubble yet. Computed before the
        // script runtime borrows the actor mutably.
        // Script-facing waiting() = "my wait-mode bubble exists" — the
        // opening animation counts, or an eager press during the pop
        // would slip past the script's challenge gate. The is_open gate
        // lives only in the engine's dismissal.
        let waiting = actor
            .bubble
            .is_some_and(|bubble| bubbles.get(bubble).is_ok_and(|b| b.is_waiting()));
        let Some(runtime) = actor.script.as_mut() else {
            continue;
        };
        // The model reveal waits for this (see ScriptTicked).
        commands.entity(entity).insert(ScriptTicked);
        runtime.script.set_waiting(waiting);
        match runtime.script.update(
            &mut runtime.scope,
            position.x,
            position.z,
            player_x,
            player_z,
            dt,
        ) {
            Ok(tick) => {
                let from = Vec2::new(position.x, position.z);
                if let Some([x, z]) = tick.position {
                    let to = Vec2::new(x, z);
                    // A constrained actor keeps its whole body on the mesh
                    // and slides along its edge, like the player.
                    let to = match (actor.constrained, walk_mesh) {
                        (true, Some(mesh)) => mesh.constrain(from, to, ACTOR_RADIUS),
                        _ => to,
                    };
                    let ground = walkmesh::ground_height(walk_mesh, to.x, to.y, from);
                    transform.translation = Vec3::new(to.x, ground, to.y);
                    transform.rotation =
                        face_direction(transform.rotation, to - from, TURN_SPEED, dt);
                }
                // The gait follows actual displacement, so a script
                // holding still reads as standing, and an emote goes
                // straight to the animation driver.
                if let Some(mut locomotion) = locomotion {
                    locomotion.moving = tick
                        .position
                        .is_some_and(|[x, z]| (x - from.x).abs() + (z - from.y).abs() > 1e-6);
                    locomotion.running = false;
                }
                if tick.emote.is_some() || tick.pose.is_some() {
                    commands.entity(entity).insert(EmoteRequest {
                        emote: tick.emote,
                        pose: tick.pose,
                    });
                }
                party.apply(&tick.party);
                if !tick.said.is_empty() {
                    // Lines said off-screen — or with no camera — are
                    // dropped this tick; a visible script says again.
                    let at_screen = camera.and_then(|(camera, camera_transform)| {
                        camera
                            .world_to_viewport(camera_transform, transform.translation)
                            .ok()
                    });
                    for said in &tick.said {
                        say(
                            &mut commands,
                            &mut bubbles,
                            SpeechHandles {
                                assets: ui_assets.bubbles.as_deref(),
                                text_assets: ui_assets.text.as_deref(),
                                theme: ui_assets.theme.as_deref(),
                            },
                            &mut actor,
                            at_screen,
                            said,
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Actor script errored, disabling it: {e}");
                commands.entity(entity).insert(ScriptBroken);
            }
        }
    }
}

/// Speech's optional handles, gathered from [`UiAssets`] at the call
/// site so `say` stays under the argument-count lint.
struct SpeechHandles<'a> {
    assets: Option<&'a bubble::BubbleAssets>,
    text_assets: Option<&'a TextAssets>,
    theme: Option<&'a BubbleTheme>,
}

/// Shows a said line in a bubble. Without `at`, the bubble floats above
/// the actor's on-screen point with a downward tail; with it, the line
/// lands at that virtual-screen position, tailless unless the script
/// aims one. `None` — off-screen, or no camera — drops an anchored line
/// for this tick; a still-visible script says it again. Repeating the
/// exact same line keeps the open bubble steady instead of restarting
/// its animation.
fn say(
    commands: &mut Commands,
    bubbles: &mut Query<&mut bubble::SpeechBubble>,
    handles: SpeechHandles<'_>,
    actor: &mut Actor,
    at_screen: Option<Vec2>,
    said: &Said,
) {
    let (Some(assets), Some(text_assets), Some(theme), Some(at_screen)) = (
        handles.assets,
        handles.text_assets,
        handles.theme,
        at_screen,
    ) else {
        return;
    };
    // Repeating the line that's already up keeps that bubble steady
    // instead of restarting its open animation every tick. A bubble
    // that's closing or gone falls through to a fresh spawn.
    let steady = match actor.bubble {
        Some(entity) => bubbles.get(entity).is_ok(),
        None => false,
    };
    if steady && actor.said.as_ref() == Some(said) {
        return;
    }
    if let Some(old) = actor.bubble
        && let Ok(mut old_bubble) = bubbles.get_mut(old)
    {
        bubble::dismiss_bubble(&mut old_bubble);
    }
    let free = said.at.is_some();
    let at = said
        .at
        .map_or(at_screen - Vec2::Y * SAY_HEADROOM_PX, |[x, y]| {
            Vec2::new(x, y)
        });
    let tail = if said.no_tail {
        None
    } else {
        match said.tail {
            Some([x, y]) => Some(Vec2::new(x, y)),
            None if free => None,
            None => Some(Vec2::NEG_Y),
        }
    };
    // Wait-mode bubbles stay until confirmed unless the script also
    // sets a ttl; plain says close themselves after the default time.
    let ttl = if said.wait {
        said.ttl.map(|secs| secs as f32)
    } else {
        Some(said.ttl.map_or(SAY_TTL_SECS, |secs| secs as f32))
    };
    actor.bubble = Some(bubble::spawn_bubble(
        commands,
        assets,
        text_assets,
        theme,
        bubble::BubbleParams {
            text: said.text.clone(),
            at,
            tail,
            free,
            ttl,
            wait: said.wait,
        },
    ));
    actor.said = Some(said.clone());
}

/// Keeps anchored bubbles glued to their speaker. The bubble was placed
/// once at say-time; this re-projects the actor's position every frame
/// so the bubble rides along with the actor's image — including the
/// camera pan, whose sub view `world_to_viewport` already accounts for.
/// Free-placed bubbles are the script's business and stay where they
/// were put.
pub(crate) fn track_anchored_bubbles(
    graphics: Option<Res<SceneGraphics>>,
    roots: Query<&GlobalTransform>,
    actors: Query<(&Actor, &Transform), Without<bubble::SpeechBubble>>,
    mut bubbles: Query<&mut Transform, With<bubble::SpeechBubble>>,
    camera: Query<(&Camera, &GlobalTransform), With<GameCamera>>,
) {
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let root = match graphics {
        Some(graphics) => roots.get(graphics.0).ok(),
        None => None,
    };
    for (actor, transform) in &actors {
        let Some(entity) = actor.bubble else {
            continue;
        };
        if actor.said.as_ref().is_some_and(|said| said.at.is_some()) {
            continue;
        }
        let Some(root) = root else {
            continue;
        };
        let world = root.transform_point(transform.translation);
        let Ok(at) = camera.world_to_viewport(camera_transform, world) else {
            continue;
        };
        let at = at - Vec2::Y * SAY_HEADROOM_PX;
        if let Ok(mut root) = bubbles.get_mut(entity) {
            root.translation = bubble::screen_to_world(at).extend(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::systems::party::Party;
    use crate::text;
    use bevy::asset::{AssetServer, AssetServerMode, UnapprovedPathMode, io::AssetSourceBuilders};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::platform::collections::HashMap;
    use bevy::tasks::{ComputeTaskPool, IoTaskPool, TaskPool};

    use super::*;

    /// A bare server that accepts glTF handle creation without a loader.
    fn test_asset_server() -> AssetServer {
        let mut builders = AssetSourceBuilders::default();
        builders.init_default_source("assets", None);
        let sources = Arc::new(builders.build_sources(false, false));
        AssetServer::new(
            sources,
            AssetServerMode::Unprocessed,
            false,
            UnapprovedPathMode::Forbid,
        )
    }

    fn gltf_with_default_scene() -> Gltf {
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
            named_animations: HashMap::default(),
            source: None,
        }
    }

    fn scripted_actor(text: &str) -> (Actor, Transform) {
        (
            Actor {
                script: Some(ScriptRuntime {
                    script: ActorScript::compile(text).unwrap(),
                    scope: Scope::new(),
                }),
                bubble: None,
                said: None,
                constrained: false,
            },
            Transform::from_xyz(1.0, 0.0, 2.0),
        )
    }

    #[test]
    fn spawn_actors_places_each_actor_facing_its_direction() {
        // `assets.load` queues its file read on Bevy's task pools, which
        // a bare test world doesn't set up.
        IoTaskPool::get_or_init(TaskPool::new);
        ComputeTaskPool::get_or_init(TaskPool::new);
        let mut world = World::new();
        let server = test_asset_server();
        let gltfs = Assets::<Gltf>::default();
        server.register_asset(&gltfs);
        world.insert_resource(server);
        world.insert_resource(gltfs);

        let scene = Scene {
            background: None,
            camera: crate::scene::CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walk_mesh: None,
            teleporters: Vec::new(),
            depth_map: None,
            depth_range: 32.0,
            pan: None,
            script: None,
            actors: vec![crate::scene::Actor {
                model: "models/goblin.glb".into(),
                position: [1.0, 2.0],
                id: None,
                shared: false,
                params: std::collections::BTreeMap::new(),
                script: None,
                facing: None,
                constrained: true,
            }],
        };
        let server = world.resource::<AssetServer>().clone();
        let graphics = crate::systems::scene::SceneGraphics(world.spawn_empty().id());
        world.insert_resource(graphics);
        let mut commands = world.commands();
        let env = crate::scripts::ScriptEnv::detached();
        spawn_actors(
            &mut commands,
            &server,
            &scene,
            Vec2::NEG_Y,
            "scenes/test.scene",
            &env,
            &graphics,
        );
        world.flush();

        let mut actors = world.query::<(&Transform, &Actor)>();
        let (transform, _) = actors.single(&world).unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 0.0, 2.0));
        let front = transform.rotation * Vec3::Z;
        assert!(front.abs_diff_eq(Vec3::NEG_Z, 1e-5));
    }

    #[test]
    fn a_pinned_facing_overrides_the_camera_facing() {
        IoTaskPool::get_or_init(TaskPool::new);
        ComputeTaskPool::get_or_init(TaskPool::new);
        let mut world = World::new();
        let server = test_asset_server();
        let gltfs = Assets::<Gltf>::default();
        server.register_asset(&gltfs);
        world.insert_resource(server);
        world.insert_resource(gltfs);

        let scene = Scene {
            background: None,
            camera: crate::scene::CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walk_mesh: None,
            teleporters: Vec::new(),
            depth_map: None,
            depth_range: 32.0,
            pan: None,
            script: None,
            actors: vec![crate::scene::Actor {
                model: "models/goblin.glb".into(),
                position: [1.0, 2.0],
                id: None,
                shared: false,
                params: std::collections::BTreeMap::new(),
                script: None,
                facing: Some(90.0),
                constrained: true,
            }],
        };
        let server = world.resource::<AssetServer>().clone();
        let graphics = crate::systems::scene::SceneGraphics(world.spawn_empty().id());
        world.insert_resource(graphics);
        let mut commands = world.commands();
        let env = crate::scripts::ScriptEnv::detached();
        spawn_actors(
            &mut commands,
            &server,
            &scene,
            Vec2::NEG_Y,
            "scenes/test.scene",
            &env,
            &graphics,
        );
        world.flush();

        let mut actors = world.query::<&Transform>();
        let transform = actors.single(&world).unwrap();
        // 90° of world yaw turns the +Z front toward +X.
        let front = transform.rotation * Vec3::Z;
        assert!(front.abs_diff_eq(Vec3::X, 1e-5));
    }

    #[test]
    fn the_shipped_actors_run_contract_abiding_scripts() {
        // The demo cast lives in a village scene; every actor that IS
        // shipped must be contract-abiding, and each character's contract
        // holds whenever that character ships.
        let scene = crate::scene::SHIPPED_SCENES
            .iter()
            .map(|name| crate::scene::read_shipped(name))
            .find(|scene| !scene.actors.is_empty())
            .expect("a shipped scene ships actors");

        for actor in &scene.actors {
            match actor.id.as_deref() {
                Some("goblin") => {
                    assert_eq!(actor.model, "models/goblin.glb");
                    assert_eq!(actor.script.as_deref(), Some("scripts/goblin.rhai"));
                }
                Some("tester") => {
                    assert_eq!(actor.model, "models/goblin.glb");
                    assert_eq!(actor.script.as_deref(), Some("scripts/tester.rhai"));
                }
                Some("entrance-chest") | Some("corner-chest") => {
                    assert_eq!(actor.model, "models/chest.glb");
                    assert_eq!(actor.script.as_deref(), Some("scripts/chest.rhai"));
                    assert!(actor.id.is_some(), "a chest without an id shares memory");
                    assert!(!actor.shared, "chests are scene-local");
                    let item = if actor.id.as_deref() == Some("entrance-chest") {
                        "a health potion"
                    } else {
                        "an elixir"
                    };
                    assert_eq!(
                        actor
                            .params
                            .get("item")
                            .and_then(|v| v.clone().into_string().ok()),
                        Some(item.to_owned())
                    );
                }
                other => panic!("shipped actor with unknown id {other:?}"),
            }
        }

        // The shipped scripts compile and answer the actor contract.
        let mut scope = rhai::Scope::new();
        if let Some(goblin) = scene
            .actors
            .iter()
            .find(|a| a.id.as_deref() == Some("goblin"))
        {
            let goblin_script =
                ActorScript::compile(include_str!("../../assets/scripts/goblin.rhai"))
                    .expect("the shipped goblin script must compile");
            // Far from the player the goblin closes in; close by it
            // stays put. The player is a fixed distance north-east of
            // wherever the goblin stands, so the step must close that gap.
            let (goblin_x, goblin_z) = (goblin.position[0], goblin.position[1]);
            let (player_x, player_z) = (goblin_x + 10.0, goblin_z - 10.0);
            let moved = goblin_script
                .update(&mut scope, goblin_x, goblin_z, player_x, player_z, 1.0 / 60.0)
                .unwrap()
                .position
                .expect("the goblin approaches a far player");
            assert!(moved[0] > goblin_x && moved[1] < goblin_z);
            assert_eq!(
                goblin_script
                    .update(
                        &mut scope,
                        goblin_x,
                        goblin_z,
                        goblin_x,
                        goblin_z,
                        1.0 / 60.0,
                    )
                    .unwrap()
                    .position,
                None
            );
        }
        if let Some(entrance) = scene
            .actors
            .iter()
            .find(|a| a.id.as_deref() == Some("entrance-chest"))
        {
            let chest_script =
                ActorScript::compile(include_str!("../../assets/scripts/chest.rhai"))
                    .expect("the shipped chest script must compile");
            // A chest with nobody nearby holds still and stays shut.
            let (chest_x, chest_z) = (entrance.position[0], entrance.position[1]);
            let tick = chest_script
                .update(
                    &mut scope,
                    chest_x,
                    chest_z,
                    chest_x + 50.0,
                    chest_z + 50.0,
                    1.0 / 60.0,
                )
                .unwrap();
            assert_eq!(tick.position, None);
            assert!(tick.emote.is_none());
        }
    }

    #[test]
    fn models_attach_when_their_gltf_loads() {
        let mut world = World::new();
        let mut gltfs = Assets::<Gltf>::default();
        let handle = gltfs.add(gltf_with_default_scene());
        world.insert_resource(gltfs);
        world.spawn((
            Actor {
            script: None,
                bubble: None,
                said: None,
                constrained: false,
            },
            ActorModel(handle),
            Transform::default(),
        ));

        world.run_system_once(attach_actor_models).unwrap();
        world.flush();

        let mut actors = world.query::<&Children>();
        assert_eq!(actors.single(&world).unwrap().len(), 1);
        let mut pending = world.query::<&ActorModel>();
        assert!(pending.iter(&world).next().is_none());
    }

    #[test]
    fn the_script_moves_the_actor_and_turns_its_nose() {
        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        // run_actor_scripts reads the scene's walk mesh; an empty asset
        // collection means no scene is loaded, so nothing bounds actors.
        world.insert_resource(Assets::<Scene>::default());
        world.spawn((Player, Transform::from_xyz(50.0, 0.9, 50.0)));
        let (actor, transform) = scripted_actor("fn on_update(x, z, px, pz, dt) { [x + 1.0, z] }");
        world.spawn((
            actor,
            transform.with_rotation(Quat::from_rotation_arc(Vec3::Z, Vec3::NEG_Z)),
        ));
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.5));

        world.run_system_once(run_actor_scripts).unwrap();

        let mut actors = world.query_filtered::<&Transform, With<Actor>>();
        let transform = actors.single(&world).unwrap();
        assert_eq!(transform.translation, Vec3::new(2.0, 0.0, 2.0));
        // 5 radians of budget turns the front all the way from -Z to +X.
        let front = transform.rotation * Vec3::Z;
        assert!(front.abs_diff_eq(Vec3::X, 1e-4));
    }

    #[test]
    fn a_saying_script_spawns_a_timed_bubble_above_the_actor() {
        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        // run_actor_scripts reads the scene's walk mesh; an empty asset
        // collection means no scene is loaded, so nothing bounds actors.
        world.insert_resource(Assets::<Scene>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<bubble::GradientMaterial>::default());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);
        world.insert_resource(text::test_assets());
        world.insert_resource(bubble::BubbleTheme::default());
        world.spawn((Player, Transform::default()));
        // A targetless camera with pre-seeded render values: eye at the
        // origin looking -Z, 90-degree square perspective. The actor at
        // (0, 0, -6) then projects to the viewport center (160, 120).
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(320, 240),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view =
            Mat4::perspective_rh(core::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        world.spawn((GameCamera, camera, GlobalTransform::IDENTITY));
        let (actor, _) = scripted_actor("fn on_update(x, z, px, pz, dt) { say(\"hi\"); }");
        world.spawn((actor, Transform::from_xyz(0.0, 0.0, -6.0)));

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();

        // The actor's tracked bubble is the spawned one, lifted by the
        // headroom and centered above the projected ground point.
        let mut actors = world.query::<&Actor>();
        let shown = actors.single(&world).unwrap().bubble.unwrap();
        let transform = world.get::<Transform>(shown).unwrap();
        assert!(
            transform
                .translation
                .abs_diff_eq(Vec3::new(0.0, SAY_HEADROOM_PX, 0.0), 1e-4)
        );
    }

    #[test]
    fn saying_again_replaces_the_previous_bubble() {
        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        // run_actor_scripts reads the scene's walk mesh; an empty asset
        // collection means no scene is loaded, so nothing bounds actors.
        world.insert_resource(Assets::<Scene>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<bubble::GradientMaterial>::default());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);
        world.insert_resource(text::test_assets());
        world.insert_resource(bubble::BubbleTheme::default());
        world.spawn((Player, Transform::default()));
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(320, 240),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view =
            Mat4::perspective_rh(core::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        world.spawn((GameCamera, camera, GlobalTransform::IDENTITY));
        let script = ActorScript::compile(
            r#"
            fn on_update(x, z, px, pz, dt) {
                if lines < 2 { lines += 1; say("line" + lines); }
            }
            "#,
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("lines", 0_i64);
        world.spawn((
            Actor {
                script: Some(ScriptRuntime { script, scope }),
                bubble: None,
                said: None,
                constrained: false,
            },
            Transform::from_xyz(0.0, 0.0, -6.0),
        ));

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();
        let mut actors = world.query::<&Actor>();
        let first = actors.single(&world).unwrap().bubble.unwrap();

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();

        let mut actors = world.query::<&Actor>();
        let second = actors.single(&world).unwrap().bubble.unwrap();
        assert_ne!(first, second);
        // The old bubble is on its way out, not despawned outright.
        let old = world.get::<bubble::SpeechBubble>(first).unwrap();
        assert!(!old.is_open());
        // The new one carries the second line.
        let children = world.get::<Children>(second).unwrap();
        let text_entity = children
            .iter()
            .find(|&child| world.get::<Text2d>(child).is_some())
            .unwrap();
        assert_eq!(world.get::<Text2d>(text_entity).unwrap().0, "line2");
    }

    #[test]
    fn repeating_a_line_keeps_the_bubble_steady() {
        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        // run_actor_scripts reads the scene's walk mesh; an empty asset
        // collection means no scene is loaded, so nothing bounds actors.
        world.insert_resource(Assets::<Scene>::default());
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<ColorMaterial>::default());
        world.insert_resource(Assets::<bubble::GradientMaterial>::default());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);
        world.insert_resource(text::test_assets());
        world.insert_resource(bubble::BubbleTheme::default());
        world.spawn((Player, Transform::default()));
        let mut camera = Camera::default();
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(320, 240),
            scale_factor: 1.0,
        });
        camera.computed.clip_from_view =
            Mat4::perspective_rh(core::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        world.spawn((GameCamera, camera, GlobalTransform::IDENTITY));
        let (actor, _) = scripted_actor("fn on_update(x, z, px, pz, dt) { say(\"hi\"); }");
        world.spawn((actor, Transform::from_xyz(0.0, 0.0, -6.0)));

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();
        let mut actors = world.query::<&Actor>();
        let first = actors.single(&world).unwrap().bubble.unwrap();

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();

        let mut actors = world.query::<&Actor>();
        assert_eq!(actors.single(&world).unwrap().bubble, Some(first));
        // Exactly one bubble exists: nothing was dismissed or respawned.
        let mut bubbles = world.query::<&bubble::SpeechBubble>();
        assert_eq!(bubbles.iter(&world).count(), 1);
    }

    #[test]
    fn a_broken_script_is_disabled_not_a_crash() {
        let mut world = World::new();
        world.insert_resource(Party::default());
        world.insert_resource(Time::<()>::default());
        // run_actor_scripts reads the scene's walk mesh; an empty asset
        // collection means no scene is loaded, so nothing bounds actors.
        world.insert_resource(Assets::<Scene>::default());
        world.spawn((Player, Transform::default()));
        let (actor, transform) = scripted_actor("fn on_update(x, z, px, pz, dt) { 7 }");
        let actor_entity = world.spawn((actor, transform)).id();

        world.run_system_once(run_actor_scripts).unwrap();
        world.flush();

        assert_eq!(
            world.get::<Transform>(actor_entity).unwrap().translation,
            Vec3::new(1.0, 0.0, 2.0)
        );
        assert!(world.get::<ScriptBroken>(actor_entity).is_some());
    }

    /// Anchored bubbles re-project their speaker every frame; free-
    /// placed bubbles stay where the script put them.
    #[test]
    fn anchored_bubbles_track_their_speaker() {
        use bevy::camera::RenderTarget;
        use crate::scripts::Said;
        use bevy::asset::RenderAssetUsages;
        use bevy::render::camera::camera_system;
        use bevy::render::render_resource::{
            Extent3d, TextureDimension, TextureFormat,
        };

        let mut world = World::new();
        let mut images = Assets::<Image>::default();
        let target = images.add(Image::new_fill(
            Extent3d {
                width: 320,
                height: 240,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        world.insert_resource(images);
        world.init_resource::<bevy::render::texture::ManualTextureViews>();
        world.init_resource::<bevy::ecs::message::Messages<bevy::window::WindowResized>>();
        world.init_resource::<bevy::ecs::message::Messages<bevy::window::WindowCreated>>();
        world.init_resource::<bevy::ecs::message::Messages<
            bevy::window::WindowScaleFactorChanged,
        >>();
        world.init_resource::<bevy::ecs::message::Messages<
            bevy::asset::AssetEvent<Image>,
        >>();
        let camera_transform =
            Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y);
        world.spawn((
            GameCamera,
            Camera::default(),
            RenderTarget::Image(target.into()),
            GlobalTransform::from(camera_transform),
            Projection::Perspective(PerspectiveProjection {
                fov: 45.0_f32.to_radians(),
                aspect_ratio: 320.0 / 240.0,
                ..default()
            }),
        ));
        // world_to_viewport needs the camera's computed target info.
        world
            .run_system_once(camera_system)
            .map(|_: ()| ())
            .unwrap();

        // The speaker sits under an identity root, so world == local.
        let root = world.spawn_empty().id();
        world.spawn((crate::systems::scene::SceneGraphics(root),));
        world.entity_mut(root).insert(GlobalTransform::IDENTITY);

        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<ColorMaterial>>();
        world.init_resource::<Assets<crate::systems::bubble::GradientMaterial>>();
        world.init_resource::<bubble::BubbleTheme>();
        world.insert_resource(crate::text::test_assets());
        let assets = bubble::test_assets(&mut world);
        world.insert_resource(assets);

        let mut anchored_bubble = Entity::PLACEHOLDER;
        let mut free_bubble = Entity::PLACEHOLDER;
        world.resource_scope::<crate::text::TextAssets, _>(|world, text_assets| {
            let text_assets = text_assets.into_inner();
            let assets = world.resource::<bubble::BubbleAssets>().clone();
            let theme = *world.resource::<bubble::BubbleTheme>();
            anchored_bubble = bubble::spawn_bubble(
                &mut world.commands(),
                &assets,
                &text_assets,
                &theme,
                bubble::BubbleParams {
                    text: "yay".into(),
                    at: Vec2::new(140.0, 100.0),
                    tail: Some(Vec2::NEG_Y),
                    free: false,
                    ttl: None,
                    wait: false,
                },
            );
            free_bubble = bubble::spawn_bubble(
                &mut world.commands(),
                &assets,
                &text_assets,
                &theme,
                bubble::BubbleParams {
                    text: "menu".into(),
                    at: Vec2::new(160.0, 40.0),
                    tail: None,
                    free: true,
                    ttl: None,
                    wait: false,
                },
            );
        });
        world.flush();

        let speaker_at = Vec3::new(1.0, 0.0, 1.0);
        world.spawn((
            Actor {
                script: None,
                bubble: Some(anchored_bubble),
                said: Some(Said {
                    at: None,
                    ..default()
                }),
                constrained: false,
            },
            Transform::from_translation(speaker_at),
            GlobalTransform::from(Transform::from_translation(speaker_at)),
        ));
        world.spawn((
            Actor {
                script: None,
                bubble: Some(free_bubble),
                said: Some(Said {
                    at: Some([160.0, 40.0]),
                    ..default()
                }),
                constrained: false,
            },
            Transform::default(),
            GlobalTransform::default(),
        ));

        world.run_system_once(track_anchored_bubbles).unwrap();

        // The anchored bubble sits over the speaker; the free one stays.
        let (camera, camera_transform) = world
            .query::<(&Camera, &GlobalTransform)>()
            .single(&world)
            .unwrap();
        let expected = camera
            .world_to_viewport(camera_transform, speaker_at)
            .unwrap()
            - Vec2::Y * SAY_HEADROOM_PX;
        let anchored = world.get::<Transform>(anchored_bubble).unwrap();
        assert_eq!(
            anchored.translation,
            bubble::screen_to_world(expected).extend(0.0)
        );
        let free = world.get::<Transform>(free_bubble).unwrap();
        assert_eq!(
            free.translation,
            bubble::screen_to_world(Vec2::new(160.0, 40.0)).extend(0.0)
        );
    }
}
