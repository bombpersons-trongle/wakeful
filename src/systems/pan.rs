//! The camera pan: a scene whose background plate is bigger than the
//! game's 320x240 view shows a window of it that follows the player,
//! clamping at the plate's edges (the player walks to the edge; the
//! window just stops).
//!
//! The window is a `SubCameraView` on the scene camera: a pure crop of
//! the full plate view, so nothing about the world, the actors, or the
//! depth card changes as it slides — apply_scene set the full view's fov
//! to the plate's bake fov, and the card *is* the plate.

use bevy::prelude::*;

use crate::camera::SceneCamera;

/// Where the window's top-left corner should sit so the player is
/// centered, clamped to the plate. `player_at` is the player's position
/// in PLATE pixels — the full view's own projection, which does not
/// depend on where the window currently is (a window-space measurement
/// would feed the follow math back into itself and hunt).
pub(crate) fn target_offset(player_at: Vec2, window: Vec2, plate: Vec2) -> Vec2 {
    (player_at - window / 2.0).clamp(Vec2::ZERO, plate - window)
}

/// The player's position in plate pixels, projecting through the full
/// plate view by hand: view space, then the pinhole at the plate's
/// vertical fov, then pixels with the origin at the top-left.
pub(crate) fn player_plate_px(
    camera_transform: &GlobalTransform,
    fov_radians: f32,
    plate: Vec2,
    world_position: Vec3,
) -> Option<Vec2> {
    let view = camera_transform
        .affine()
        .inverse()
        .transform_point3(world_position);
    // Bevy view space looks down -z; points behind the camera have no
    // window they belong in.
    if view.z >= -1e-4 {
        return None;
    }
    let tan_v = (fov_radians * 0.5).tan();
    let tan_h = tan_v * plate.x / plate.y;
    let ndc_x = view.x / (-view.z * tan_h);
    let ndc_y = view.y / (-view.z * tan_v);
    if !ndc_x.is_finite() || !ndc_y.is_finite() {
        return None;
    }
    Some(Vec2::new(
        (ndc_x + 1.0) * 0.5 * plate.x,
        (1.0 - (ndc_y + 1.0) * 0.5) * plate.y,
    ))
}

/// Snaps the window onto the player every frame — no smoothing: the
/// view rides the player directly and stops crisply at the plate's
/// edges. Runs every frame the scene is live; the window's offset in
/// the camera's own sub view is the whole carried-over state, so
/// scenes without a pan spec (no sub view) never enter the math.
pub(crate) fn follow_player(
    player: Query<&GlobalTransform, With<crate::Player>>,
    mut camera: Query<(&mut Camera, &Projection, &GlobalTransform), With<SceneCamera>>,
) {
    let Ok(player) = player.single() else {
        return;
    };
    let Ok((mut camera, projection, camera_transform)) = camera.single_mut() else {
        return;
    };
    let Some(sub) = camera.sub_camera_view.as_ref() else {
        return;
    };
    let window = sub.size.as_vec2();
    let plate = sub.full_size.as_vec2();
    // Project through the FULL plate view in plate pixels — a constant
    // projection that cannot see the window, so the follow math cannot
    // feed back into itself (that made the window hunt and wobble).
    // The camera's fov is the plate's: it is what the plate was baked
    // with and what apply_scene set.
    let Projection::Perspective(perspective) = projection else {
        return;
    };
    let Some(player_at) =
        player_plate_px(camera_transform, perspective.fov, plate, player.translation())
    else {
        return;
    };
    let target = target_offset(player_at, window, plate);
    if let Some(sub) = camera.sub_camera_view.as_mut() {
        sub.offset = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera of the synthetic devroom frame: at (0, 6, 9) looking
    /// at the origin, 45 degrees vertical, on a 640x480 plate.
    fn devroom_camera() -> (GlobalTransform, f32) {
        (
            GlobalTransform::from(Transform::from_xyz(0.0, 6.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y)),
            45.0_f32.to_radians(),
        )
    }

    #[test]
    fn the_player_is_centered_when_the_plate_allows() {
        let (transform, fov) = devroom_camera();
        let plate = Vec2::new(640.0, 480.0);
        // The world origin projects to the plate's center; a window of
        // the plate's half size centered there sits at one quarter.
        let player_at = player_plate_px(&transform, fov, plate, Vec3::ZERO).unwrap();
        assert!(player_at.abs_diff_eq(plate / 2.0, 1e-3));
        let offset = target_offset(player_at, plate / 2.0, plate);
        assert!(offset.abs_diff_eq(plate / 4.0, 1e-3));
    }

    #[test]
    fn the_window_stops_at_the_plate_edges() {
        let window = Vec2::new(320.0, 240.0);
        let plate = Vec2::new(640.0, 480.0);
        // Player at the plate's top-left corner: the window pins at zero.
        assert_eq!(target_offset(Vec2::ZERO, window, plate), Vec2::ZERO);
        // Player beyond the bottom-right: the window pins at the travel
        // limit.
        assert_eq!(target_offset(plate, window, plate), plate - window);
    }

    #[test]
    fn a_window_the_size_of_the_plate_never_moves() {
        let plate = Vec2::new(320.0, 240.0);
        assert_eq!(target_offset(Vec2::new(40.0, 30.0), plate, plate), Vec2::ZERO);
    }

    #[test]
    fn plate_projection_respects_the_fov_and_the_flip() {
        let (transform, fov) = devroom_camera();
        let plate = Vec2::new(640.0, 480.0);
        // Off-center world points land off-center in matching screen
        // directions: +x world is screen right, +y world is screen up
        // (which is a smaller y in top-left pixel space).
        let right = player_plate_px(&transform, fov, plate, Vec3::new(3.0, 0.0, 0.0)).unwrap();
        let up = player_plate_px(&transform, fov, plate, Vec3::new(0.0, 2.0, 0.0)).unwrap();
        assert!(right.x > plate.x / 2.0);
        assert!(up.y < plate.y / 2.0);
        // Behind the camera has no window.
        assert!(player_plate_px(&transform, fov, plate, Vec3::new(0.0, 6.0, 12.0)).is_none());
    }
}
