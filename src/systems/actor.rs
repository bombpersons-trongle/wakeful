//! Scene actors: a positioned model per actor, optionally driven by a
//! Rhai script (see [`crate::scripts`] for the contract).

use bevy::gltf::Gltf;
use bevy::prelude::*;
use rhai::Scope;

use crate::Player;
use crate::editor::assets_root;
use crate::movement::{TURN_SPEED, face_direction, facing_rotation};
use crate::scene::Scene;
use crate::scripts::CompiledScript;
use crate::systems::scene::gltf_asset_path;

/// Marks a scene actor. The optional script runtime is disabled once it
/// errors, so a broken file can't spam warnings every tick.
#[derive(Component)]
pub struct Actor {
    script: Option<ScriptRuntime>,
}

/// A compiled script and its persistent variable scope.
struct ScriptRuntime {
    script: CompiledScript,
    scope: Scope<'static>,
}

/// glTF model queued for the actor; removed once attached as a child.
#[derive(Component)]
pub(crate) struct ActorModel(Handle<Gltf>);

/// Marks an actor whose script errored.
#[derive(Component)]
pub(crate) struct ScriptBroken;

/// Actors with runnable scripts and their transform; broken scripts are
/// filtered out at the query level.
type ActorQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform, &'static mut Actor),
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
) {
    for actor in &scene.actors {
        let script = actor.script.as_deref().and_then(load_script);
        commands.spawn((
            Actor { script },
            ActorModel(assets.load(gltf_asset_path(&actor.model))),
            Transform::from_xyz(actor.position[0], 0.0, actor.position[1])
                .with_rotation(facing_rotation(toward)),
        ));
    }
}

/// Reads and compiles an actor script from the assets folder.
fn load_script(path: &str) -> Option<ScriptRuntime> {
    let text = match std::fs::read_to_string(assets_root().join(path)) {
        Ok(text) => text,
        Err(e) => {
            warn!("Actor script {path} could not be read, actor runs without it: {e}");
            return None;
        }
    };
    match CompiledScript::compile(&text) {
        Ok(script) => Some(ScriptRuntime {
            script,
            scope: Scope::new(),
        }),
        Err(e) => {
            warn!("Actor script {path} failed to compile, actor runs without it: {e}");
            None
        }
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
            .with_child((WorldAssetRoot(scene), Transform::default()));
        commands.entity(entity).remove::<ActorModel>();
    }
}

/// Runs each actor's `on_update` and applies the returned position.
/// Actors are not grid-constrained: their scripts are trusted content.
pub(crate) fn run_actor_scripts(
    mut commands: Commands,
    time: Res<Time>,
    players: Query<&Transform, With<Player>>,
    mut actors: ActorQuery,
) {
    let Ok(player) = players.single() else {
        return;
    };
    let (player_x, player_z) = (player.translation.x, player.translation.z);
    let dt = time.delta_secs();
    for (entity, mut transform, mut actor) in &mut actors {
        let Some(runtime) = actor.script.as_mut() else {
            continue;
        };
        let position = &transform.translation;
        match runtime.script.update(
            &mut runtime.scope,
            position.x,
            position.z,
            player_x,
            player_z,
            dt,
        ) {
            Ok(Some([x, z])) => {
                let direction = Vec2::new(x, z) - Vec2::new(position.x, position.z);
                transform.translation = Vec3::new(x, 0.0, z);
                transform.rotation = face_direction(transform.rotation, direction, TURN_SPEED, dt);
            }
            Ok(None) => {}
            Err(e) => {
                warn!("Actor script errored, disabling it: {e}");
                commands.entity(entity).insert(ScriptBroken);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

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
                    script: CompiledScript::compile(text).unwrap(),
                    scope: Scope::new(),
                }),
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
            walkable: None,
            character_model: None,
            teleporters: Vec::new(),
            actors: vec![crate::scene::Actor {
                model: "models/goblin.glb".into(),
                position: [1.0, 2.0],
                script: None,
            }],
        };
        let server = world.resource::<AssetServer>().clone();
        let mut commands = world.commands();
        spawn_actors(&mut commands, &server, &scene, Vec2::NEG_Y);
        world.flush();

        let mut actors = world.query::<(&Transform, &Actor)>();
        let (transform, _) = actors.single(&world).unwrap();
        assert_eq!(transform.translation, Vec3::new(1.0, 0.0, 2.0));
        let nose = transform.rotation * Vec3::Y;
        assert!(nose.abs_diff_eq(Vec3::NEG_Z, 1e-5));
    }

    #[test]
    fn models_attach_when_their_gltf_loads() {
        let mut world = World::new();
        let mut gltfs = Assets::<Gltf>::default();
        let handle = gltfs.add(gltf_with_default_scene());
        world.insert_resource(gltfs);
        world.spawn((
            Actor { script: None },
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
        world.insert_resource(Time::<()>::default());
        world.spawn((Player, Transform::from_xyz(50.0, 0.9, 50.0)));
        let (actor, transform) = scripted_actor("fn on_update(x, z, px, pz, dt) { [x + 1.0, z] }");
        world.spawn((
            actor,
            transform.with_rotation(Quat::from_rotation_arc(Vec3::Y, Vec3::NEG_Z)),
        ));
        world
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(0.5));

        world.run_system_once(run_actor_scripts).unwrap();

        let mut actors = world.query_filtered::<&Transform, With<Actor>>();
        let transform = actors.single(&world).unwrap();
        assert_eq!(transform.translation, Vec3::new(2.0, 0.0, 2.0));
        // 5 radians of budget turns the nose all the way from -Z to +X.
        let nose = transform.rotation * Vec3::Y;
        assert!(nose.abs_diff_eq(Vec3::X, 1e-4));
    }

    #[test]
    fn a_broken_script_is_disabled_not_a_crash() {
        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
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
}
