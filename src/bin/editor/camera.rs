//! The free-fly camera: WASD and the mouse, nothing else.
//!
//! Yaw and pitch live on the entity's rotation and the movement basis is
//! derived from them, so the camera is one transform and this module is
//! the arithmetic around it. Look is right-mouse-drag rather than a
//! grabbed cursor: an editor is often driven over a remote session, and
//! taking the pointer away is the one thing you cannot undo without
//! killing the process.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

/// Radians of yaw per pixel dragged, and of pitch. Pitch is a touch
/// slower because a vertical sweep is easier to overshoot.
const YAW_PER_PIXEL: f32 = 0.0025;
const PITCH_PER_PIXEL: f32 = 0.0020;

/// Degrees of field of view per scroll click, and its limits. The scene
/// fov is usually very wide — a plate is baked through it — so a narrow
/// one is a click or two away for inspecting the geometry up close.
const FOV_PER_CLICK: f32 = 2.0;
const FOV_MIN: f32 = 20.0;
const FOV_MAX: f32 = 120.0;

/// World units per second at walking pace, and the multiplier while
/// shift is held.
const FLY_SPEED: f32 = 8.0;
const BOOST: f32 = 4.0;

/// Just shy of straight up: at exactly 90 degrees the look direction
/// and the up vector cross product that feeds the movement basis
/// collapses, and the camera flips over.
const PITCH_LIMIT: f32 = 1.5533;

/// The camera the editor flies. Tagged, not named: the editor has one.
#[derive(Component)]
pub struct EditorCamera;

/// The rotation a given yaw/pitch looks along, with pitch clamped.
///
/// Yaw turns about world up, then pitch about the camera's own right, so
/// pitch stays "look up" at every heading instead of rolling near the
/// poles.
pub fn look_rotation(yaw: f32, pitch: f32) -> Quat {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT))
}

/// The yaw a rotation carries: its heading about world up.
pub fn yaw_of(rotation: Quat) -> f32 {
    let (yaw, _, _) = rotation.to_euler(EulerRot::YXZ);
    yaw
}

/// The pitch a rotation carries, within the limit.
pub fn pitch_of(rotation: Quat) -> f32 {
    let (_, pitch, _) = rotation.to_euler(EulerRot::YXZ);
    pitch
}

/// Where the camera flies, given which way it looks and which keys are
/// down. Forward and right come off the yaw alone — pitching down must
/// not drive the camera into the floor — and up/down are world up.
pub fn fly_direction(rotation: Quat, forward: bool, back: bool, left: bool, right: bool) -> Vec3 {
    let yaw = yaw_of(rotation);
    // -Z is forward in Bevy's camera space, so -sin/-cos of the yaw.
    let heading = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let side = Vec3::new(-heading.z, 0.0, heading.x);
    let mut direction = Vec3::ZERO;
    if forward {
        direction += heading;
    }
    if back {
        direction -= heading;
    }
    if right {
        direction += side;
    }
    if left {
        direction -= side;
    }
    direction.normalize_or_zero()
}

/// The per-frame camera update: right-drag to look, WASD to fly, space
/// and ctrl for height, shift to boost.
pub fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cameras: Query<&mut Transform, With<EditorCamera>>,
) {
    let Ok(mut transform) = cameras.single_mut() else {
        return;
    };
    if buttons.pressed(MouseButton::Right) {
        let yaw = yaw_of(transform.rotation) - motion.delta.x * YAW_PER_PIXEL;
        let pitch = pitch_of(transform.rotation) - motion.delta.y * PITCH_PER_PIXEL;
        transform.rotation = look_rotation(yaw, pitch);
    }

    let pressed = |key: KeyCode| keys.pressed(key);
    let forward = pressed(KeyCode::KeyW);
    let back = pressed(KeyCode::KeyS);
    let left = pressed(KeyCode::KeyA);
    let right = pressed(KeyCode::KeyD);
    let up = pressed(KeyCode::Space);
    let down = pressed(KeyCode::ControlLeft) || pressed(KeyCode::ControlRight);
    let horizontal =
        fly_direction(transform.rotation, forward, back, left, right) * FLY_SPEED;
    let vertical = (up as i8 - down as i8) as f32 * FLY_SPEED;
    if horizontal == Vec3::ZERO && vertical == 0.0 {
        return;
    }
    let speed = if pressed(KeyCode::ShiftLeft) || pressed(KeyCode::ShiftRight) {
        BOOST
    } else {
        1.0
    };
    let step = (horizontal + Vec3::Y * vertical) * speed * time.delta_secs();
    transform.translation += step;
}

