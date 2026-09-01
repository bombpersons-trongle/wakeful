//! Player movement math.
//!
//! Pure math with no Bevy ECS involved, so the unit tests below run without
//! an app or a window.

use bevy::math::Vec2;

/// World units moved per second.
pub const PLAYER_SPEED: f32 = 4.0;

/// Steps `position` along `direction` at `speed`, scaled by `dt`.
///
/// `direction` is normalized first so moving diagonally isn't faster than
/// moving cardinally; a zero direction leaves the position unchanged.
pub fn move_position(position: Vec2, direction: Vec2, speed: f32, dt: f32) -> Vec2 {
    position + direction.normalize_or_zero() * (speed * dt)
}

/// Rotates screen-space arrow input onto the ground plane as seen from
/// the camera: screen up walks away from the camera, screen right walks
/// to the camera's screen-right. `screen` is +x = right and +y = away;
/// `forward` is the camera's ground-projected facing direction.
///
/// A degenerate `forward` (camera looking straight down) yields no
/// movement.
pub fn camera_relative_direction(screen: Vec2, forward: Vec2) -> Vec2 {
    let f = forward.normalize_or_zero();
    // Camera right in world space is forward × world-up, which reduces
    // on the ground plane to a 90° counter-clockwise turn.
    let right = Vec2::new(-f.y, f.x);
    screen.x * right + screen.y * f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_cardinally() {
        let moved = move_position(Vec2::ZERO, Vec2::X, 100.0, 1.0);
        assert_eq!(moved, Vec2::new(100.0, 0.0));
    }

    #[test]
    fn diagonals_are_no_faster_than_cardinals() {
        let cardinal = move_position(Vec2::ZERO, Vec2::X, 100.0, 1.0);
        let diagonal = move_position(Vec2::ZERO, Vec2::ONE, 100.0, 1.0);
        assert_eq!(cardinal.length(), diagonal.length());
    }

    #[test]
    fn zero_direction_stays_put() {
        let moved = move_position(Vec2::new(3.0, 4.0), Vec2::ZERO, 100.0, 1.0);
        assert_eq!(moved, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn scales_with_delta_time() {
        let moved = move_position(Vec2::ZERO, Vec2::X, 100.0, 0.5);
        assert_eq!(moved, Vec2::new(50.0, 0.0));
    }

    fn devroom_forward() -> Vec2 {
        // Camera above +Z looking at the origin: ground facing is -Z.
        Vec2::new(0.0, -1.0)
    }

    #[test]
    fn devroom_style_camera_keeps_the_classic_mapping() {
        // up walks away (-Z), right walks +X, down and left mirror.
        assert_eq!(
            camera_relative_direction(Vec2::Y, devroom_forward()),
            Vec2::NEG_Y
        );
        assert_eq!(
            camera_relative_direction(Vec2::X, devroom_forward()),
            Vec2::X
        );
        assert_eq!(
            camera_relative_direction(Vec2::NEG_Y, devroom_forward()),
            Vec2::Y
        );
        assert_eq!(
            camera_relative_direction(Vec2::NEG_X, devroom_forward()),
            Vec2::NEG_X
        );
    }

    #[test]
    fn an_opposite_facing_flips_the_mapping() {
        // Camera above -Z looking at the origin: ground facing is +Z.
        let forward = Vec2::new(0.0, 1.0);
        assert_eq!(camera_relative_direction(Vec2::Y, forward), Vec2::Y);
        assert_eq!(camera_relative_direction(Vec2::X, forward), Vec2::NEG_X);
    }

    #[test]
    fn screen_right_stays_perpendicular_to_the_facing() {
        // A 45° yaw: right must be the 90° counter-clockwise turn of the
        // normalized forward.
        let forward = Vec2::ONE.normalize();
        let right = camera_relative_direction(Vec2::X, forward);
        assert!(right.dot(forward).abs() < 1e-5);
        assert!(right.x < 0.0 && right.y > 0.0);
        assert!((right.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn diagonals_map_length_for_length() {
        // screen (1,1) on the devroom camera maps to a vector of the same
        // length; move_position then normalizes it as before.
        let mapped = camera_relative_direction(Vec2::ONE, devroom_forward());
        assert_eq!(mapped, Vec2::new(1.0, -1.0));
        assert_eq!(mapped.length(), Vec2::ONE.length());
    }

    #[test]
    fn a_top_down_facing_moves_nothing() {
        // A camera looking straight down has no ground-projected facing.
        assert_eq!(camera_relative_direction(Vec2::ONE, Vec2::ZERO), Vec2::ZERO);
    }
}
