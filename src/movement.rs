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
}
