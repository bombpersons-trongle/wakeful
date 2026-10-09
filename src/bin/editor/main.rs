//! The scene editor: its own app, not a mode of the game.
//!
//! `cargo run --bin editor -- scenes/Village_Entrance.scene`
//!
//! It opens a scene file, shows it the way the game will — the baked
//! plate as real geometry, the walk mesh characters are locked to — and
//! lets you fly around it at the window's own resolution, which the
//! game's fixed 320x240 pipeline cannot show you.
//!
//! Everything both apps must agree on (the scene format, the walk mesh,
//! the assets root, the card's geometry) lives in the library this
//! binary and the game share; everything below is the editor's own.

mod actors;
mod camera;
mod picking;
mod scene_view;
mod ui;

use bevy::prelude::*;
use bevy::window::WindowResolution;
use bevy_common_assets::ron::RonAssetPlugin;
use bevy_egui::{EguiPlugin, EguiPrimaryContextPass};

use wakeful::scene::Scene;

use actors::Selected;
use camera::EditorCamera;
use scene_view::OpenScene;

/// The scene opened when the command line names none. The game's boot
/// scene, so a bare run shows something familiar.
const DEFAULT_SCENE: &str = "scenes/devroom.scene";

/// What the command line asked for.
#[derive(Resource)]
struct Args {
    scene: String,
}

fn main() {
    // `cargo run --bin editor -- <scene>`; anything that is not a scene
    // path is ignored rather than fatal, because a typo should still open
    // the default instead of quitting before the window exists.
    let named = std::env::args()
        .nth(1)
        .filter(|arg| arg.ends_with(".scene"))
        .unwrap_or_else(|| DEFAULT_SCENE.to_owned());
    // Asset paths are relative to `assets/`, and named by file: pointing
    // the editor at another checkout's file is not a thing, so a
    // full-looking path is trimmed to its file name.
    let scene = format!("scenes/{}", named.rsplit('/').next().unwrap_or(&named));
    println!("[editor] opening {scene}");

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "wakeful — editor".into(),
                    // 4:3, the aspect every plate was baked at, so the
                    // background lines up with the game's framing. Not
                    // the game's 320x240: the whole point here is to see
                    // the scene at the window's own resolution.
                    resolution: WindowResolution::new(1280, 960),
                    ..default()
                }),
                ..default()
            })
            .set(bevy::asset::AssetPlugin {
                // Same walk-up as the game, so `target/debug/editor`
                // finds this checkout's assets.
                file_path: wakeful::assets::assets_root()
                    .to_string_lossy()
                    .into_owned(),
                ..default()
            }),
    )
    .add_plugins(RonAssetPlugin::<Scene>::new(&["scene"]))
    .add_plugins(EguiPlugin::default())
    .insert_resource(ClearColor(Color::srgb(0.06, 0.06, 0.08)))
    .insert_resource(Args { scene })
    .init_resource::<scene_view::Shown>()
    .init_resource::<Selected>()
    .init_resource::<actors::GroundPointer>()
    .init_resource::<ui::Clipboard>()
    .init_resource::<ui::Status>()
    .init_resource::<ui::Models>()
    .init_resource::<ui::Dialog>()
    .init_resource::<ui::Scenes>()
    .init_resource::<ui::ListSweep>()
    .init_resource::<picking::ViewportSweep>()
    .init_resource::<camera::UiWantsInput>()
    .init_resource::<scene_view::Working>()
    .add_systems(Startup, (setup, ui::list_assets))
    .add_systems(
        Update,
        (
            // Chained: a reload despawns the background the builder may
            // be halfway through, and the builder must not race it.
            camera::track_ui_input,
            scene_view::reload_when_changed,
            scene_view::apply_scene,
            scene_view::build_backgrounds,
            // The actors are the scene asset's view of itself, so they
            // follow the file: place them after whatever just rebuilt it.
            actors::sync_actors,
            actors::attach_models,
            actors::track_ground_pointer,
            camera::fly,
            camera::zoom,
            scene_view::draw_walk_mesh,
            actors::draw_selection,
            quit_on_escape,
        )
            .chain(),
    )
    // The panels and the picking that share a cursor with them run in the
    // egui pass, chained so a click that opens a menu is not also read as
    // a click on whatever the menu covered.
    .add_systems(
        EguiPrimaryContextPass,
        (picking::select_in_viewport, ui::panels).chain(),
    );

    app.run();
}

/// Spawns the camera the editor flies, and opens the scene named on the
/// command line.
fn setup(mut commands: Commands, assets: Res<AssetServer>, args: Res<Args>) {
    commands.spawn((
        EditorCamera,
        Camera3d::default(),
        Camera {
            // The window's own surface, at full resolution: no offscreen
            // target, no present pass, no letterbox.
            order: 0,
            clear_color: ClearColorConfig::Default,
            ..default()
        },
        // Scene application replaces this with the scene's own fov, which
        // is what its plate was baked through.
        Projection::Perspective(PerspectiveProjection {
            fov: 45.0_f32.to_radians(),
            ..default()
        }),
        // The card is unlit paint, but a light is here anyway so that
        // placed models (and anything lit) read as solids.
        Transform::from_xyz(0.0, 2.0, 0.0),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 6_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-8.0, 16.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(OpenScene::new(&assets, args.scene.clone()));
}

/// The game's habit, kept: escape leaves — but not while a field has
/// focus, where escape means "let go of this" first.
fn quit_on_escape(
    keys: Res<ButtonInput<KeyCode>>,
    mut ctxs: bevy_egui::EguiContexts,
    mut exit: MessageWriter<AppExit>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if ctxs.ctx_mut().is_ok_and(|ctx| ctx.egui_wants_keyboard_input()) {
        return;
    }
    exit.write(AppExit::Success);
}
