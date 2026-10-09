mod assets;
mod battle;
#[cfg(debug_assertions)]
mod debug_shot;
mod debug_server;
mod display;
mod editor;
mod game_state;
mod input;
mod movement;
mod scene;
mod screen;
mod scripts;
mod systems;
mod text;
mod transition;
mod walkmesh;
mod world_state;

use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::sprite_render::Material2dPlugin;
use bevy::window::WindowResolution;
use bevy_common_assets::ron::RonAssetPlugin;

use crate::input::InputManager;
use crate::scene::Scene;
use crate::systems::{
    actor, animation, bubble, camera, debug_draw, depth_card, input as sys_input, pan, party,
    player, scene as scene_loader, teleport, ui, world, world_script,
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
pub(crate) struct Player;

/// Marks the fixed gameplay camera whose pose the scene controls.
#[derive(Component)]
struct GameCamera;

/// Marks the placeholder ground plane; hidden while the scene shows a
/// depth card.
#[derive(Component)]
pub(crate) struct Ground;

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
    (
        &'static mut Transform,
        &'static mut Projection,
        &'static mut Camera,
    ),
    (With<GameCamera>, Without<Player>),
>;

fn main() {
    let mut app = App::new();
    app.init_resource::<transition::TransitionState>();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "wakeful".into(),
                    resolution: WindowResolution::new(
                        screen::GAME_WIDTH * 2,
                        screen::GAME_HEIGHT * 2,
                    ),
                    ..default()
                }),
                ..default()
            })
            .set(bevy::asset::AssetPlugin {
                // Anchor bevy's asset root to the same folder our own
                // file reads use, so direct binary launches (target/
                // debug/wakeful) resolve assets from the checkout.
                file_path: crate::assets::assets_root()
                    .to_string_lossy()
                    .into_owned(),
                ..default()
            }),
    )
    .add_plugins(RonAssetPlugin::<Scene>::new(&["scene"]))
    .add_plugins(Material2dPlugin::<transition::TransitionMaterial>::default())
    // The dither/CRT final post stays off: it is a fullscreen pass and
    // re-enabling it needs its own verification pass.
    // .add_plugins(FullscreenMaterialPlugin::<display::FinalPostMaterial>::default())
    .add_plugins(Material2dPlugin::<bubble::GradientMaterial>::default())
    .add_plugins(editor::plugin)
    .insert_resource(ClearColor(Color::srgb(0.10, 0.08, 0.13)))
    // bevy_gilrs only registers these when its backend starts, and
    // that can legitimately fail (no pad subsystem); empty ones
    // read as "no gamepad" instead of panicking the aggregator.
    .init_resource::<ButtonInput<GamepadButton>>()
    .init_resource::<Axis<GamepadAxis>>()
    .insert_resource(InputManager::load())
    .insert_resource(crate::input::InputCaptures::shared())
    .insert_resource(ui::UiApi::new())
    .insert_resource(world_state::WorldState::default())
    .insert_resource(battle::BattleHandle::new())
    .insert_resource(debug_server::DebugCommands::default())
    .insert_resource(crate::scripts::WorldCommands::default())
    .init_resource::<battle::PendingBattleStart>()
    .init_state::<game_state::GameState>()
    // The game opens on the boot menu; New Game / Continue hands over
    // to the scene.
    .insert_state(game_state::GameState::StartMenu)
    .insert_resource(crate::scripts::CurrentStateName::shared())
    .init_resource::<ui::UiPause>()
    .init_resource::<crate::input::InjectedInputs>()
    .insert_resource(party::Party::default())
    .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ));

    app.add_systems(
        Startup,
        (
            screen::setup_screen,
            camera::setup_game_camera,
            text::setup,
            load_ui_config,
            bubble::setup,
            ui::setup,
            transition::setup,
            // The context roots exist before anything spawns into them.
            scene_loader::setup_graphics,
            world::spawn_world,
            world_script::startup,
        )
            .chain(),
    )
    .add_systems(
        Update,
        (
            transition::capture.run_if(in_state(game_state::GameState::Transition)),
            transition::drive_transition,
        )
            .chain(),
    )
    .add_systems(
        Update,
        (
            scene_loader::load_desired_scene,
            world_script::sync_game_state,
            sys_input::quit_on_escape,
            camera::sync_camera_activation,
            screen::resize_present,
            screen::validate_post_process_layout,
            display::sync_display_effects,
            bubble::sync_theme,
            scene_loader::apply_scene,
            scene_loader::sync_ground,
            depth_card::build_pending_cards,
            party::sync_player_model,
            party::attach_player_model,
            actor::attach_actor_models,
            animation::resolve_pending_animations,
            bubble::fit_bubbles,
            bubble::animate_bubbles,
            actor::track_anchored_bubbles,
            pan::follow_player.run_if(in_state(game_state::GameState::Scene)),
            debug_draw::debug_draw_walkables,
        ),
    )
    // Transition before application so a scene whose file is already
    // cached applies the same frame the teleport lands.
    .add_systems(
        Update,
        (scene_loader::transition_scene.before(scene_loader::apply_scene),),
    )
    .add_systems(
        FixedUpdate,
        (
            battle::battle_requests,
            crate::input::aggregate_inputs,
            ui::navigate,
            bubble::dismiss_on_confirm,
            player::move_player.run_if(in_state(game_state::GameState::Scene)),
            teleport::check_teleporters.run_if(in_state(game_state::GameState::Scene)),
            actor::run_actor_scripts.run_if(in_state(game_state::GameState::Scene)),
            animation::run_character_animations,
            scene_loader::run_scene_scripts.run_if(in_state(game_state::GameState::Scene)),
            world_script::run_world_scripts
                .run_if(not(in_state(game_state::GameState::Transition))),
            world_script::drain_world_commands,
            battle::battle_turns.run_if(in_state(game_state::GameState::Battle)),
            ui::drain,
            ui::sync_cursor,
        )
            .chain(),
    )
    .add_systems(
        OnEnter(game_state::GameState::Battle),
        (battle::stage_battle, scene_loader::suspend_scene),
    )
    // Fires at the transition's covered point, behind the opaque cover:
    // the battle teardown and the scene resume are never on screen.
    // (Also fires once at boot, where both no-op harmlessly.)
    .add_systems(
        OnEnter(game_state::GameState::Scene),
        (
            battle::cleanup_battle,
            scene_loader::resume_scene,
            // The covered point: hand a queued scene warp to the swap.
            teleport::release_warp,
        ),
    );

    #[cfg(debug_assertions)]
    {
        if debug_server::console_requested() {
            app.add_systems(
                Startup,
                (
                    debug_shot::setup,
                    debug_server::spawn_server,
                    debug_server::spawn_repl,
                ),
            );
        } else {
            app.add_systems(Startup, (debug_shot::setup, debug_server::spawn_server));
        }
        // The executor feeds the injected-input layer, so it runs at
        // the head of the tick: a tap lands in this tick's aggregate.
        app.add_systems(FixedUpdate, debug_shot::serve_debug_commands);
    }

    app.run();
}
