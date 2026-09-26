//! Scene loading and application: the systems that turn a loaded
//! `assets/scenes/*.scene` file into live entities and resources.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use rhai::Scope;

use crate::input::InputManager;
use crate::movement::facing_rotation;
use crate::scene::Scene;
use crate::screen;
use crate::scripts::{SceneScript as SceneScriptRuntime, ScriptBroken};
use crate::systems::actor::{self, Actor};
use crate::systems::party::Party;
use crate::systems::player;
use crate::systems::ui::{UiApi, UiWindow, close_all};
use crate::{
    BackgroundCamera, BackgroundSprite, CurrentScene, GameCameraQuery, Ground, PendingTeleport,
    Player, PlayerModel, PlayerSpawn, SceneApplied, TeleporterArmed,
};

/// Camera layer that draws the pre-rendered background image.
const BG_LAYER: usize = 2;

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
/// everything it brought (background camera, background sprite, player)
/// and points `CurrentScene` at the destination file. The destination's
/// application — camera pose, background, fresh player — then goes
/// through `apply_scene` like any scene load.
#[allow(clippy::too_many_arguments)]
pub fn transition_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    pending: Option<Res<PendingTeleport>>,
    current: Option<ResMut<CurrentScene>>,
    applied: Option<ResMut<SceneApplied>>,
    mut party: ResMut<Party>,
    backgrounds: Query<Entity, With<BackgroundSprite>>,
    bg_cameras: Query<Entity, With<BackgroundCamera>>,
    actors: Query<Entity, With<Actor>>,
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
    for entity in backgrounds
        .iter()
        .chain(bg_cameras.iter())
        .chain(actors.iter())
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
/// layer, and the player — repositioned if it survives from the last
/// scene, spawned fresh (placeholder cone; the party dresses it) on
/// first load.
#[allow(clippy::too_many_arguments)]
pub fn apply_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    game_image: Res<screen::GameImage>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    applied: Option<ResMut<SceneApplied>>,
    spawn: Option<Res<PlayerSpawn>>,
    mut players: Query<&mut Transform, With<Player>>,
    input: Res<InputManager>,
    ui: Res<UiApi>,
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

    // The background camera always clears the image (with the global clear
    // color) so the 3D camera can draw over it without clearing.
    commands.spawn((
        BackgroundCamera,
        Camera2d,
        Camera {
            order: 0,
            ..default()
        },
        Msaa::Off, // must be explicit: the default is 4x, which would
        // mismatch the single-sampled game image once a background
        // sprite gives this camera a depth-bearing 2d pass.
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(BG_LAYER),
    ));

    if let Some(path) = &scene.background {
        spawn_background(&mut commands, &assets, path);
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
        Err(_) => player::spawn_player(
            &mut commands,
            &mut meshes,
            &mut materials,
            at,
            scene.camera_forward(),
        ),
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

    actor::spawn_actors(
        &mut commands,
        &assets,
        scene,
        scene.camera_forward(),
        &input.handle(),
        &ui,
    );

    // The scene's script, if the file declares one; run_scene_scripts
    // fires its on_enter on the first tick after this.
    if let Some(runtime) = scene
        .script
        .as_deref()
        .and_then(|path| SceneScriptRuntime::load(path, &input.handle(), &ui))
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

/// Spawns the scene's background image on its dedicated layer. Also used
/// by the editor when the background path changes at runtime.
pub(crate) fn spawn_background(commands: &mut Commands, assets: &AssetServer, path: &str) {
    commands.spawn((
        BackgroundSprite,
        Sprite {
            image: assets.load(path.to_owned()),
            // Backgrounds are authored at the virtual resolution.
            custom_size: Some(Vec2::new(
                screen::GAME_WIDTH as f32,
                screen::GAME_HEIGHT as f32,
            )),
            ..default()
        },
        RenderLayers::layer(BG_LAYER),
    ));
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

/// Hides the placeholder ground while the scene shows a pre-rendered
/// background, and brings it back when the background is cleared. Runs
/// every frame so live editor edits react immediately.
pub fn sync_ground(
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    mut grounds: Query<&mut Visibility, With<Ground>>,
) {
    let has_background = current
        .as_ref()
        .and_then(|c| scenes.get(&c.handle))
        .is_some_and(|scene| scene.background.is_some());
    let Ok(mut visibility) = grounds.single_mut() else {
        return;
    };
    *visibility = if has_background {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
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

    fn test_scene(background: Option<&str>) -> Scene {
        Scene {
            background: background.map(str::to_string),
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walkable: None,
            teleporters: Vec::new(),
            script: None,
            actors: Vec::new(),
        }
    }

    fn world_with_scene(background: Option<&str>) -> (World, Entity) {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let handle = assets.add(test_scene(background));
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
    fn ground_hides_while_a_background_is_set() {
        let (mut world, ground) = world_with_scene(Some("backgrounds/room.png"));
        world.run_system_once(sync_ground).unwrap();
        assert_eq!(world.get::<Visibility>(ground), Some(&Visibility::Hidden));
    }

    #[test]
    fn ground_returns_when_the_background_is_cleared() {
        let (mut world, ground) = world_with_scene(None);
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
        let handle = assets.add(test_scene(Some("backgrounds/room1.png")));
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
        world.spawn((BackgroundSprite, Sprite::default()));
        world.spawn((BackgroundCamera, Camera2d));
        world.spawn((Player, Transform::default()));
        world
    }

    #[test]
    fn transition_despawns_the_old_scene_and_swaps_in_the_target() {
        let mut world = world_for_transition();
        world.run_system_once(transition_scene).unwrap();
        world.flush();

        let mut sprites = world.query::<&BackgroundSprite>();
        let mut cams = world.query::<&BackgroundCamera>();
        let mut players = world.query::<&Player>();
        assert_eq!(sprites.iter(&world).count(), 0);
        assert_eq!(cams.iter(&world).count(), 0);
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
            script: None,
            actors: Vec::new(),
        }
    }

    fn world_for_apply(scene: Scene, player_spawn: Option<Vec2>) -> World {
        let mut world = World::new();
        world.insert_resource(crate::input::InputManager::standard());
        world.insert_resource(crate::systems::ui::UiApi::new());
        world.insert_resource(crate::systems::party::Party::default());
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
        let mut world = world_for_apply(test_scene(None), Some(Vec2::new(3.0, 4.0)));
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
        let mut world = world_for_apply(test_scene(None), Some(Vec2::new(3.0, 4.0)));
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
        let mut world = world_for_apply(test_scene(None), None);
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
