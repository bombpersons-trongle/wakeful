//! Scene loading and application: the systems that turn a loaded
//! `assets/scenes/*.scene` file into live entities and resources.

use bevy::prelude::*;
use rhai::Scope;

use crate::input::InputManager;
use crate::movement::facing_rotation;
use crate::scene::Scene;
use crate::screen;
use crate::scripts::{SceneScript as SceneScriptRuntime, ScriptBroken, ScriptEnv};
use crate::systems::actor::{self, Actor};
use crate::systems::bubble::SpeechBubble;
use crate::systems::depth_card;
use crate::systems::party::Party;
use crate::systems::player;
use crate::systems::ui::{UiApi, UiWindow, close_all};
use crate::world_state::WorldState;
use crate::{CurrentScene, GameCameraQuery, Ground, PendingTeleport, Player, PlayerModel, PlayerSpawn, SceneApplied, TeleporterArmed};

/// The scene file the game loads; the editor saves back to this path via
/// the copy stored on `CurrentScene`.
const SCENE_PATH: &str = "scenes/devroom.scene";

pub fn load_scene(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(CurrentScene {
        handle: assets.load(SCENE_PATH),
        path: SCENE_PATH.to_string(),
    });
    commands.insert_resource(SceneApplied(false));
}

/// The scene's script runtime, spawned when the scene applies and
/// despawned with it. `entered` gates the one-shot `on_enter`.
#[derive(Component)]
pub(crate) struct SceneScript {
    runtime: SceneScriptRuntime,
    scope: Scope<'static>,
    entered: bool,
}

/// Tears the old scene down when a teleporter was touched: despawns
/// everything it brought (depth card, actors, bubbles) and points
/// `CurrentScene` at the destination file. The destination's application
/// — camera pose, background card, fresh player — then goes through
/// `apply_scene` like any scene load.
#[allow(clippy::too_many_arguments)]
pub fn transition_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    pending: Option<Res<PendingTeleport>>,
    current: Option<ResMut<CurrentScene>>,
    applied: Option<ResMut<SceneApplied>>,
    mut party: ResMut<Party>,
    cards: Query<Entity, Or<(With<depth_card::DepthCard>, With<depth_card::DepthCardPending>)>>,
    actors: Query<Entity, With<Actor>>,
    bubbles: Query<Entity, With<SpeechBubble>>,
    mut scene_scripts: Query<(Entity, &mut SceneScript, Option<&ScriptBroken>)>,
    ui: Res<UiApi>,
    ui_windows: Query<(Entity, &UiWindow)>,
) {
    let Some(pending) = pending else {
        return;
    };
    // The player is NOT despawned: it's a persistent view of the party
    // leader, and rebuilding it would flash the placeholder cone and
    // reload the model every transition. apply_scene repositions it.
    for entity in cards
        .iter()
        .chain(actors.iter())
        .chain(bubbles.iter())
    {
        commands.entity(entity).despawn();
    }
    // UI windows are scene-agnostic names over live entities: scene
    // changes close them all; a still-open world menu re-declares
    // itself on the next tick.
    close_all(
        &ui,
        &mut commands,
        ui_windows.iter().map(|(entity, _)| entity),
    );

    for (entity, mut script, broken) in &mut scene_scripts {
        let SceneScript { runtime, scope, .. } = &mut *script;
        // Broken scripts already warned once; their on_exit is skipped
        // rather than risking a second failure on teardown.
        if broken.is_none() {
            match runtime.exit(scope) {
                Ok(changes) => party.apply(&changes),
                Err(e) => warn!("Scene script errored in on_exit: {e}"),
            }
        }
        commands.entity(entity).despawn();
    }
    // The old scene's model queue must not dress the new scene's player.
    commands.remove_resource::<PlayerModel>();
    if let Some(mut current) = current {
        current.handle = assets.load(pending.target.clone());
        current.path = pending.target.clone();
    }
    if let Some(mut applied) = applied {
        applied.0 = false;
    }
    commands.insert_resource(PlayerSpawn(pending.arrival));
    commands.remove_resource::<PendingTeleport>();
}

