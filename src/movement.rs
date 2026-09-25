//! Player movement math.
//!
//! Pure math with no Bevy ECS involved, so the unit tests below run without
//! an app or a window.

use bevy::math::{Quat, Vec2, Vec3};

/// World units moved per second.
pub const PLAYER_SPEED: f32 = 4.0;

/// World units moved per second while holding run; a brisker gait for
/// the run animation and impatient players.
pub const PLAYER_RUN_SPEED: f32 = 7.0;

/// How fast the player turns toward the movement direction, in radians
/// per second; a half turn takes about 0.3 s.
pub const TURN_SPEED: f32 = 10.0;

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

/// The rotation pointing a +Z front axis (the glTF model-forward) along
/// `direction` on the ground plane immediately, keeping the body
/// upright. An idle direction leaves the body un-rotated (+Z, toward
/// the camera's default).
pub fn facing_rotation(direction: Vec2) -> Quat {
    if direction == Vec2::ZERO {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(
        Vec3::Z,
        Vec3::new(direction.x, 0.0, direction.y).normalize(),
    )
}

/// Steps `current` toward facing `direction` on the ground plane at
/// `turn_speed` radians per second, rotating around world up so the
/// front stays level. An idle direction keeps the current facing.
///
/// The body's front axis is +Z (a glTF model's forward); the turn is
/// the signed ground-yaw difference between the current front and the
/// direction, not a quaternion slerp — the shortest SO(3) path between
/// two front-pointing orientations would swing the front up off the
/// plane.
pub fn face_direction(current: Quat, direction: Vec2, turn_speed: f32, dt: f32) -> Quat {
    if direction == Vec2::ZERO {
        return current;
    }
    let dir3 = Vec3::new(direction.x, 0.0, direction.y).normalize();
    let front = current * Vec3::Z;
    let front_ground = Vec3::new(front.x, 0.0, front.z);
    if front_ground.length_squared() < 1e-8 {
        // Front pointing straight up (or down): a world-up turn can't
        // tip it, so take the shortest arc directly, step-limited.
        let target = Quat::from_rotation_arc(Vec3::Z, dir3);
        let angle = current.angle_between(target);
        let step = angle.min(turn_speed * dt);
        return current.slerp(target, step / angle);
    }
    let front_ground = front_ground.normalize();
    let signed = front_ground.cross(dir3).y.atan2(front_ground.dot(dir3));
    let step = signed.abs().min(turn_speed * dt);
    Quat::from_axis_angle(Vec3::Y, signed.signum() * step) * current
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

    /// Where the body's front (+Z axis, a glTF model's forward) points
    /// after applying `rotation`.
    fn front_of(rotation: Quat) -> Vec3 {
        rotation * Vec3::Z
    }

    #[test]
    fn idle_keeps_the_current_facing() {
        let current = Quat::from_rotation_arc(Vec3::Z, Vec3::X);
        assert_eq!(face_direction(current, Vec2::ZERO, 10.0, 0.016), current);
    }

    #[test]
    fn a_big_step_faces_the_direction_exactly() {
        // Enough turn budget covers the whole angle in one tick.
        let faced = face_direction(Quat::IDENTITY, Vec2::X, 100.0, 1.0);
        assert!(front_of(faced).abs_diff_eq(Vec3::X, 1e-5));
    }

    #[test]
    fn a_small_step_turns_toward_the_direction_around_world_up() {
        let current = Quat::from_rotation_arc(Vec3::Z, Vec3::NEG_Z);
        let turned = face_direction(current, Vec2::NEG_X, 10.0, 0.1);
        // 1 radian of budget on the 90° turn: the front stays level and
        // is 1 rad short of -X.
        let front = front_of(turned);
        assert!(front.y.abs() < 1e-5);
        assert!((front.dot(Vec3::NEG_X) - (core::f32::consts::FRAC_PI_2 - 1.0).cos()).abs() < 1e-4);
    }

    #[test]
    fn turning_rate_scales_with_the_budget() {
        let current = Quat::from_rotation_arc(Vec3::Z, Vec3::NEG_Z);
        let slow = face_direction(current, Vec2::NEG_X, 2.0, 0.5);
        let fast = face_direction(current, Vec2::NEG_X, 4.0, 0.5);
        // 1 rad of budget covers part of the 90° turn; double the budget
        // covers all of it.
        let slow_front = front_of(slow);
        assert!(
            (slow_front.dot(Vec3::NEG_X) - (core::f32::consts::FRAC_PI_2 - 1.0).cos()).abs() < 1e-4
        );
        assert!(front_of(fast).abs_diff_eq(Vec3::NEG_X, 1e-5));
    }

    #[test]
    fn converging_faces_settle_at_the_target() {
        // Tiny remaining angles snap instead of dividing by ~zero.
        let current = Quat::from_rotation_arc(Vec3::Z, Vec3::X);
        let faced = face_direction(current, Vec2::NEG_Y, 10.0, 1.0);
        assert!(front_of(faced).abs_diff_eq(Vec3::NEG_Z, 1e-5));
    }
}
