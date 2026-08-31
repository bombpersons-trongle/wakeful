mod movement;

use bevy::prelude::*;
use bevy::window::WindowResolution;
use bevy_pixcam::{PixelCameraPlugin, PixelViewport, PixelZoom};

use crate::movement::{PLAYER_SPEED, move_position};

/// The virtual resolution the game is rendered at, then integer-upscaled
/// to the window with black bars around it.
const GAME_WIDTH: i32 = 640;
const GAME_HEIGHT: i32 = 480;

/// Movement logic runs on a fixed step so behavior doesn't depend on
/// display refresh rate or frame timing jitter.
const FIXED_HZ: f64 = 60.0;

#[derive(Component)]
struct Player;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "wakeful".into(),
                        resolution: WindowResolution::new(
                            (GAME_WIDTH * 2) as u32,
                            (GAME_HEIGHT * 2) as u32,
                        ),
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
        )
        .add_plugins(PixelCameraPlugin)
        .insert_resource(ClearColor(Color::srgb(0.10, 0.08, 0.13)))
        .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
        .add_systems(Startup, (setup_camera, spawn_player))
        .add_systems(Update, quit_on_escape)
        .add_systems(FixedUpdate, move_player)
        .run();
}

fn quit_on_escape(keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        PixelZoom::FitSize {
            width: GAME_WIDTH,
            height: GAME_HEIGHT,
        },
        PixelViewport,
    ));
}

fn spawn_player(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Player,
        Sprite::from_image(assets.load("sprites/player.png")),
    ));
}

fn move_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    let Ok(mut transform) = players.single_mut() else {
        return;
    };

    let mut direction = Vec2::ZERO;
    if keys.pressed(KeyCode::ArrowUp) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    transform.translation = move_position(
        transform.translation.truncate(),
        direction,
        PLAYER_SPEED,
        time.delta_secs(),
    )
    .extend(0.0);
}
