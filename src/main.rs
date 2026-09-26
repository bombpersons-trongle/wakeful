mod assets;
#[cfg(debug_assertions)]
mod debug_shot;
mod display;
mod dither;
mod editor;
mod input;
mod movement;
mod scene;
mod screen;
mod scripts;
mod systems;
mod text;

use bevy::core_pipeline::fullscreen_material::FullscreenMaterialPlugin;
use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::sprite_render::Material2dPlugin;
use bevy::window::WindowResolution;
use bevy_common_assets::ron::RonAssetPlugin;

use crate::input::InputManager;
use crate::scene::Scene;
use crate::systems::{
    actor, animation, bubble, camera, debug_draw, input as sys_input, party, player,
    scene as scene_loader, teleport, ui, world, world_script,
};
use std::path::Path;

/// Movement logic runs on a fixed step so behavior doesn't depend on
/// display refresh rate or frame timing jitter.
const FIXED_HZ: f64 = 60.0;

/// Loads the global UI config into the resources that style themselves
/// from it; runs before `bubble::setup` in the startup chain.
fn load_ui_config(mut commands: Commands) {
    let path = Path::new(assets::assets_root().as_os_str()).join("ui.ron");
    commands.insert_resource(bubble::BubbleTheme::from_file(&path));
    commands.insert_resource(display::DisplaySettings::from_file(&path));
}

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

/// Sprite spawned for the scene's background image; despawned with the
/// background camera on scene change, and respawned by the editor when
/// the background path changes.
#[derive(Component)]
struct BackgroundSprite;

/// Per-scene camera that draws the background layer; spawned by
/// `apply_scene` and despawned on scene change.
#[derive(Component)]
struct BackgroundCamera;

/// The scene the game is currently running. `load_scene` inserts it with a
/// handle whose asset loads asynchronously; `apply_scene` polls it until
/// the file arrives. The path is kept so the editor can save back to the
/// file the scene was loaded from; teleporters update it on scene change.
#[derive(Resource)]
struct CurrentScene {
    handle: Handle<Scene>,
    path: String,
}

/// One-shot flag pairing with `CurrentScene`: the bool starts `false` and
/// `apply_scene` sets it `true` after turning the loaded scene into live
/// entities, so the application runs exactly once even though the poll
/// runs every frame.
#[derive(Resource)]
struct SceneApplied(bool);

/// A teleporter was touched: the destination scene file and where the
/// player appears. `transition_scene` consumes it.
#[derive(Resource)]
struct PendingTeleport {
    target: String,
    arrival: Vec2,
}

/// Where the player spawns in the scene being applied, set by
/// `transition_scene` and consumed by `apply_scene`. Absent on first
/// load: the player starts at the world origin.
#[derive(Resource)]
struct PlayerSpawn(Vec2);

/// One armed flag per teleporter in the current scene, rebuilt by
/// `apply_scene` on every scene application: a teleporter that already
/// contains the player's spawn point starts disarmed, so arrival regions
/// only fire after the player leaves and re-enters. Flags beyond the
/// built length (e.g. teleporters added live in the editor) count as
/// armed.
#[derive(Resource)]
struct TeleporterArmed(Vec<bool>);

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
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "wakeful".into(),
            resolution: WindowResolution::new(screen::GAME_WIDTH * 2, screen::GAME_HEIGHT * 2),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(RonAssetPlugin::<Scene>::new(&["scene"]))
    .add_plugins(FullscreenMaterialPlugin::<dither::DitherPostProcess>::default())
    .add_plugins(FullscreenMaterialPlugin::<display::CrtMaterial>::default())
    .add_plugins(Material2dPlugin::<bubble::GradientMaterial>::default())
    .add_plugins(editor::plugin)
    .insert_resource(ClearColor(Color::srgb(0.10, 0.08, 0.13)))
    // bevy_gilrs only registers these when its backend starts, and
    // that can legitimately fail (no pad subsystem); empty ones
    // read as "no gamepad" instead of panicking the aggregator.
    .init_resource::<ButtonInput<GamepadButton>>()
    .init_resource::<Axis<GamepadAxis>>()
    .insert_resource(InputManager::load())
    .insert_resource(ui::UiApi::new())
    .init_resource::<ui::UiPause>()
    .init_resource::<crate::input::InjectedInputs>()
    .insert_resource(party::Party::default())
    .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
    .add_systems(
        Startup,
        (
            screen::setup_screen,
            camera::setup_game_camera,
            text::setup,
            load_ui_config,
            bubble::setup,
            ui::setup,
            world::spawn_world,
            world_script::startup,
            scene_loader::load_scene,
        )
            .chain(),
    )
    .add_systems(
        Update,
        (
            sys_input::quit_on_escape,
            screen::resize_present,
            screen::validate_post_process_layout,
            display::sync_display_effects,
            bubble::dismiss_on_confirm,
            bubble::sync_theme,
            scene_loader::apply_scene,
            scene_loader::sync_ground,
            party::sync_player_model,
            party::attach_player_model,
            actor::attach_actor_models,
            animation::resolve_pending_animations,
            bubble::fit_bubbles,
            bubble::animate_bubbles,
            debug_draw::debug_draw_walkables,
        ),
    )
    // Transition before application so a scene whose file is already
    // cached applies the same frame the teleport lands.
    .add_systems(
        Update,
        scene_loader::transition_scene.before(scene_loader::apply_scene),
    )
    .add_systems(
        FixedUpdate,
        (
            crate::input::aggregate_inputs,
            ui::navigate,
            player::move_player,
            teleport::check_teleporters,
            actor::run_actor_scripts,
            animation::run_character_animations,
            scene_loader::run_scene_scripts,
            world_script::run_world_scripts,
            ui::drain,
            ui::sync_cursor,
        )
            .chain(),
    );

    #[cfg(debug_assertions)]
    {
        app.add_systems(Startup, debug_shot::setup);
        app.add_systems(
            Update,
            (debug_shot::check_requests, debug_shot::check_input_requests),
        );
    }

    app.run();
}
