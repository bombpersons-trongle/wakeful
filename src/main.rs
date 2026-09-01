mod dither;
mod editor;
mod movement;
mod scene;
mod screen;
mod systems;

use bevy::core_pipeline::fullscreen_material::FullscreenMaterialPlugin;
use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::window::WindowResolution;
use bevy_common_assets::ron::RonAssetPlugin;

use crate::scene::Scene;
use crate::systems::{camera, debug_draw, input, player, scene as scene_loader, world};

/// Movement logic runs on a fixed step so behavior doesn't depend on
/// display refresh rate or frame timing jitter.
const FIXED_HZ: f64 = 60.0;

/// Marks the player actor; movement and model-swap systems target this
/// entity.
#[derive(Component)]
struct Player;

/// Marks the fixed gameplay camera whose pose the scene controls.
#[derive(Component)]
struct GameCamera;

/// Marks the placeholder ground plane; hidden while the scene shows a
/// pre-rendered background.
#[derive(Component)]
struct Ground;

/// Sprite spawned for the scene's background image; the editor despawns
/// and respawns these when the background path changes.
#[derive(Component)]
struct BackgroundSprite;

/// The scene the game is currently running. `load_scene` inserts it with a
/// handle whose asset loads asynchronously; `apply_scene` polls it until
/// the file arrives. The path is kept so the editor can save back to the
/// file the scene was loaded from.
#[derive(Resource)]
struct CurrentScene {
    handle: Handle<Scene>,
    path: &'static str,
}

/// One-shot flag pairing with `CurrentScene`: the bool starts `false` and
/// `apply_scene` sets it `true` after turning the loaded scene into live
/// entities, so the application runs exactly once even though the poll
/// runs every frame.
#[derive(Resource)]
struct SceneApplied(bool);

/// Character model queued for the player, held until its glTF finishes
/// loading; `apply_player_model` removes it once applied.
#[derive(Resource)]
struct PlayerModel(Handle<Gltf>);

type GameCameraQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Projection),
    (With<GameCamera>, Without<Player>),
>;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "wakeful".into(),
                resolution: WindowResolution::new(screen::GAME_WIDTH * 2, screen::GAME_HEIGHT * 2),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(RonAssetPlugin::<Scene>::new(&["scene"]))
        .add_plugins(FullscreenMaterialPlugin::<dither::DitherPostProcess>::default())
        .add_plugins(editor::plugin)
        .insert_resource(ClearColor(Color::srgb(0.10, 0.08, 0.13)))
        .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
        .add_systems(
            Startup,
            (
                screen::setup_screen,
                camera::setup_game_camera,
                world::spawn_world,
                player::spawn_player,
                scene_loader::load_scene,
            )
                .chain(),
        )
        .add_systems(
            Update,
            (
                input::quit_on_escape,
                screen::resize_present,
                scene_loader::apply_scene,
                scene_loader::sync_ground,
                scene_loader::apply_player_model,
                debug_draw::debug_draw_walkables,
            ),
        )
        .add_systems(FixedUpdate, player::move_player)
        .run();
}
