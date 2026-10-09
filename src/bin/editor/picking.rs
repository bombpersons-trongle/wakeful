//! Picking an actor out of the viewport.
//!
//! Selection here is the editor's own ray test rather than Bevy's picking
//! backend: the thing you click is the box the highlight draws, and using
//! the same box for both means a click lands where the outline is rather
//! than on whatever triangle happens to be under the cursor.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

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

/// A sweep being dragged across the viewport: where it started, and
/// whether the button was down on the last frame.
///
/// The button state is carried rather than read as an edge because a press
/// and a release can land in the same frame, and a sweep is something that
/// happens across frames.
#[derive(Resource, Default)]
pub struct ViewportSweep {
    from: Option<Vec2>,
    was_down: bool,
}

/// How far the pointer may travel between press and release and still
/// count as a click on whatever was under it, in logical pixels.
const CLICK_SLOP: f32 = 6.0;

/// The rectangle between two cursor positions, in either order: a drag
/// can go any direction, and a rectangle has corners not a start and an
/// end.
fn brush_between(from: Vec2, to: Vec2) -> egui::Rect {
    egui::Rect::from_min_max(point(from.min(to)), point(from.max(to)))
}

/// A Bevy cursor position as an egui point.
fn point(at: Vec2) -> egui::Pos2 {
    egui::pos2(at.x, at.y)
}

/// How far in front of the camera the sweep's rectangle is drawn. Gizmos
/// are drawn without a depth test, so this only decides how much of the
/// scene the band covers on its way there.
const BAND_DISTANCE: f32 = 1.0;

/// Selects in the viewport: a click picks the actor whose box the cursor
/// went down on, shift-click adds it to or takes it from the selection, and
/// a drag sweeps up everything it covers.
///
/// Clicks egui wants belong to egui: the actors panel and every menu in it
/// are drawn over the very thing being clicked, and a click that opened a
/// menu must not also select whatever was behind it. A sweep already under
/// way is exempt — the pointer will have crossed a window on its way, and
/// it is the drag that was asked for, not the window it passed.
#[allow(clippy::too_many_arguments)]
pub fn select_in_viewport(
    mut ctxs: EguiContexts,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::camera::EditorCamera>>,
    standing: Query<(&EditorActor, &Transform)>,
    mut selected: ResMut<Selected>,
    mut sweep: ResMut<ViewportSweep>,
    mut gizmos: Gizmos,
) {
    let Ok((camera, transform)) = cameras.single() else {
        return;
    };
    let Some(cursor) = windows.iter().find_map(|window| window.cursor_position()) else {
        return;
    };
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let down = buttons.pressed(MouseButton::Left);
    let just_pressed = down && !sweep.was_down;
    let just_released = !down && sweep.was_down;
    let sweeping = sweep.from.is_some();
    sweep.was_down = down;

    if just_pressed
        && !sweeping
        && !ctxs
            .ctx_mut()
            .is_ok_and(|ctx| ctx.is_pointer_over_egui())
    {
        sweep.from = Some(cursor);
    }
    if let (true, Some(from)) = (just_released, sweep.from.take()) {
        if (cursor - from).length() > CLICK_SLOP {
            let boxes = screen_boxes(camera, transform, &standing);
            selected.sweep(brush_between(from, cursor), boxes.into_iter(), shift);
        } else if let Some(hit) = hit_actor(camera, transform, cursor, &standing) {
            if shift {
                selected.toggle(hit);
            } else {
                selected.only(hit);
            }
        } else if !shift {
            // A click on nothing clears, the way clicking empty space in a
            // file list does.
            selected.clear();
        }
    }
    if let Some(from) = sweep.from
        && (cursor - from).length() > CLICK_SLOP
    {
        draw_band(&mut gizmos, camera, transform, brush_between(from, cursor));
    }
}

/// The actor box of every standing actor, as a rectangle on the screen.
///
/// A box is where any of its corners lands, which is what makes a stroke
/// drawn loosely across a row of actors catch them: the boxes are small
/// and far apart, and asking for all eight corners inside the brush would
/// miss almost every brush stroke that visibly goes over one.
fn screen_boxes(
    camera: &Camera,
    transform: &GlobalTransform,
    standing: &Query<(&EditorActor, &Transform)>,
) -> Vec<(usize, egui::Rect)> {
    standing
        .iter()
        .filter_map(|(actor, at)| {
            let corners = box_corners(at.translation);
            let mut min = Vec2::splat(f32::INFINITY);
            let mut max = Vec2::splat(f32::NEG_INFINITY);
            for corner in corners {
                let Ok(at) = camera.world_to_viewport(transform, corner) else {
                    continue;
                };
                min = min.min(at);
                max = max.max(at);
            }
            (min.x <= max.x)
                .then_some((actor.index, egui::Rect::from_min_max(point(min), point(max))))
        })
        .collect()
}

/// The eight corners of the box an actor is picked within.
fn box_corners(base: Vec3) -> impl Iterator<Item = Vec3> {
    let (w, d, h) = (BOX_HALF_WIDTH, BOX_HALF_WIDTH, BOX_HEIGHT);
    [
        (0.0, 0.0),
        (w, 0.0),
        (w, d),
        (0.0, d),
        (0.0, h),
        (w, h),
        (w, d + h),
        (0.0, d + h),
    ]
    .into_iter()
    .map(move |(dx, y)| base + Vec3::new(dx, y, dx))
}

/// The actor whose box the cursor is inside, nearest the camera first:
/// two actors can overlap on screen, and the one in front is the one you
/// can see.
fn hit_actor(
    camera: &Camera,
    transform: &GlobalTransform,
    cursor: Vec2,
    standing: &Query<(&EditorActor, &Transform)>,
) -> Option<usize> {
    let Ok(ray) = camera.viewport_to_world(transform, cursor) else {
        return None;
    };
    standing
        .iter()
        .filter_map(|(actor, at)| {
            let (min, max) = actor_box(at.translation);
            ray_hits_box(ray.origin, ray.direction.into(), min, max).map(|along| (along, actor.index))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, index)| index)
}

/// Draws the sweep's rectangle where the pointer drew it: the four corners
/// are unprojected onto a plane in front of the camera, so the band covers
/// exactly the rectangle the test used whatever the projection does.
fn draw_band(
    gizmos: &mut Gizmos,
    camera: &Camera,
    transform: &GlobalTransform,
    brush: egui::Rect,
) {
    let color = Color::srgb(1.0, 0.65, 0.15);
    let corners: Vec<Vec3> = [
        brush.left_top(),
        brush.right_top(),
        brush.right_bottom(),
        brush.left_bottom(),
    ]
    .into_iter()
    .filter_map(|corner| on_band_plane(camera, transform, corner))
    .collect();
    if corners.len() != 4 {
        return;
    }
    for (at, next) in corners.iter().enumerate() {
        gizmos.line(*next, corners[(at + 1) % 4], color);
    }
}

/// Where one corner of the rectangle sits on the band plane: a step along
/// the ray the corner's pixel would have taken, which puts every corner on
/// one plane and so keeps the band closed.
fn on_band_plane(
    camera: &Camera,
    transform: &GlobalTransform,
    corner: egui::Pos2,
) -> Option<Vec3> {
    let ray = camera
        .viewport_to_world(transform, Vec2::new(corner.x, corner.y))
        .ok()?;
    Some(ray.origin + ray.direction.normalize() * BAND_DISTANCE)

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