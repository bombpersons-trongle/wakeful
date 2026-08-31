mod movement;
mod screen;

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::window::WindowResolution;

use crate::movement::{PLAYER_SPEED, move_position};

/// Movement logic runs on a fixed step so behavior doesn't depend on
/// display refresh rate or frame timing jitter.
const FIXED_HZ: f64 = 60.0;

/// Capsule geometry; `PLAYER_Y` keeps it resting on the ground plane.
const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_HALF_HEIGHT: f32 = 0.5;
const PLAYER_Y: f32 = PLAYER_RADIUS + PLAYER_HALF_HEIGHT;
const GROUND_SIZE: f32 = 30.0;
const PLAYER_COLOR: Color = Color::srgb(0.949, 0.651, 0.306);
const GROUND_COLOR: Color = Color::srgb(0.23, 0.21, 0.28);

#[derive(Component)]
struct Player;

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
        .insert_resource(ClearColor(Color::srgb(0.10, 0.08, 0.13)))
        .insert_resource(Time::<Fixed>::from_hz(FIXED_HZ))
        .add_systems(
            Startup,
            (
                screen::setup_screen,
                setup_game_camera,
                spawn_world,
                spawn_player,
            )
                .chain(),
        )
        .add_systems(Update, (quit_on_escape, screen::resize_present))
        .add_systems(FixedUpdate, move_player)
        .run();
}

fn quit_on_escape(keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

/// FF7-style fixed view: high angle, looking down at the arena.
fn setup_game_camera(mut commands: Commands, game_image: Res<screen::GameImage>) {
    commands.spawn((
        Camera3d::default(),
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(0),
        Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Placeholder floor; a pre-rendered background image arrives with real art.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(GROUND_SIZE, GROUND_SIZE))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(GROUND_COLOR))),
    ));

    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn spawn_player(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Player,
        Mesh3d(meshes.add(Capsule3d::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT))),
        MeshMaterial3d(materials.add(StandardMaterial::from_color(PLAYER_COLOR))),
        Transform::from_xyz(0.0, PLAYER_Y, 0.0),
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

    // Arrows map onto the ground plane as seen by the fixed camera:
    // up walks away from the camera, down walks toward it.
    let mut direction = Vec2::ZERO;
    if keys.pressed(KeyCode::ArrowUp) {
        direction.y -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        direction.y += 1.0;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        direction.x -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        direction.x += 1.0;
    }

    let moved = move_position(
        transform.translation.xz(),
        direction,
        PLAYER_SPEED,
        time.delta_secs(),
    );
    transform.translation = Vec3::new(moved.x, PLAYER_Y, moved.y);
}