/// Applies the scene once its file has loaded: camera pose, background
/// card, and the player — repositioned if it survives from the last
/// scene, spawned fresh (placeholder cone; the party dresses it) on
/// first load.
#[allow(clippy::too_many_arguments)]
pub fn apply_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    applied: Option<ResMut<SceneApplied>>,
    spawn: Option<Res<PlayerSpawn>>,
    mut players: Query<&mut Transform, With<Player>>,
    input: Res<InputManager>,
    ui: Res<UiApi>,
    state: Res<WorldState>,
    battle: Res<crate::battle::BattleHandle>,
    graphics: Res<SceneGraphics>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cameras: GameCameraQuery,
) {
    let (Some(current), Some(mut applied)) = (current, applied) else {
        return;
    };
    if applied.0 {
        return;
    }
    let Some(scene) = scenes.get(&current.handle) else {
        return;
    };

    let Ok((mut transform, mut projection)) = cameras.single_mut() else {
        return;
    };
    *transform = Transform::from_translation(scene.camera.position.into())
        .looking_at(scene.camera.target.into(), Vec3::Y);
    *projection = Projection::Perspective(PerspectiveProjection {
        fov: scene.camera.fov_degrees.to_radians(),
        aspect_ratio: screen::GAME_WIDTH as f32 / screen::GAME_HEIGHT as f32,
        ..default()
    });

    // The background is real geometry: a depth card in the scene
    // camera's pass, built once its images load. A background without its
    // required depth map is an authoring error; the placeholder ground
    // stands in (sync_ground) rather than leaving a void.
    match (&scene.background, &scene.depth_map) {
        (Some(path), Some(depth_path)) => {
            commands.spawn(depth_card::DepthCardPending {
                pose: scene.camera,
                depth_range: scene.depth_range,
                depth: assets.load(depth_path),
                background: assets.load(path),
            });
        }
        (Some(_), None) => warn!(
            "scene {}: background without a depth map — the placeholder ground stands in",
            current.path
        ),
        _ => {}
    }

    // Every scene application starts a fresh player at the scene's chosen
    // spot: the world origin on first load, the teleporter's arrival point
    // after a transition. The player starts facing screen-up (away from
    // the camera).
    let at = spawn.map(|spawn| spawn.0).unwrap_or(Vec2::ZERO);
    // The player persists across scenes; on first load there isn't one
    // yet, and it starts as the placeholder cone for the party to dress.
    match players.single_mut() {
        Ok(mut transform) => {
            transform.translation = Vec3::new(at.x, player::PLAYER_Y, at.y);
            transform.rotation = facing_rotation(scene.camera_forward());
        }
        Err(_) => {
            let player = player::spawn_player(
                &mut commands,
                &mut meshes,
                &mut materials,
                at,
                scene.camera_forward(),
            );
            commands.entity(graphics.0).add_child(player);
        }
    }
    commands.remove_resource::<PlayerSpawn>();

    // A teleporter already under the player on arrival must not fire
    // until the player leaves it and re-enters.
    commands.insert_resource(TeleporterArmed(
        scene
            .teleporters
            .iter()
            .map(|t| !t.contains(at.x, at.y))
            .collect(),
    ));

    let env = ScriptEnv::new(input.handle(), ui.clone(), state.clone(), battle.clone());
    let scene_path = current.path.as_str();
    actor::spawn_actors(
        &mut commands,
        &assets,
        scene,
        scene.camera_forward(),
        scene_path,
        &env,
        &graphics,
    );

    // The scene's script, if the file declares one; run_scene_scripts
    // fires its on_enter on the first tick after this.
    if let Some(runtime) = scene
        .script
        .as_deref()
        .and_then(|path| SceneScriptRuntime::load(path, env.clone().with_store(path)))
    {
        commands.spawn((SceneScript {
            runtime,
            scope: Scope::new(),
            entered: false,
        },));
    }

    applied.0 = true;
}

