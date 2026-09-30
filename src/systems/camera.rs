//! Gameplay camera setup: one 3D view per game state.

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use crate::GameCamera;
use crate::game_state::GameState;
use crate::screen::GameImage;

/// The 3D view that renders while a scene is playing. Posed by
/// apply_scene; the battle never touches it.
#[derive(Component)]
pub(crate) struct SceneCamera;

/// The 3D view that renders during a battle. Posed by stage_battle.
#[derive(Component)]
pub(crate) struct BattleCamera;

pub fn setup_game_camera(mut commands: Commands, game_image: Res<GameImage>) {
    // MSAA stays off: the game image is single-sampled, and hard edges
    // match the pixelated pipeline. Neither camera clears: the
    // background photo shows through wherever nothing renders. Both
    // start inactive — sync_camera_activation turns exactly one on.
    commands.spawn((
        GameCamera,
        SceneCamera,
        Camera3d::default(),
        Camera {
            order: 1,
            clear_color: ClearColorConfig::None,
            is_active: false,
            ..default()
        },
        Msaa::Off,
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(0),
        Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        BattleCamera,
        Camera3d::default(),
        Camera {
            order: 1,
            // The battle owns the whole image: nothing composites under
            // it (the background camera is suspended), so the pass must
            // clear — an uncleared frame smears the fighters and ghosts
            // the last scene across the arena's culled walls.
            clear_color: ClearColorConfig::Default,
            is_active: false,
            ..default()
        },
        Msaa::Off,
        RenderTarget::Image(game_image.0.clone().into()),
        RenderLayers::layer(0),
        Transform::from_xyz(10.0, 9.0, 10.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
}

/// Exactly one 3D view renders, per the game state. Runs every frame
/// so the activation can never drift from the state.
pub fn sync_camera_activation(
    state: Res<State<GameState>>,
    mut scene: Query<&mut Camera, With<SceneCamera>>,
    mut battle_cameras: Query<&mut Camera, (With<BattleCamera>, Without<SceneCamera>)>,
) {
    // The transition never touches the cameras: the outgoing view keeps
    // rendering (static — every motion system is state-gated) through
    // the capture and the closing cover, and the covered flip swaps
    // activation while the screen is fully black.
    let battle_active = match state.get() {
        GameState::Battle => true,
        GameState::Scene => false,
        GameState::Transition => return,
    };
    for mut camera in &mut scene {
        camera.is_active = !battle_active;
    }
    for mut camera in &mut battle_cameras {
        camera.is_active = battle_active;
    }
}
