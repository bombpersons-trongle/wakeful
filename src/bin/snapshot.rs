//! Headless render snapshot: renders the game scene into the offscreen
//! 320x240 texture without any OS window, then saves that texture as a PNG.
//!
//! Used to visually verify rendering changes on machines without a display
//! (CI, sandboxes, remote shells). Run with a software GL setup, e.g.:
//!
//! ```sh
//! DISPLAY=:99 LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=gl cargo run --bin snapshot -- out.png
//! ```
//!
//! The scene here mirrors `main.rs`; if it grows, move shared scene setup
//! into a common module instead of duplicating more of it.

use std::time::{Duration, Instant};

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::core_pipeline::fullscreen_material::FullscreenMaterialPlugin;
use bevy::prelude::*;
use bevy::render::error_handler::{RenderErrorHandler, RenderErrorPolicy};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::{ExitCondition, WindowPlugin};

// Shares the dither material with the main game binary.
#[path = "../dither.rs"]
mod dither;

// Mirrors the game's virtual resolution in `screen.rs`.
const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;

const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_HALF_HEIGHT: f32 = 0.5;
const PLAYER_Y: f32 = PLAYER_RADIUS + PLAYER_HALF_HEIGHT;
const GROUND_SIZE: f32 = 30.0;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);
const GROUND_COLOR: Color = Color::srgb(0.23, 0.21, 0.28);

/// Handle to the texture the game camera renders into.
#[derive(Resource)]
struct GameImage(Handle<Image>);

/// A virtual-resolution render target the game camera draws into.
fn target_image() -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    // new_fill omits RENDER_ATTACHMENT, but the camera renders into this
    // texture; COPY_SRC lets the screenshot read it back.
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    image
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "snapshot.png".into());

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>(),
        )
        // Headless software renderers may fail to build some optional bevy
        // pipelines (e.g. SSAO on llvmpipe); keep rendering anyway.
        .insert_resource(RenderErrorHandler(|_, _, _| RenderErrorPolicy::Ignore))
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16)))
        .add_plugins(FullscreenMaterialPlugin::<dither::DitherPostProcess>::default())
        .add_observer(on_captured(output))
        .add_systems(Startup, setup_scene)
        .add_systems(Update, (capture_soon, exit_soon))
        .run();
}

/// Saves the captured frame, then quits.
fn on_captured(output: String) -> impl FnMut(On<ScreenshotCaptured>, Commands) {
    move |trigger, mut commands| {
        (save_to_disk(&output))(trigger);
        commands.write_message(AppExit::Success);
    }
}

fn setup_scene(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let scene_image = images.add(target_image());
    commands.insert_resource(GameImage(scene_image.clone()));

    // The dither pass runs after this camera's 3D pass, dithering the
    // finished frame in place before the screenshot reads it back.
    commands.spawn((
        Camera3d::default(),
        Msaa::Off,
        RenderTarget::Image(scene_image.into()),
        dither::DitherPostProcess { ..dither::tuned() },
        Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(GROUND_SIZE, GROUND_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(GROUND_COLOR))),
    ));

    // Headless software renderers can't compile bevy's shadow-sampling
    // shaders; verification doesn't need shadows.
    commands.spawn((
        DirectionalLight {
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_xyz(0.0, PLAYER_Y, 0.0),
    ));
}

fn capture_soon(
    mut commands: Commands,
    mut done: Local<bool>,
    mut started: Local<Option<Instant>>,
    game_image: Res<GameImage>,
) {
    // Give the renderer a couple of seconds; software rasterizers are slow.
    let start = *started.get_or_insert_with(Instant::now);
    if *done || start.elapsed() < Duration::from_secs(2) {
        return;
    }
    *done = true;
    commands.spawn(Screenshot::image(game_image.0.clone()));
}

fn exit_soon(mut started: Local<Option<Instant>>, mut exit: MessageWriter<AppExit>) {
    // Fallback exit in case the capture never completes.
    let start = *started.get_or_insert_with(Instant::now);
    if start.elapsed() > Duration::from_secs(60) {
        exit.write(AppExit::Success);
    }
}
