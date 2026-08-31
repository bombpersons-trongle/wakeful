//! Scene loading and application: the systems that turn a loaded
//! `assets/scenes/*.scene` file into live entities and resources.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::world_serialization::WorldAsset;

use crate::scene::Scene;
use crate::screen;
use crate::{CurrentScene, GameCameraQuery, Player, PlayerModel, SceneApplied};

/// Camera layer that draws the pre-rendered background image.
const BG_LAYER: usize = 2;

pub fn load_scene(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(CurrentScene(assets.load("scenes/devroom.scene")));
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
    let Some(scene) = scenes.get(&current.0) else {
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
        commands.spawn((
            Sprite {
                image: assets.load(path),
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

    if let Some(path) = &scene.character_model {
        commands.insert_resource(PlayerModel(assets.load(path)));
    }

    applied.0 = true;
}

/// Swaps the placeholder capsule for the scene's character model once the
/// glTF file has loaded.
pub fn apply_player_model(
    mut commands: Commands,
    model: Option<Res<PlayerModel>>,
    scenes: Res<Assets<WorldAsset>>,
    mut players: Query<Entity, With<Player>>,
) {
    let Some(model) = model else {
        return;
    };
    if scenes.get(&model.0).is_none() {
        return;
    }
    let Ok(player) = players.single_mut() else {
        return;
    };
    commands
        .entity(player)
        .remove::<Mesh3d>()
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .with_child((WorldAssetRoot(model.0.clone()), Transform::default()));
    commands.remove_resource::<PlayerModel>();
}