/// Widens or narrows the field of view on the scroll wheel. The scene
/// opens at the fov its plate was baked through, so what you see first is
/// what the game sees; scrolling narrows it for looking at the terrain up
/// close without leaving the app.
pub fn zoom(
    scroll: Res<AccumulatedMouseScroll>,
    mut cameras: Query<&mut Projection, With<EditorCamera>>,
) {
    let scroll = scroll.delta.y;
    if scroll == 0.0 {
        return;
    }
    let Ok(mut projection) = cameras.single_mut() else {
        return;
    };
    let Projection::Perspective(perspective) = &mut *projection else {
        return;
    };
    let to_radians = |degrees: f32| degrees.to_radians();
    perspective.fov = (perspective.fov - scroll * to_radians(FOV_PER_CLICK))
        .clamp(to_radians(FOV_MIN), to_radians(FOV_MAX));
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// A world with one editor camera at the origin looking down -Z, and
    /// the input resources `fly` reads.
    fn world() -> (World, Entity) {
        let mut world = World::new();
        let mut time = Time::<()>::default();
        time.advance_by(core::time::Duration::from_millis(500));
        world.insert_resource(time);
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.insert_resource(ButtonInput::<MouseButton>::default());
        world.insert_resource(AccumulatedMouseMotion::default());
        let camera = world
            .spawn((
                EditorCamera,
                Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::NEG_Z, Vec3::Y),
            ))
            .id();
        (world, camera)
    }

    fn at(world: &World, camera: Entity) -> Transform {
        *world.get::<Transform>(camera).unwrap()
    }

    #[test]
    fn holding_w_flies_the_way_the_camera_looks() {
        let (mut world, camera) = world();
        world.resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
        world.run_system_once(fly).unwrap();
        let moved = at(&world, camera).translation;
        // Half a second at the walking speed, heading -Z.
        assert!(moved.z < -1.0, "{moved:?}");
        assert!(moved.x.abs() < 1e-5, "no sideways drift: {moved:?}");
    }

    #[test]
    fn nothing_pressed_leaves_the_camera_alone() {
        let (mut world, camera) = world();
        let before = at(&world, camera);
        world.run_system_once(fly).unwrap();
        assert_eq!(at(&world, camera).translation, before.translation);
    }

    #[test]
    fn dragging_the_right_button_turns_the_camera() {
        let (mut world, camera) = world();
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(60.0, 0.0);
        world.run_system_once(fly).unwrap();
        let yaw = yaw_of(at(&world, camera).rotation);
        assert!(yaw < 0.0, "dragging right yaws left: {yaw}");
    }

    #[test]
    fn moving_the_mouse_without_the_button_does_nothing() {
        let (mut world, camera) = world();
        world.resource_mut::<AccumulatedMouseMotion>().delta = Vec2::new(60.0, 0.0);
        let before = at(&world, camera);
        world.run_system_once(fly).unwrap();
        assert_eq!(at(&world, camera).rotation, before.rotation);
    }

    #[test]
    fn pitch_stops_short_of_the_pole() {
        let up = look_rotation(0.0, 10.0);
        assert!((pitch_of(up) - PITCH_LIMIT).abs() < 1e-4);
        let down = look_rotation(0.0, -10.0);
        assert!((pitch_of(down) + PITCH_LIMIT).abs() < 1e-4);
    }

    #[test]
    fn looking_straight_ahead_flies_north() {
        // No yaw: the camera faces -Z.
        let direction = fly_direction(look_rotation(0.0, 0.0), true, false, false, false);
        assert!((direction - Vec3::NEG_Z).length() < 1e-5);
    }

    #[test]
    fn looking_down_does_not_push_the_camera_into_the_floor() {
        // Pitched 45 degrees down, forward is still horizontal: the
        // camera flies level and drops with the view instead.
        let down = look_rotation(0.0, -core::f32::consts::FRAC_PI_4);
        let direction = fly_direction(down, true, false, false, false);
        assert!(direction.y.abs() < 1e-5, "{direction:?}");
    }

    #[test]
    fn a_quarter_turn_carries_both_axes_with_it() {
        // Facing -Z: forward is -Z and right is +X. A quarter turn left
        // (yaw +90) turns that pair a quarter turn too, so the movement
        // basis can never drift out of step with where the camera looks.
        let straight = look_rotation(0.0, 0.0);
        let turned = look_rotation(std::f32::consts::FRAC_PI_2, 0.0);
        let ahead = fly_direction(straight, true, false, false, false);
        let aside = fly_direction(straight, false, false, false, true);
        assert!((ahead - Vec3::NEG_Z).length() < 1e-5, "{ahead:?}");
        assert!((aside - Vec3::X).length() < 1e-5, "{aside:?}");

        let ahead = fly_direction(turned, true, false, false, false);
        let aside = fly_direction(turned, false, false, false, true);
        assert!((ahead - Vec3::NEG_X).length() < 1e-5, "{ahead:?}");
        assert!((aside - Vec3::NEG_Z).length() < 1e-5, "{aside:?}");
    }

    #[test]
    fn no_keys_means_no_travel() {
        let direction = fly_direction(look_rotation(0.0, 0.0), false, false, false, false);
        assert_eq!(direction, Vec3::ZERO);
    }
}
