//! Scene loading and application: the systems that turn a loaded
//! `assets/scenes/*.scene` file into live entities and resources.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::gltf::Gltf;
use bevy::prelude::*;

use crate::scene::Scene;
use crate::screen;
use crate::{
    BackgroundSprite, CurrentScene, GameCameraQuery, Ground, Player, PlayerModel, SceneApplied,
};

/// Camera layer that draws the pre-rendered background image.
const BG_LAYER: usize = 2;

/// The scene file the game loads; the editor saves back to this path via
/// the copy stored on `CurrentScene`.
const SCENE_PATH: &str = "scenes/devroom.scene";

pub fn load_scene(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(CurrentScene {
        handle: assets.load(SCENE_PATH),
        path: SCENE_PATH,
    });
    commands.insert_resource(SceneApplied(false));
}

/// Applies the scene once its file has loaded: camera pose, background
/// layer, and character model choice.
pub fn apply_scene(
    mut commands: Commands,
    assets: Res<AssetServer>,
    game_image: Res<screen::GameImage>,
    scenes: Res<Assets<Scene>>,
    current: Option<Res<CurrentScene>>,
    applied: Option<ResMut<SceneApplied>>,
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
        Camera2d,
        Camera {
            order: 0,
            ..default()
        },
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(BG_LAYER),
    ));

    if let Some(path) = &scene.background {
        spawn_background(&mut commands, &assets, path);
    }

    if let Some(path) = &scene.character_model {
        commands.insert_resource(PlayerModel(assets.load(gltf_asset_path(path))));
    }

    applied.0 = true;
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

/// Swaps the placeholder capsule for the scene's character model once the
/// glTF file has loaded.
pub fn apply_player_model(
    mut commands: Commands,
    model: Option<Res<PlayerModel>>,
    gltfs: Res<Assets<Gltf>>,
    mut players: Query<Entity, With<Player>>,
) {
    let Some(model) = model else {
        return;
    };
    let Some(gltf) = gltfs.get(&model.0) else {
        return;
    };
    // The file's default scene; the first one if the glTF declares none.
    let Some(scene) = gltf
        .default_scene
        .clone()
        .or_else(|| gltf.scenes.first().cloned())
    else {
        return;
    };
    let Ok(player) = players.single_mut() else {
        return;
    };
    commands
        .entity(player)
        .remove::<Mesh3d>()
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .with_child((WorldAssetRoot(scene), Transform::default()));
    commands.remove_resource::<PlayerModel>();
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use crate::scene::CameraPose;

    use super::*;

    fn test_scene(background: Option<&str>) -> Scene {
        Scene {
            background: background.map(str::to_string),
            camera: CameraPose {
                position: [0.0, 6.0, 9.0],
                target: [0.0, 0.0, 0.0],
                fov_degrees: 45.0,
            },
            walkable: None,
            character_model: None,
        }
    }

    fn world_with_scene(background: Option<&str>) -> (World, Entity) {
        let mut world = World::new();
        let mut assets = Assets::<Scene>::default();
        let handle = assets.add(test_scene(background));
        world.insert_resource(assets);
        world.insert_resource(CurrentScene {
            handle,
            path: "scenes/devroom.scene",
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
}