/// Runs the scene's script hooks on the fixed tick: `on_enter` once on
/// the first tick after application, then `on_update` every tick. A
/// runtime error disables the script with one warning.
pub(crate) fn run_scene_scripts(
    mut commands: Commands,
    time: Res<Time>,
    mut party: ResMut<Party>,
    players: Query<&Transform, With<Player>>,
    mut scripts: Query<(Entity, &mut SceneScript), Without<ScriptBroken>>,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let (player_x, player_z) = (player.translation.x, player.translation.z);
    let dt = time.delta_secs();
    for (entity, mut script) in &mut scripts {
        let SceneScript {
            runtime,
            scope,
            entered,
        } = &mut *script;
        if !*entered {
            *entered = true;
            match runtime.enter(scope, player_x, player_z) {
                Ok(changes) => party.apply(&changes),
                Err(e) => {
                    warn!("Scene script errored in on_enter, disabling it: {e}");
                    commands.entity(entity).insert(ScriptBroken);
                    continue;
                }
            }
        }
        match runtime.update(scope, player_x, player_z, dt) {
            Ok(changes) => party.apply(&changes),
            Err(e) => {
                warn!("Scene script errored in on_update, disabling it: {e}");
                commands.entity(entity).insert(ScriptBroken);
            }
        }
    }
}

/// Strips a `#SceneN` sub-asset suffix from a character-model path: the
/// model's default scene is used regardless. Scene files written when the
/// suffix was part of the contract keep loading.
pub(crate) fn gltf_asset_path(path: &str) -> String {
    match path.split_once('#') {
        Some((base, _)) => base.to_owned(),
        None => path.to_owned(),
    }
}

