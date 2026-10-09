//! Picking an actor out of the viewport.
//!
//! Selection here is the editor's own ray test rather than Bevy's picking
//! backend: the thing you click is the box the highlight draws, and using
//! the same box for both means a click lands where the outline is rather
//! than on whatever triangle happens to be under the cursor.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::actors::{BOX_HALF_WIDTH, BOX_HEIGHT, EditorActor, Selected};

/// The distance along a ray at which it enters a box, or `None` when it
/// misses. The slab test, which unlike a mesh test costs nothing and
/// cannot be confused by a model's bones or a plate's depth.
pub fn ray_hits_box(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let direction = direction.normalize();
    let mut near = f32::NEG_INFINITY;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let (start, step, low, high) = (
            origin[axis],
            direction[axis],
            min[axis],
            max[axis],
        );
        if step.abs() < f32::EPSILON {
            // Parallel to this slab: either inside it for all time, or
            // never inside it at all.
            if start < low || start > high {
                return None;
            }
            continue;
        }
        let (enter, exit) = ((low - start) / step, (high - start) / step);
        let (enter, exit) = if enter <= exit { (enter, exit) } else { (exit, enter) };
        near = near.max(enter);
        far = far.min(exit);
        if near > far {
            return None;
        }
    }
    Some(near.max(0.0))
}

/// The box an actor is clicked within: the same one the selection draws.
fn actor_box(at: Vec3) -> (Vec3, Vec3) {
    (
        at + Vec3::new(-BOX_HALF_WIDTH, 0.0, -BOX_HALF_WIDTH),
        at + Vec3::new(BOX_HALF_WIDTH, BOX_HEIGHT, BOX_HALF_WIDTH),
    )
}

/// Selects the actor whose box the cursor is over when the left button
/// goes down, and nothing when the click lands on empty ground.
///
/// Clicks egui wants belong to egui: the actors panel and every menu in
/// it are drawn over the very thing being clicked, and a click that opened
/// a menu must not also select whatever was behind it.
pub fn select_on_click(
    mut ctxs: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    mut selected: ResMut<Selected>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::camera::EditorCamera>>,
    standing: Query<(&EditorActor, &Transform)>,
) {
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    if ctxs
        .ctx_mut()
        .is_ok_and(|ctx| ctx.is_pointer_over_egui())
    {
        return;
    }
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let Some(cursor) = windows.iter().find_map(|window| window.cursor_position()) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return;
    };
    // The nearest box wins: two actors can overlap on screen, and the one
    // in front is the one you can see.
    let hit = standing
        .iter()
        .filter_map(|(actor, at)| {
            let (min, max) = actor_box(at.translation);
            ray_hits_box(ray.origin, ray.direction.into(), min, max).map(|along| (along, actor.index))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, index)| index);
    selected.0 = hit;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_around() -> (Vec3, Vec3) {
        actor_box(Vec3::ZERO)
    }

    #[test]
    fn a_ray_straight_down_through_the_box_hits_its_top() {
        let (min, max) = box_around();
        let along = ray_hits_box(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y, min, max).unwrap();
        // Five units down to the top of the box.
        assert!((along - (5.0 - BOX_HEIGHT)).abs() < 1e-4, "{along}");
    }

    #[test]
    fn a_ray_that_passes_beside_the_box_misses() {
        let (min, max) = box_around();
        // Across the box's height and width, but a body-width to the side
        // of it in depth.
        let beside = ray_hits_box(Vec3::new(5.0, 0.5, 5.0), Vec3::NEG_X, min, max);
        assert!(beside.is_none());
    }

    #[test]
    fn a_ray_above_and_beside_misses() {
        let (min, max) = box_around();
        assert!(ray_hits_box(Vec3::new(5.0, 9.0, 0.0), Vec3::NEG_Y, min, max).is_none());
    }

    #[test]
    fn a_ray_starting_inside_reports_no_distance() {
        let (min, max) = box_around();
        let along = ray_hits_box(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Y, min, max).unwrap();
        assert_eq!(along, 0.0);
    }

    #[test]
    fn the_nearer_of_two_boxes_wins() {
        let (min, max) = box_around();
        let far = actor_box(Vec3::new(0.0, 0.0, -6.0));
        let ray = Vec3::new(0.0, 0.9, 0.0);
        let ahead = ray_hits_box(ray, Vec3::NEG_Z, min, max);
        let behind = ray_hits_box(ray, Vec3::NEG_Z, far.0, far.1);
        assert!(ahead.unwrap() < behind.unwrap());
    }
}