/// Hides the placeholder ground while the scene shows a depth card, and
/// brings it back when there is no card (no background, or a background
/// missing its required depth map). Runs every frame so live editor
/// edits react immediately.
pub fn sync_ground(
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut grounds: Query<&mut Visibility, With<Ground>>,
) {
    let has_card = current
        .as_ref()
        .and_then(|c| scenes.get(&c.handle))
        .is_some_and(|scene| scene.background.is_some() && scene.depth_map.is_some());
    let Ok(mut visibility) = grounds.single_mut() else {
        return;
    };
    *visibility = if has_card {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
}

/// Root of every scene-context graphic: the background photo, the
/// ground, the scene actors, and the field player. A battle folds the
/// whole tree away with one visibility toggle instead of walking
/// per-entity roots — per-entity suspend/resume churn is what let
/// hidden subtrees leak back into view.
#[derive(Resource, Clone, Copy)]
pub(crate) struct SceneGraphics(pub(crate) Entity);

/// Spawns the two context roots once at boot: scene graphics and (over
/// in battle.rs) battle graphics. Nothing else ever re-parents under
/// them.
pub(crate) fn setup_graphics(mut commands: Commands) {
    let scene = commands
        .spawn((Name::new("scene graphics"), Visibility::default(), Transform::IDENTITY))
        .id();
    commands.insert_resource(SceneGraphics(scene));
    crate::battle::setup_graphics(&mut commands);
}

/// `OnEnter(Battle)`: the scene steps aside — one toggle on the scene
/// graphics root folds the card, ground, actors, and player away while
/// the arena has the screen. Every managed model root defers to its
/// ancestors (`Inherited` when revealed, `Hidden` before the first
/// driven step), so a parent toggle is the whole suspension.
pub(crate) fn suspend_scene(mut commands: Commands, graphics: Res<SceneGraphics>) {
    commands.entity(graphics.0).insert(Visibility::Hidden);
}

/// `OnEnter(Scene)`: fires at the transition's covered point — the
/// scene comes back exactly as it was.
pub(crate) fn resume_scene(
    mut commands: Commands,
    // The init state transition fires OnEnter(Scene) before Startup has
    // run, so the graphics root may not exist yet; the hook no-ops.
    graphics: Option<Res<SceneGraphics>>,
) {
    let Some(graphics) = graphics else {
        return;
    };
    commands.entity(graphics.0).insert(Visibility::Visible);
}


#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy::asset::{AssetServer, AssetServerMode, UnapprovedPathMode, io::AssetSourceBuilders};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::tasks::{ComputeTaskPool, IoTaskPool, TaskPool};

    use crate::GameCamera;
    use crate::scene::{CameraPose, Teleporter};

    use super::*;

    /// An `AssetServer` for tests: real handle resolution, no file reads
    /// we care about (loads of nonexistent files fail silently off-thread).
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

    fn test_scene(background: Option<&str>, depth_map: Option<&str>) -> Scene {
        Scene {
            background: background.map(str::to_string),
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walkable: None,
            teleporters: Vec::new(),
            depth_map: depth_map.map(str::to_string),
            depth_range: 32.0,
            script: None,
            actors: Vec::new(),
        }
    }

    fn world_with_scene(background: Option<&str>, depth_map: Option<&str>) -> (World, Entity) {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let handle = assets.add(test_scene(background, depth_map));
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        let ground = world.spawn((Ground, Visibility::default())).id();
        (world, ground)
    }

    #[test]
    fn gltf_paths_drop_the_scene_suffix() {
        assert_eq!(gltf_asset_path("elf.glb"), "elf.glb");
        assert_eq!(
            gltf_asset_path("models/hero.gltf#Scene0"),
            "models/hero.gltf"
        );
        assert_eq!(gltf_asset_path("models/hero.glb#Scene0"), "models/hero.glb");
    }

    #[test]
    fn ground_hides_while_a_depth_card_is_set() {
        let (mut world, ground) = world_with_scene(
            Some("backgrounds/room.png"),
            Some("backgrounds/room_depth.png"),
        );
        world.run_system_once(sync_ground).unwrap();
        assert_eq!(world.get::<Visibility>(ground), Some(&Visibility::Hidden));
    }

    /// A background without its required depth map is an authoring
    /// error; the placeholder ground stands in rather than a void.
    #[test]
    fn ground_stands_in_when_the_depth_map_is_missing() {
        let (mut world, ground) = world_with_scene(Some("backgrounds/room.png"), None);
        world.run_system_once(sync_ground).unwrap();
        assert_eq!(world.get::<Visibility>(ground), Some(&Visibility::Visible));
    }

    #[test]
    fn ground_returns_when_the_background_is_cleared() {
        let (mut world, ground) = world_with_scene(None, None);
        world.entity_mut(ground).insert(Visibility::Hidden);
        world.run_system_once(sync_ground).unwrap();
        assert_eq!(world.get::<Visibility>(ground), Some(&Visibility::Visible));
    }

    #[test]
    fn ground_stays_visible_without_a_scene() {
        let mut world = World::new();
        world.insert_resource(Assets::<Scene>::default());
        let ground = world.spawn((Ground, Visibility::default())).id();
        world.run_system_once(sync_ground).unwrap();
        assert_eq!(world.get::<Visibility>(ground), Some(&Visibility::Visible));
    }

    fn world_for_transition() -> World {
        // `server.load` queues its file read on Bevy's task pools, which a
        // bare test world doesn't set up.
        IoTaskPool::get_or_init(TaskPool::new);
        ComputeTaskPool::get_or_init(TaskPool::new);
        let mut world = World::new();
        let server = test_asset_server();
        let mut assets = Assets::<Scene>::default();
        server.register_asset(&assets);
        world.insert_resource(server);
        world.insert_resource(crate::systems::ui::UiApi::new());
        let handle = assets.add(test_scene(
            Some("backgrounds/room1.png"),
            Some("backgrounds/room1_depth.png"),
        ));
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        world.insert_resource(SceneApplied(true));
        world.insert_resource(crate::systems::party::Party::default());
        world.insert_resource(PlayerModel(Handle::default()));
        world.insert_resource(PendingTeleport {
            target: "scenes/room2.scene".to_string(),
            arrival: Vec2::new(3.0, 4.0),
        });
        world.spawn((depth_card::DepthCard, Transform::default()));
        world.spawn((Player, Transform::default()));
        world
    }

    #[test]
    fn transition_despawns_the_old_scene_and_swaps_in_the_target() {
        let mut world = world_for_transition();
        world.run_system_once(transition_scene).unwrap();
        world.flush();

        let mut cards = world.query::<&depth_card::DepthCard>();
        let mut players = world.query::<&Player>();
        assert_eq!(cards.iter(&world).count(), 0);
        // The player persists across scenes (a view of the party leader).
        assert_eq!(players.iter(&world).count(), 1);
        assert!(world.get_resource::<PlayerModel>().is_none());
        assert!(world.get_resource::<PendingTeleport>().is_none());

        let current = world.resource::<CurrentScene>();
        assert_eq!(current.path, "scenes/room2.scene");
        let server = world.resource::<AssetServer>();
        assert_eq!(current.handle, server.load("scenes/room2.scene"));
        assert!(!world.resource::<SceneApplied>().0);
        assert_eq!(world.resource::<PlayerSpawn>().0, Vec2::new(3.0, 4.0));
    }

    /// A scene whose teleporter covers exactly the point (3, 4).
    fn covering_scene() -> Scene {
        Scene {
            background: None,
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walkable: None,
            teleporters: vec![Teleporter {
                position: [3.0, 4.0],
                size: [1.0, 1.0],
                target: "scenes/elsewhere.scene".into(),
                arrival: [0.0, 0.0],
            }],
            depth_map: None,
            depth_range: 32.0,
            script: None,
            actors: Vec::new(),
        }
    }

    fn world_for_apply(scene: Scene, player_spawn: Option<Vec2>) -> World {
        let mut world = World::new();
        world.insert_resource(crate::input::InputManager::standard());
        world.insert_resource(crate::systems::ui::UiApi::new());
        world.insert_resource(crate::systems::party::Party::default());
        world.insert_resource(crate::world_state::WorldState::default());
        world.insert_resource(crate::battle::BattleHandle::new());
        let graphics = SceneGraphics(world.spawn_empty().id());
        world.insert_resource(graphics);
        let server = test_asset_server();
        let mut assets = Assets::<Scene>::default();
        server.register_asset(&assets);
        world.insert_resource(server);
        let handle = assets.add(scene);
        world.insert_resource(assets);
        let mut images = Assets::<Image>::default();
        world.insert_resource(screen::GameImage(images.add(Image::default())));
        world.insert_resource(Assets::<Mesh>::default());
        world.insert_resource(Assets::<StandardMaterial>::default());
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene".to_string(),
        });
        world.insert_resource(SceneApplied(false));
        if let Some(at) = player_spawn {
            world.insert_resource(PlayerSpawn(at));
        }
        world.spawn((GameCamera, Transform::default(), Projection::default()));
        world
    }

    #[test]
    fn a_surviving_player_is_repositioned_and_keeps_its_body() {
        // The player persists across scenes: apply must move it instead
        // of duplicating it, and whatever body the party attached stays.
        let mut world = world_for_apply(test_scene(None, None), Some(Vec2::new(3.0, 4.0)));
        let player = world.spawn((Player, Transform::default())).id();
        world.resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
            world.resource_scope(|world, mut materials: Mut<Assets<StandardMaterial>>| {
                let mut commands = world.commands();
                crate::systems::player::spawn_placeholder_body(
                    &mut commands,
                    player,
                    &mut meshes,
                    &mut materials,
                );
            });
        });
        world.flush();

        world.run_system_once(apply_scene).unwrap();
        world.flush();

        let mut players = world.query_filtered::<Entity, With<Player>>();
        assert_eq!(players.iter(&world).count(), 1, "no duplicate player");
        let mut transforms = world.query_filtered::<&Transform, With<Player>>();
        let transform = transforms.single(&world).unwrap();
        assert_eq!(transform.translation.xz(), Vec2::new(3.0, 4.0));
        let body = world
            .query_filtered::<&ChildOf, With<crate::systems::player::PlaceholderBody>>()
            .single(&world)
            .unwrap();
        assert_eq!(body.parent(), player, "the attached body survives");
    }

    #[test]
    fn apply_scene_spawns_the_player_at_the_arrival_point() {
        let mut world = world_for_apply(test_scene(None, None), Some(Vec2::new(3.0, 4.0)));
        world.run_system_once(apply_scene).unwrap();
        world.flush();

        let mut players = world.query_filtered::<&Transform, With<Player>>();
        let transform = players.single(&world).unwrap();
        assert_eq!(transform.translation.xz(), Vec2::new(3.0, 4.0));
        assert!(world.get_resource::<PlayerSpawn>().is_none());
        assert!(world.resource::<SceneApplied>().0);
    }

    #[test]
    fn the_first_scene_spawns_the_player_at_the_origin() {
        let mut world = world_for_apply(test_scene(None, None), None);
        world.run_system_once(apply_scene).unwrap();
        world.flush();

        let mut players = world.query_filtered::<&Transform, With<Player>>();
        let transform = players.single(&world).unwrap();
        assert_eq!(transform.translation.xz(), Vec2::ZERO);
    }

    #[test]
    fn arriving_inside_a_trigger_starts_it_disarmed() {
        let mut world = world_for_apply(covering_scene(), Some(Vec2::new(3.0, 4.0)));
        world.run_system_once(apply_scene).unwrap();
        world.flush();
        assert!(!world.resource::<TeleporterArmed>().0[0]);
    }

    #[test]
    fn arriving_clear_starts_the_trigger_armed() {
        let mut world = world_for_apply(covering_scene(), Some(Vec2::new(0.0, 0.0)));
        world.run_system_once(apply_scene).unwrap();
        world.flush();
        assert!(world.resource::<TeleporterArmed>().0[0]);
    }

    #[test]
    fn the_first_tick_fires_on_enter_then_on_update_each_tick() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(crate::systems::party::Party::default());
        world.spawn((Player, Transform::from_xyz(3.0, 0.0, 4.0)));
        let runtime = SceneScriptRuntime::compile(
            r"
            fn on_enter(px, pz) { enters += 1; entered_x = px; }
            fn on_update(px, pz, dt) { updates += 1; }
            ",
        )
        .unwrap();
        let mut scope = Scope::new();
        scope.push("enters", 0_i64);
        scope.push("entered_x", 0.0_f64);
        scope.push("updates", 0_i64);
        world.spawn((SceneScript {
            runtime,
            scope,
            entered: false,
        },));

        world.run_system_once(run_scene_scripts).unwrap();
        world.run_system_once(run_scene_scripts).unwrap();

        let mut scripts = world.query::<&SceneScript>();
        let script = scripts.single(&world).unwrap();
        assert!(script.entered);
        assert_eq!(script.scope.get_value::<i64>("enters"), Some(1));
        assert_eq!(script.scope.get_value::<f64>("entered_x"), Some(3.0));
        assert_eq!(script.scope.get_value::<i64>("updates"), Some(2));
    }

    #[test]
    fn transition_runs_on_exit_and_drops_the_scene_script() {
        let mut world = world_for_transition();
        world.insert_resource(crate::systems::party::Party::default());
        let runtime = SceneScriptRuntime::compile("fn on_exit() { }").unwrap();
        let script_entity = world
            .spawn((SceneScript {
                runtime,
                scope: Scope::new(),
                entered: true,
            },))
            .id();

        world.run_system_once(transition_scene).unwrap();
        world.flush();

        // The exit hook ran before the despawn (no panic) and the
        // script is gone with the rest of the scene.
        assert!(world.get_entity(script_entity).is_err());
    }

    #[test]
    fn a_scene_script_which_errors_is_disabled_not_spammed() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(crate::systems::party::Party::default());
        world.spawn((Player, Transform::default()));
        let runtime = SceneScriptRuntime::compile("fn on_update(px, pz, dt) { bogus(); }").unwrap();
        let entity = world
            .spawn((SceneScript {
                runtime,
                scope: Scope::new(),
                entered: false,
            },))
            .id();

        world.run_system_once(run_scene_scripts).unwrap();
        world.flush();

        assert!(world.get::<ScriptBroken>(entity).is_some());
        // The tick query filters it out from then on.
        world.run_system_once(run_scene_scripts).unwrap();
    }
}
